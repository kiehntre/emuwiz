//! Read-only BIOS / firmware projection review.

use archivefs_core::bios_projection::{
    self, BiosProjectionAction, BiosProjectionMethod, BiosProjectionResult, BiosProjectionStatus,
};
use eframe::egui;
use std::path::PathBuf;

#[derive(Default)]
pub(crate) struct BiosProjectionPageState {
    master_root: String,
    result: Option<BiosProjectionResult>,
    error: Option<String>,
}

impl BiosProjectionPageState {
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("BIOS / Firmware");
        ui.label("Inspect a master BIOS collection and review emulator-specific projection plans. Nothing is applied from this page.");
        ui.horizontal(|ui| {
            ui.label("Master BIOS root:");
            ui.add(egui::TextEdit::singleline(&mut self.master_root).desired_width(420.0));
            if ui.button("Inspect master BIOS root").clicked() {
                self.inspect();
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(180, 70, 55), error);
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
        for plan in &result.plans {
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
                for warning in &plan.warnings {
                    ui.colored_label(egui::Color32::from_rgb(180, 130, 40), warning);
                }
            });
        }
        ui.small("Projection actions are review-only. Apply, link, copy, firmware installation, and download controls are intentionally unavailable.");
    }

    fn inspect(&mut self) {
        self.error = None;
        let root = PathBuf::from(self.master_root.trim());
        match bios_projection::inspect_master_root(&root) {
            Ok(inventory) => self.result = Some(bios_projection::plan_projections(&inventory)),
            Err(error) => {
                self.result = None;
                self.error = Some(error.to_string());
            }
        }
    }
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
    }
}
