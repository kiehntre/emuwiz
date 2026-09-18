//! BIOS / firmware projection review and explicit immutable-link setup.

use archivefs_core::bios_projection::{
    self, BiosProjectionAction, BiosProjectionMethod, BiosProjectionPlan, BiosProjectionResult,
    BiosProjectionStatus,
};
use eframe::egui;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

struct BiosApplyDialog {
    plan: BiosProjectionPlan,
    target_root: PathBuf,
    confirmation: String,
}

struct BiosRollbackDialog {
    transaction: bios_projection::BiosProjectionTransaction,
}

struct AppliedBiosSetup {
    plan: BiosProjectionPlan,
    transaction: bios_projection::BiosProjectionTransaction,
}

#[derive(Default)]
pub(crate) struct BiosProjectionPageState {
    master_root: String,
    target_root: String,
    result: Option<BiosProjectionResult>,
    error: Option<String>,
    feedback: Option<String>,
    apply_dialog: Option<BiosApplyDialog>,
    rollback_dialog: Option<BiosRollbackDialog>,
    applied_setup: Option<AppliedBiosSetup>,
    doctor_refresh_requested: bool,
}

impl BiosProjectionPageState {
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        crate::widgets::workflow_header(
            ui,
            "BIOS / Firmware",
            "Prepare firmware required by your emulators.",
        );
        crate::widgets::folder_picker(ui, "BIOS folder", &mut self.master_root);
        if crate::widgets::action_button(
            ui,
            "Inspect BIOS Folder",
            crate::widgets::ActionStyle::Primary,
            true,
        )
        .clicked()
        {
            self.inspect();
        }
        crate::widgets::folder_picker(ui, "Approved setup destination", &mut self.target_root);
        ui.small("EmuWiz creates BIOS links in this explicitly supplied folder only; it does not edit emulator settings.");
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(180, 70, 55), error);
        }
        if let Some(feedback) = &self.feedback {
            ui.colored_label(egui::Color32::from_rgb(70, 150, 85), feedback);
        }
        if let Some(applied) = &self.applied_setup {
            ui.separator();
            ui.label(format!("Last BIOS setup: {}", applied.transaction.emulator));
            if ui.button("Undo BIOS setup").clicked() {
                self.rollback_dialog = Some(BiosRollbackDialog {
                    transaction: applied.transaction.clone(),
                });
            }
            ui.small("Undo removes only links created by this setup and never removes source BIOS files.");
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
                    if self.applied_setup.is_some() {
                        ui.small("Undo the current BIOS setup before creating another one.");
                    } else if applyable(bound_plan) {
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
        self.show_rollback_dialog(ui);
    }

    pub(crate) fn take_doctor_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.doctor_refresh_requested)
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
                                self.error = None;
                                self.applied_setup = Some(AppliedBiosSetup {
                                    plan: dialog.plan.clone(),
                                    transaction: transaction.clone(),
                                });
                                self.doctor_refresh_requested = true;
                                match verify_applied_setup(&dialog.plan, &transaction) {
                                    Ok(()) => {
                                        self.feedback = Some(format!(
                                            "{} BIOS link(s) configured and verified for {}. Source BIOS files were not changed. Doctor is refreshing.",
                                            transaction.applied.len(),
                                            transaction.emulator
                                        ));
                                    }
                                    Err(reason) => {
                                        self.feedback = None;
                                        self.error = Some(format!(
                                            "Applied, verification failed: {reason}. Source BIOS files were not changed. Undo remains available."
                                        ));
                                    }
                                }
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

    fn show_rollback_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(dialog) = &mut self.rollback_dialog else {
            return;
        };
        let mut close = false;
        egui::Window::new("Undo BIOS setup")
            .collapsible(false)
            .show(ui.ctx(), |ui| {
                ui.label(format!("Emulator: {}", dialog.transaction.emulator));
                ui.label("EmuWiz will remove only the BIOS links created by the last setup.");
                ui.label("The master BIOS source will remain unchanged.");
                for item in &dialog.transaction.applied {
                    ui.label(format!("Remove: {}", item.target.display()));
                }
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if ui.button("Confirm undo").clicked() {
                        match bios_projection::rollback_plan(&dialog.transaction) {
                            Ok(()) => {
                                let verification = verify_rollback(&dialog.transaction);
                                self.doctor_refresh_requested = true;
                                match verification {
                                    Ok(()) => {
                                        self.error = None;
                                        self.applied_setup = None;
                                        self.feedback = Some(
                                            "BIOS setup was undone and verified. Doctor is refreshing; source BIOS files were not changed.".into(),
                                        );
                                    }
                                    Err(reason) => {
                                        self.feedback = Some(format!(
                                            "Undo completed, but verification failed: {reason}."
                                        ));
                                    }
                                }
                                close = true;
                            }
                            Err(error) => {
                                self.error = Some(format!(
                                    "BIOS setup was not undone: {error}. No unsafe removal was attempted."
                                ));
                            }
                        }
                    }
                });
            });
        if close {
            self.rollback_dialog = None;
        }
    }
}

