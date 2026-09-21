//! Beginner-facing ordinary RPCS3 file-layer mod workflow.
//!
//! This panel delegates package inspection, exact preview, identity checks,
//! and rollback to archivefs-core's shared transaction machinery.

use std::path::{Path, PathBuf};

use archivefs_core::game_identity::GameIdentityReport;
use archivefs_core::patch_manager::{
    Rpcs3OrdinaryModInspection, Rpcs3OrdinaryModPlan, Rpcs3ProfileDiscovery,
    Rpcs3ProfileDiscoveryRoots, SharedApplyConfirmation, SharedApplyOptions, SharedApplyResult,
    SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions,
    build_rpcs3_ordinary_mod_plan, build_rpcs3_ordinary_mod_transaction_plan,
    default_shared_backup_root, default_shared_history_root, discover_rpcs3_profiles,
    execute_shared_rollback, generate_shared_operation_id, inspect_rpcs3_ordinary_mod,
    inspect_rpcs3_ordinary_mod_zip, preview_rpcs3_ordinary_mod_rollback,
    rpcs3_ordinary_mod_destination_root,
};
use eframe::egui;

use crate::ui::components as widgets;

#[derive(Default)]
pub struct Rpcs3OrdinaryModPageState {
    archive: Option<PathBuf>,
    source: Option<PathBuf>,
    staging_root: Option<PathBuf>,
    inspection: Option<Rpcs3OrdinaryModInspection>,
    plan: Option<Rpcs3OrdinaryModPlan>,
    applied: Option<SharedApplyResult>,
    error: Option<String>,
    selected_profile: Option<String>,
}

