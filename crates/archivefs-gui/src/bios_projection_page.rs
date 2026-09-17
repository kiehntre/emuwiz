//! BIOS / firmware projection review and explicit immutable-link setup.

use archivefs_core::bios_projection::{
    self, BiosProjectionAction, BiosProjectionMethod, BiosProjectionPlan, BiosProjectionResult,
    BiosProjectionStatus,
};
use eframe::egui;
use std::path::PathBuf;

struct BiosApplyDialog {
    plan: BiosProjectionPlan,
    target_root: PathBuf,
    confirmation: String,
}

#[derive(Default)]
pub(crate) struct BiosProjectionPageState {
    master_root: String,
    target_root: String,
    result: Option<BiosProjectionResult>,
    error: Option<String>,
    feedback: Option<String>,
    apply_dialog: Option<BiosApplyDialog>,
}

impl BiosProjectionPageState {
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("BIOS / Firmware");
        ui.label(
            "Inspect a master BIOS collection and review safe, emulator-specific setup plans.",
        );
        ui.horizontal(|ui| {
            ui.label("Master BIOS root:");
            ui.add(egui::TextEdit::singleline(&mut self.master_root).desired_width(420.0));
            if ui.button("Inspect master BIOS root").clicked() {
                self.inspect();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Approved target folder:");
            ui.add(egui::TextEdit::singleline(&mut self.target_root).desired_width(420.0));
        });
        ui.small("EmuWiz creates BIOS links in this explicitly supplied folder only; it does not edit emulator settings.");
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(180, 70, 55), error);
        }
        if let Some(feedback) = &self.feedback {
            ui.colored_label(egui::Color32::from_rgb(70, 150, 85), feedback);
        }
        let Some(result) = &self.result else {
            ui.separator();
            ui.label("Choose a master BIOS root to begin a bounded read-only inspection.");
            return;
        };
        ui.separator();
        ui.label(format!("Master root: {}", result.inventory.root.display()));
        ui.label(format!(
            "{} files discovered; {} inventory warnings",
            result.inventory.entries.len(),
            result.inventory.warnings.len()
        ));
        for warning in &result.inventory.warnings {
            ui.colored_label(egui::Color32::from_rgb(180, 130, 40), warning);
        }
        let target_root = PathBuf::from(self.target_root.trim());
        let mut requested_apply = None;
        for plan in &result.plans {
            let bound = if target_root.as_os_str().is_empty() {
                None
            } else {
                bios_projection::bind_plan_to_target_root(plan, &target_root).ok()
            };
            egui::CollapsingHeader::new(plan.emulator.clone()).show(ui, |ui| {
                ui.label(format!("Status: {}", status_label(plan.status)));
                for (index, requirement) in plan.requirements.iter().enumerate() {
                    ui.group(|ui| {
                        ui.label(egui::RichText::new(&requirement.name).strong());
                        ui.label(format!("Target: {}", requirement.target.description));
                        ui.label(format!("Method: {}", method_label(requirement.method)));
                        if let Some(Some(source)) = plan.matches.get(index) {
                            ui.label(format!(
                                "Source: {} ({})",
                                source.relative_path.display(),
                                match_label(source.match_status)
                            ));
                        } else if requirement.method == BiosProjectionMethod::NoBiosRequired {
                            ui.label("No conventional BIOS is required.");
                        } else {
                            ui.label("No source match was found in this master root.");
                        }
                        if let Some(bound_plan) = &bound
                            && let Some(bound_requirement) = bound_plan.requirements.get(index)
                        {
                            ui.label(format!(
                                "Target state: {}",
                                target_state_label(bound_requirement.target.current_state)
                            ));
                        }
                        if let Some(BiosProjectionAction::KeepWritableLocal { .. }) =
                            plan.actions.get(index)
                        {
                            ui.colored_label(
                                egui::Color32::from_rgb(180, 130, 40),
                                "This is writable emulator state and must remain local.",
                            );
                        }
                    });
                }
                if let Some(bound_plan) = &bound {
                    if applyable(bound_plan) {
                        if ui.button("Preview BIOS setup").clicked() {
                            requested_apply = Some(bound_plan.clone());
                        }
                    } else if plan.status == BiosProjectionStatus::Ready
                        || plan.status == BiosProjectionStatus::AlreadyProjected
                    {
                        ui.small("Setup is unavailable because this plan is external, writable, or still needs review.");
                    }
                }
                for warning in &plan.warnings {
                    ui.colored_label(egui::Color32::from_rgb(180, 130, 40), warning);
                }
            });
        }
        if let Some(plan) = requested_apply {
            self.apply_dialog = Some(BiosApplyDialog {
                plan,
                target_root,
                confirmation: String::new(),
            });
        }
        self.show_apply_dialog(ui);
    }

    fn inspect(&mut self) {
        self.error = None;
        self.feedback = None;
        let root = PathBuf::from(self.master_root.trim());
        match bios_projection::inspect_master_root(&root) {
            Ok(inventory) => self.result = Some(bios_projection::plan_projections(&inventory)),
            Err(error) => {
                self.result = None;
                self.error = Some(error.to_string());
            }
        }
    }

    fn show_apply_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(dialog) = &mut self.apply_dialog else {
            return;
        };
        let mut close = false;
        egui::Window::new("Preview BIOS setup")
            .collapsible(false)
            .show(ui.ctx(), |ui| {
                ui.label(format!("Emulator: {}", dialog.plan.emulator));
                ui.label(format!("Master BIOS source: {}", dialog.plan.master_root.display()));
                ui.label(format!("Approved target folder: {}", dialog.target_root.display()));
                ui.label("Operation: create symbolic links to the selected immutable BIOS files.");
                ui.label("BIOS source files remain unchanged. Emulator configuration is not edited.");
                for (index, requirement) in dialog.plan.requirements.iter().enumerate() {
                    if let Some(Some(source)) = dialog.plan.matches.get(index)
                        && let Some(target) = &requirement.target.path
                    {
                        ui.label(format!(
                            "{}: {} → {}",
                            requirement.name,
                            source.relative_path.display(),
                            target.display()
                        ));
                    }
                }
                ui.separator();
                let phrase = bios_projection::apply_confirmation(dialog.plan.requirements.len());
                ui.label(format!("Type {phrase} to confirm:"));
                ui.add(egui::TextEdit::singleline(&mut dialog.confirmation).desired_width(360.0));
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if ui.button("Confirm and apply").clicked() {
                        match bios_projection::apply_plan(
                            &dialog.plan,
                            &dialog.target_root,
                            dialog.confirmation.trim(),
                        ) {
                            Ok(transaction) => {
                                self.feedback = Some(format!(
                                    "{} BIOS link(s) configured for {}. Source BIOS files were not changed.",
                                    transaction.applied.len(),
                                    transaction.emulator
                                ));
                                close = true;
                            }
                            Err(error) => self.error = Some(error.to_string()),
                        }
                    }
                });
            });
        if close {
            self.apply_dialog = None;
        }
    }
}

