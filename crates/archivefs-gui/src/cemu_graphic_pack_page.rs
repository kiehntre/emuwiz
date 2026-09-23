//! Cemu Graphic Pack inspection, review, apply and undo panel.
//!
//! This is deliberately a thin GUI adapter over the core inspector and the
//! shared transaction journal. It never writes package contents or game media.

use std::path::{Path, PathBuf};

use archivefs_core::game_identity::GameIdentityReport;
use archivefs_core::patch_manager::{
    CemuGraphicPackInspection, CemuGraphicPackPlan, CemuProfileDiscovery,
    CemuProfileDiscoveryRoots, SharedApplyConfirmation, SharedApplyOptions, SharedApplyResult,
    SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions,
    build_cemu_graphic_pack_plan, build_cemu_graphic_pack_transaction_plan,
    cemu_graphic_packs_root, default_shared_backup_root, default_shared_history_root,
    discover_cemu_profiles, execute_shared_rollback, generate_shared_operation_id,
    inspect_cemu_graphic_pack, inspect_cemu_graphic_pack_zip, preview_cemu_graphic_pack_rollback,
    verified_cemu_title_id,
};
use eframe::egui;

use crate::ui::components as widgets;

#[derive(Default)]
pub struct CemuGraphicPackPageState {
    archive: Option<PathBuf>,
    source: Option<PathBuf>,
    staging_root: Option<PathBuf>,
    inspection: Option<CemuGraphicPackInspection>,
    plan: Option<CemuGraphicPackPlan>,
    applied: Option<SharedApplyResult>,
    error: Option<String>,
    selected_profile: Option<String>,
    confirm_replace: bool,
}