fn verify_applied_setup(
    plan: &BiosProjectionPlan,
    transaction: &bios_projection::BiosProjectionTransaction,
) -> Result<(), String> {
    for item in &transaction.applied {
        let metadata = std::fs::symlink_metadata(&item.target)
            .map_err(|error| format!("{} could not be checked: {error}", item.target.display()))?;
        if !metadata.file_type().is_symlink() {
            return Err(format!(
                "{} is no longer a symbolic link",
                item.target.display()
            ));
        }
        let actual = std::fs::read_link(&item.target)
            .map_err(|error| format!("{} could not be read: {error}", item.target.display()))?;
        if actual != item.source {
            return Err(format!(
                "{} points to an unexpected source",
                item.target.display()
            ));
        }
        if !std::fs::symlink_metadata(&item.source)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false)
        {
            return Err(format!(
                "{} is no longer a regular BIOS file",
                item.source.display()
            ));
        }
    }
    if transaction.emulator != plan.emulator {
        return Err("the applied emulator did not match the reviewed plan".into());
    }
    for item in &transaction.applied {
        let Some((_, requirement)) = plan
            .requirements
            .iter()
            .enumerate()
            .find(|(_, requirement)| requirement.target.path.as_ref() == Some(&item.target))
        else {
            continue;
        };
        let expected = requirement.expected_sha256.as_ref().or_else(|| {
            plan.requirements
                .iter()
                .position(|candidate| candidate.target.path.as_ref() == Some(&item.target))
                .and_then(|index| plan.matches.get(index))
                .and_then(|match_item| match_item.as_ref())
                .and_then(|evidence| evidence.sha256.as_ref())
        });
        if let Some(expected) = expected {
            let mut file = File::open(&item.source).map_err(|error| {
                format!("{} could not be opened: {error}", item.source.display())
            })?;
            let mut hasher = Sha256::new();
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer).map_err(|error| {
                    format!("{} could not be hashed: {error}", item.source.display())
                })?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            let actual = hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if &actual != expected {
                return Err(format!(
                    "{} no longer matches its expected BIOS identity",
                    item.source.display()
                ));
            }
        }
    }
    Ok(())
}

fn verify_rollback(transaction: &bios_projection::BiosProjectionTransaction) -> Result<(), String> {
    for item in &transaction.applied {
        if std::fs::symlink_metadata(&item.target).is_ok() {
            return Err(format!("{} still exists", item.target.display()));
        }
    }
    Ok(())
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
    use archivefs_core::bios_projection::{
        BiosContentClass, BiosProjectionTarget, BiosRequirement, BiosTargetState,
    };
    use std::fs;
    use tempfile::tempdir;
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

    #[cfg(unix)]
    #[test]
    fn applied_setup_verification_rejects_external_target_replacement() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("bios.bin");
        let alternate = directory.path().join("other.bin");
        let target = directory.path().join("target.bin");
        fs::write(&source, b"bios").unwrap();
        fs::write(&alternate, b"other").unwrap();
        std::os::unix::fs::symlink(&source, &target).unwrap();
        let plan = BiosProjectionPlan {
            master_root: directory.path().to_path_buf(),
            emulator: "Test Emulator".into(),
            requirements: vec![BiosRequirement {
                name: "Test BIOS".into(),
                emulator: "Test Emulator".into(),
                expected_filenames: vec!["bios.bin".into()],
                expected_sha256: None,
                target: BiosProjectionTarget {
                    description: "target".into(),
                    path: Some(target.clone()),
                    current_state: BiosTargetState::ExistingCorrectLink,
                },
                content_class: BiosContentClass::ImmutableFirmware,
                method: BiosProjectionMethod::SymlinkFile,
            }],
            matches: vec![None],
            actions: vec![],
            status: BiosProjectionStatus::AlreadyProjected,
            warnings: vec![],
        };
        let transaction = bios_projection::BiosProjectionTransaction {
            journal_id: "test".into(),
            emulator: "Test Emulator".into(),
            requirement_ids: vec!["Test BIOS".into()],
            applied: vec![bios_projection::BiosAppliedItem {
                source: source.clone(),
                target: target.clone(),
                method: BiosProjectionMethod::SymlinkFile,
                pre_state: BiosTargetState::Missing,
                post_state: BiosTargetState::ExistingCorrectLink,
            }],
            already_correct: vec![],
        };
        fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(&alternate, &target).unwrap();
        assert!(verify_applied_setup(&plan, &transaction).is_err());
    }

    #[test]
    fn rollback_verification_requires_created_targets_to_be_gone() {
        let transaction = bios_projection::BiosProjectionTransaction {
            journal_id: "test".into(),
            emulator: "Test Emulator".into(),
            requirement_ids: vec![],
            applied: vec![bios_projection::BiosAppliedItem {
                source: PathBuf::from("/source/bios.bin"),
                target: PathBuf::from("/target/bios.bin"),
                method: BiosProjectionMethod::SymlinkFile,
                pre_state: BiosTargetState::Missing,
                post_state: BiosTargetState::ExistingCorrectLink,
            }],
            already_correct: vec![],
        };
        assert!(verify_rollback(&transaction).is_ok());
    }
}