impl Drop for Rpcs3OrdinaryModPageState {
    fn drop(&mut self) {
        if let Some(root) = &self.staging_root {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

impl Rpcs3OrdinaryModPageState {
    pub(crate) fn reset_for(&mut self, archive: &Path) {
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
    }
}

pub(crate) fn show_rpcs3_ordinary_mod_panel(
    ui: &mut egui::Ui,
    state: &mut Rpcs3OrdinaryModPageState,
    archive_path: &Path,
    identity: &GameIdentityReport,
) {
    if identity.platform != archivefs_core::game_identity::IdentityPlatform::PlayStation3 {
        return;
    }
    state.reset_for(archive_path);
    widgets::section_header(
        ui,
        "RPCS3 ordinary mods",
        Some(
            "Inspect a normal RPCS3 file-layer mod, review every change, install it into the selected game, and undo it safely.",
        ),
    );
    let discovery = Rpcs3ProfileDiscoveryRoots::from_environment()
        .map(|roots| discover_rpcs3_profiles(&roots))
        .unwrap_or(Rpcs3ProfileDiscovery {
            profiles: Vec::new(),
            warnings: Vec::new(),
            complete: false,
        });
    widgets::card(ui, |ui| {
        ui.label("Choose a mod folder or ZIP. EmuWiz reads the package without changing it.");
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Inspect RPCS3 mod folder",
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
                "Inspect RPCS3 mod ZIP",
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
                "RPCS3 mod needs review",
                error,
                widgets::StatusTone::Blocked,
            );
        }
    });
    let Some(inspection) = state.inspection.clone() else {
        return;
    };
    let Some(title_id) = identity
        .verified_ps3_title_id()
        .map(str::to_ascii_uppercase)
    else {
        widgets::banner(
            ui,
            "Target game could not be confirmed",
            "The selected game has no single verified PS3 Title ID. EmuWiz will not guess from the mod name or folder.",
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
            "RPCS3 profile unavailable",
            "No eligible RPCS3 profile was discovered. EmuWiz will not invent a game destination.",
            widgets::StatusTone::Warning,
        );
        return;
    }
    let selected_id = state
        .selected_profile
        .clone()
        .or_else(|| eligible.first().map(|profile| profile.profile_id.clone()));
    state.selected_profile = selected_id.clone();
    let Some(profile) = selected_id.as_deref().and_then(|id| {
        discovery
            .profiles
            .iter()
            .find(|profile| profile.profile_id == id)
    }) else {
        return;
    };
    let Ok(destination_root) = rpcs3_ordinary_mod_destination_root(profile, &title_id) else {
        return;
    };
    widgets::card(ui, |ui| {
        ui.strong(format!("Mod: {}", inspection.mod_name));
        ui.label(format!("Target game Title ID: {title_id}"));
        ui.label(format!(
            "Package identity: {}",
            inspection.declared_title_id
        ));
        ui.label(format!(
            "Files: {} · expanded size: {} bytes",
            inspection.files.len(),
            inspection.total_bytes
        ));
        ui.label(format!("Destination: {}", destination_root.display()));
        if eligible.len() > 1 {
            egui::ComboBox::from_label("RPCS3 profile")
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
            match build_rpcs3_ordinary_mod_plan(&inspection, identity, &destination_root).and_then(
                |plan| {
                    build_rpcs3_ordinary_mod_transaction_plan(&plan, &profile.profile_id)
                        .map(|_| plan)
                },
            ) {
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
                SharedApplyStatus::Success => "RPCS3 mod installed",
                SharedApplyStatus::PartialFailure => "RPCS3 mod partially installed",
                _ => "RPCS3 mod install failed",
            };
            ui.strong(label);
            if let Some(path) = &applied.journal_path {
                ui.label(format!("Receipt: {}", path.display()));
            }
            if applied.journal.status == SharedApplyStatus::Success
                && widgets::action_button(
                    ui,
                    "Undo this mod install",
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
                let destination = applied
                    .journal
                    .destination_root
                    .to_path_buf()
                    .unwrap_or_default();
                let preview =
                    preview_rpcs3_ordinary_mod_rollback(journal_path, &destination, &backup_root);
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
                ui.label(if result.status == SharedApplyStatus::Success {
                    "Undo completed; unrelated RPCS3 files were preserved."
                } else {
                    "Undo was blocked because the destination changed."
                });
            }
        });
    }
}

fn inspect_selected(state: &mut Rpcs3OrdinaryModPageState, path: PathBuf) {
    state.source = Some(path.clone());
    state.plan = None;
    state.applied = None;
    state.error = None;
    if path.is_dir() {
        match inspect_rpcs3_ordinary_mod(&path) {
            Ok(inspection) => state.inspection = Some(inspection),
            Err(error) => state.error = Some(error.to_string()),
        }
    } else {
        match inspect_rpcs3_ordinary_mod_zip(&path) {
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
    state: &mut Rpcs3OrdinaryModPageState,
    plan: &Rpcs3OrdinaryModPlan,
    profile_id: &str,
) {
    widgets::card(ui, |ui| {
        ui.strong("Exact changes");
        for entry in &plan.report.entries {
            let action = match entry.proposed_action {
                archivefs_core::patch_manager::PreviewProposedAction::Install => "Create",
                archivefs_core::patch_manager::PreviewProposedAction::Replace => {
                    "Replace (backup kept)"
                }
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
            "Create: {} · replace: {} · already installed: {} · blocked: {}",
            plan.report.summary.install_new,
            plan.report.summary.replace_different,
            plan.report.summary.already_installed,
            plan.report.summary.blocked
        ));
        if plan.report.complete
            && plan.report.summary.blocked == 0
            && widgets::action_button(
                ui,
                "Install reviewed mod",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
        {
            match build_rpcs3_ordinary_mod_transaction_plan(plan, profile_id)
                .map_err(|error| error.to_string())
                .and_then(apply_transaction)
            {
                Ok(result) => {
                    state.applied = Some(result.apply);
                    state.plan = None;
                }
                Err(error) => state.error = Some(error),
            }
        } else if !plan.report.complete || plan.report.summary.blocked != 0 {
            ui.label(
                "Install is blocked until every file and destination is safe and unambiguous.",
            );
        }
    });
}

fn apply_transaction(
    plan: archivefs_core::patch_manager::SharedTransactionPlan,
) -> Result<archivefs_core::patch_manager::Rpcs3OrdinaryModApplyResult, String> {
    let history_root = default_shared_history_root().map_err(|e| e.detail)?;
    let backup_root = default_shared_backup_root().map_err(|e| e.detail)?;
    Ok(archivefs_core::patch_manager::apply_rpcs3_ordinary_mod(
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
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::game_identity::{
        GameIdentityReport, IdentityImageFormat, IdentityPlatform,
    };

    #[test]
    fn ordinary_rpcs3_panel_starts_with_plain_language_actions() {
        let context = egui::Context::default();
        let root =
            std::env::temp_dir().join(format!("archivefs-rpcs3-gui-test-{}", std::process::id()));
        let game = root.join("game.iso");
        let identity = GameIdentityReport {
            archive_path: game.clone(),
            platform: IdentityPlatform::PlayStation3,
            format: IdentityImageFormat::Iso,
            evidence: Vec::new(),
            warnings: Vec::new(),
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        };
        let mut state = Rpcs3OrdinaryModPageState::default();
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_rpcs3_ordinary_mod_panel(ui, &mut state, &game, &identity);
            });
        });
        fn contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text().contains(needle),
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains(shape, needle)),
                _ => false,
            }
        }
        assert!(
            output
                .shapes
                .iter()
                .any(|clipped| contains(&clipped.shape, "RPCS3 ordinary mods"))
        );
        assert!(
            output
                .shapes
                .iter()
                .any(|clipped| contains(&clipped.shape, "Inspect RPCS3 mod folder"))
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