impl Drop for CemuGraphicPackPageState {
    fn drop(&mut self) {
        if let Some(root) = &self.staging_root {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

impl CemuGraphicPackPageState {
    fn reset_for(&mut self, archive: &Path) {
        if self.archive.as_deref() == Some(archive) {
            return;
        }
        self.archive = Some(archive.to_path_buf());
        if let Some(root) = self.staging_root.take() {
            let _ = std::fs::remove_dir_all(root);
        }
        self.source = None;
        self.inspection = None;
        self.plan = None;
        self.applied = None;
        self.error = None;
        self.selected_profile = None;
        self.confirm_replace = false;
    }
}

pub fn show_cemu_graphic_pack_panel(
    ui: &mut egui::Ui,
    state: &mut CemuGraphicPackPageState,
    archive_path: &Path,
    identity: &GameIdentityReport,
) {
    if identity.platform != archivefs_core::game_identity::IdentityPlatform::WiiU {
        return;
    }
    state.reset_for(archive_path);
    widgets::section_header(
        ui,
        "Cemu graphic pack",
        Some(
            "Inspect a Cemu graphic pack, review every file, install it into Cemu's graphicPacks folder, and undo it safely.",
        ),
    );
    let discovery = CemuProfileDiscoveryRoots::from_environment()
        .map(|roots| discover_cemu_profiles(&roots))
        .unwrap_or(CemuProfileDiscovery {
            profiles: Vec::new(),
            complete: false,
        });
    widgets::card(ui, |ui| {
        ui.label("Choose a Cemu graphic-pack folder or ZIP. EmuWiz reads it without changing the package.");
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Inspect graphic-pack folder",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
                && let Some(path) = rfd::FileDialog::new().pick_folder()
            {
                inspect_selected(state, path);
            }
            if widgets::action_button(
                ui,
                "Inspect graphic-pack ZIP",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                inspect_selected(state, path);
            }
        });
        if let Some(error) = &state.error {
            widgets::banner(
                ui,
                "Graphic pack needs review",
                error,
                widgets::StatusTone::Blocked,
            );
        }
    });
    let Some(inspection) = state.inspection.clone() else {
        return;
    };
    let Some(title_id) = verified_cemu_title_id(identity, archive_path) else {
        widgets::banner(
            ui,
            "Target game could not be confirmed",
            "The selected Wii U title has no safe structural title-ID evidence. EmuWiz will not guess from a filename or pack name.",
            widgets::StatusTone::Warning,
        );
        return;
    };
    let eligible: Vec<_> = discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .collect();
    if eligible.is_empty() {
        widgets::banner(
            ui,
            "Cemu profile unavailable",
            "No eligible Cemu profile was discovered. Configure Cemu first; EmuWiz will not invent a destination.",
            widgets::StatusTone::Warning,
        );
        return;
    }
    let selected_id = state
        .selected_profile
        .clone()
        .or_else(|| eligible.first().map(|p| p.profile_id.clone()));
    state.selected_profile = selected_id.clone();
    let Some(profile) = selected_id
        .as_deref()
        .and_then(|id| discovery.profiles.iter().find(|p| p.profile_id == id))
    else {
        return;
    };
    let Ok(destination_root) = cemu_graphic_packs_root(profile) else {
        return;
    };
    widgets::card(ui, |ui| {
        ui.strong(format!("Pack: {}", inspection.pack_name));
        ui.label(format!(
            "Target title ID(s): {}",
            inspection.title_ids.join(", ")
        ));
        ui.label(format!("Selected game title ID: {title_id}"));
        ui.label(format!(
            "Files: {} · expanded size: {} bytes",
            inspection.files.len(),
            inspection.total_bytes
        ));
        ui.label(format!(
            "Destination: {}",
            destination_root.join(&inspection.pack_name).display()
        ));
        if eligible.len() > 1 {
            egui::ComboBox::from_label("Cemu profile")
                .selected_text(profile.profile_id.as_str())
                .show_ui(ui, |ui| {
                    for candidate in &eligible {
                        ui.selectable_value(
                            &mut state.selected_profile,
                            Some(candidate.profile_id.clone()),
                            candidate.profile_id.as_str(),
                        );
                    }
                });
        }
        if state.plan.is_none()
            && widgets::action_button(
                ui,
                "Review exact changes",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
        {
            match build_cemu_graphic_pack_plan(
                &inspection,
                &title_id,
                archive_path,
                &destination_root,
            )
            .and_then(|plan| {
                build_cemu_graphic_pack_transaction_plan(&plan, &profile.profile_id).map(|_| plan)
            }) {
                Ok(plan) => {
                    state.plan = Some(plan);
                    state.error = None;
                }
                Err(error) => state.error = Some(error.to_string()),
            }
        }
    });
    if let Some(plan) = state.plan.clone() {
        show_plan(ui, state, &plan, &profile.profile_id);
    }
    if let Some(applied) = state.applied.as_ref() {
        widgets::card(ui, |ui| {
            let label = match applied.journal.status {
                SharedApplyStatus::Success => "Installed successfully",
                SharedApplyStatus::PartialFailure => "Install partially failed",
                _ => "Install failed",
            };
            ui.strong(label);
            if let Some(path) = &applied.journal_path {
                ui.label(format!("Receipt: {}", path.display()));
            }
            if applied.journal.status == SharedApplyStatus::Success
                && widgets::action_button(
                    ui,
                    "Undo this install",
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
            {
                let Some(journal_path) = applied.journal_path.as_ref() else {
                    return;
                };
                let Ok(history_root) = default_shared_history_root() else {
                    return;
                };
                let Ok(backup_root) = default_shared_backup_root() else {
                    return;
                };
                let preview = preview_cemu_graphic_pack_rollback(
                    journal_path,
                    &applied
                        .journal
                        .destination_root
                        .to_path_buf()
                        .unwrap_or_default(),
                    &backup_root,
                );
                let result = execute_shared_rollback(
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
                );
                if result.status == SharedApplyStatus::Success {
                    ui.label("Undo completed; unrelated Cemu files were preserved.");
                } else {
                    ui.label("Undo was blocked because the destination changed.");
                }
            }
        });
    }
}

fn inspect_selected(state: &mut CemuGraphicPackPageState, path: PathBuf) {
    state.source = Some(path.clone());
    state.plan = None;
    state.applied = None;
    state.error = None;
    if path.is_dir() {
        match inspect_cemu_graphic_pack(&path) {
            Ok(inspection) => state.inspection = Some(inspection),
            Err(error) => state.error = Some(error.to_string()),
        }
    } else {
        match inspect_cemu_graphic_pack_zip(&path) {
            Ok(preview) => {
                state.staging_root = Some(preview.staging_root);
                state.inspection = Some(preview.inspection);
            }
            Err(error) => state.error = Some(error.to_string()),
        }
    }
}

fn show_plan(
    ui: &mut egui::Ui,
    state: &mut CemuGraphicPackPageState,
    plan: &CemuGraphicPackPlan,
    profile_id: &str,
) {
    widgets::card(ui, |ui| {
        ui.strong("Exact changes");
        for entry in &plan.report.entries {
            let action = match entry.proposed_action {
                archivefs_core::patch_manager::PreviewProposedAction::Install => "Create",
                archivefs_core::patch_manager::PreviewProposedAction::Replace => "Replace",
                archivefs_core::patch_manager::PreviewProposedAction::Skip => "Already installed",
                archivefs_core::patch_manager::PreviewProposedAction::Blocked => "Blocked",
            };
            ui.label(format!(
                "{action}: {}",
                entry
                    .destination_path
                    .as_deref()
                    .unwrap_or(Path::new("(blocked)"))
                    .display()
            ));
        }
        ui.label(format!(
            "Install: {} · replace: {} · already installed: {} · blocked: {}",
            plan.report.summary.install_new,
            plan.report.summary.replace_different,
            plan.report.summary.already_installed,
            plan.report.summary.blocked
        ));
        if plan.report.complete && plan.report.summary.blocked == 0 {
            if widgets::action_button(
                ui,
                "Install reviewed pack",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                match build_cemu_graphic_pack_transaction_plan(plan, profile_id)
                    .map_err(|error| error.to_string())
                    .and_then(apply_transaction)
                {
                    Ok(result) => {
                        state.applied = Some(result.apply);
                        state.plan = None;
                    }
                    Err(error) => state.error = Some(error),
                }
            }
        } else {
            ui.label("Install is blocked until every file and target is safe and unambiguous.");
        }
    });
}

fn apply_transaction(
    plan: archivefs_core::patch_manager::SharedTransactionPlan,
) -> Result<archivefs_core::patch_manager::CemuGraphicPackApplyResult, String> {
    let history_root = default_shared_history_root().map_err(|e| e.detail)?;
    let backup_root = default_shared_backup_root().map_err(|e| e.detail)?;
    Ok(archivefs_core::patch_manager::apply_cemu_graphic_pack(
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
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