fn applyable(plan: &BiosProjectionPlan) -> bool {
    plan.requirements
        .iter()
        .enumerate()
        .all(|(index, requirement)| {
            if requirement.method == BiosProjectionMethod::NoBiosRequired {
                return true;
            }
            matches!(
                plan.actions.get(index),
                Some(BiosProjectionAction::CreateFileLink { .. })
            ) && plan
                .matches
                .get(index)
                .and_then(|item| item.as_ref())
                .is_some()
                && requirement.target.path.is_some()
        })
        && plan
            .requirements
            .iter()
            .any(|requirement| requirement.target.path.is_some())
}

fn status_label(status: BiosProjectionStatus) -> &'static str {
    match status {
        BiosProjectionStatus::Ready => "Ready to review",
        BiosProjectionStatus::AlreadyProjected => "Already projected",
        BiosProjectionStatus::ReviewRequired => "Review required",
        BiosProjectionStatus::Blocked => "Blocked",
        BiosProjectionStatus::Missing => "Source missing",
        BiosProjectionStatus::Unsupported => "Not a file projection",
    }
}
fn method_label(method: BiosProjectionMethod) -> &'static str {
    match method {
        BiosProjectionMethod::DirectPath => "Use this BIOS directly",
        BiosProjectionMethod::SymlinkFile => "Symbolic link to immutable BIOS",
        BiosProjectionMethod::SymlinkDirectory => "Symbolic link to immutable directory",
        BiosProjectionMethod::LocalWritableCopy => "Keep writable local copy",
        BiosProjectionMethod::LocalWritableReflink => "Keep writable local reflink",
        BiosProjectionMethod::FirmwareInstallRequired => "Firmware installation required",
        BiosProjectionMethod::ExternalSystemData => "External system data / keys",
        BiosProjectionMethod::NoBiosRequired => "No conventional BIOS required",
        BiosProjectionMethod::RomsetDependency => "MAME ROM-set dependency model",
        BiosProjectionMethod::Unsupported => "Unsupported",
        BiosProjectionMethod::Unknown => "Unknown",
    }
}
fn match_label(status: bios_projection::BiosMatchStatus) -> &'static str {
    match status {
        bios_projection::BiosMatchStatus::VerifiedMatch => "verified hash match",
        bios_projection::BiosMatchStatus::FilenameOnly => "filename evidence only",
        bios_projection::BiosMatchStatus::HashMismatch => "hash mismatch",
        bios_projection::BiosMatchStatus::Ambiguous => "ambiguous candidates",
        bios_projection::BiosMatchStatus::Missing => "missing",
        bios_projection::BiosMatchStatus::Unknown => "unknown",
    }
}

fn target_state_label(state: bios_projection::BiosTargetState) -> &'static str {
    match state {
        bios_projection::BiosTargetState::NotInspected => "not inspected",
        bios_projection::BiosTargetState::Missing => "missing",
        bios_projection::BiosTargetState::ExistingRegularFile => "existing file (conflict)",
        bios_projection::BiosTargetState::ExistingCorrectLink => "already configured",
        bios_projection::BiosTargetState::ExistingWrongLink => "different link (conflict)",
        bios_projection::BiosTargetState::UnsafeSpecialFile => "unsafe target (conflict)",
        bios_projection::BiosTargetState::Unknown => "could not inspect",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_are_plain_language_and_non_mutating() {
        assert_eq!(
            status_label(BiosProjectionStatus::Unsupported),
            "Not a file projection"
        );
        assert_eq!(
            method_label(BiosProjectionMethod::LocalWritableCopy),
            "Keep writable local copy"
        );
        assert!(!method_label(BiosProjectionMethod::SymlinkFile).contains("Apply"));
        assert!(
            target_state_label(bios_projection::BiosTargetState::ExistingCorrectLink)
                .contains("configured")
        );
    }
}
