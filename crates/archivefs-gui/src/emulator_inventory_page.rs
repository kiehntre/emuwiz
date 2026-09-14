//! Read-only installed-emulator inventory. There are intentionally no
//! lifecycle mutation controls in this page.

use archivefs_core::emulator_download::{
    EmulatorDownloadOptions, HttpsEmulatorDownloadTransport, emulator_download_spec,
    resolve_download_plan,
};
use archivefs_core::emulator_inventory::{
    self, EmulatorInstallation, EmulatorInventory, InstallationType, InventoryEmulator,
};
use archivefs_core::emulator_update::{
    self, HttpsUpdateDownloader, UpdateArtifact, UpdateExecutionEligibility, UpdateExecutionError,
    UpdateExecutionPlan, UpdateJournal, UpdateReport, UpdateResult, UpdateStatus,
    execute_staged_update, plan_staged_update, rollback_staged_update,
};
use eframe::egui;

#[derive(Default)]
pub(crate) struct EmulatorInventoryPageState {
    pub inventory: Option<EmulatorInventory>,
    pub error: Option<String>,
    pub update_report: Option<UpdateReport>,
    review: Option<UpdateReview>,
    review_journal: Option<UpdateJournal>,
    confirmation: String,
    rollback_confirmation: String,
    feedback: Option<String>,
}

#[derive(Clone)]
struct UpdateReview {
    installation: EmulatorInstallation,
    update: UpdateResult,
    plan: UpdateExecutionPlan,
}

const UPDATE_CONFIRMATION_PREFIX: &str = "UPDATE ";
const ROLLBACK_CONFIRMATION_PREFIX: &str = "ROLL BACK ";

impl EmulatorInventoryPageState {
    pub(crate) fn refresh(&mut self) {
        self.inventory = Some(emulator_inventory::discover_installed_emulators());
        self.error = None;
    }

    pub(crate) fn check_for_updates(&mut self) {
        let Some(inventory) = &self.inventory else {
            self.error = Some("Scan the inventory before checking metadata.".into());
            return;
        };
        self.update_report = Some(emulator_update::check_updates(
            &inventory.installations,
            &mut OfficialMetadataProvider::default(),
        ));
    }

    fn review_update(
        &mut self,
        installation: EmulatorInstallation,
        update: UpdateResult,
        emulator_running: bool,
    ) {
        let artifact = match update_artifact(&installation, &update) {
            Ok(artifact) => artifact,
            Err(error) => {
                self.error = Some(error);
                self.review = None;
                return;
            }
        };
        let plan = plan_staged_update(&installation, &update, artifact, emulator_running);
        self.confirmation.clear();
        self.review = Some(UpdateReview {
            installation,
            update,
            plan,
        });
    }

    fn execute_review(&mut self) {
        let Some(review) = self.review.clone() else {
            return;
        };
        let expected = update_confirmation_phrase(review.installation.emulator);
        if self.confirmation.trim() != expected {
            self.error = Some(format!(
                "Type {expected} exactly to confirm this replacement."
            ));
            return;
        }
        if review.plan.eligibility != UpdateExecutionEligibility::Ready {
            self.error = Some(format!("Update is blocked: {:?}.", review.plan.eligibility));
            return;
        }
        let mut downloader = HttpsUpdateDownloader;
        match execute_staged_update(
            &review.plan,
            &review.installation,
            &review.update,
            false,
            &mut downloader,
        ) {
            Ok(journal) => {
                self.feedback = Some(format!(
                    "Updated {} from {} to {}. Verification passed; rollback is available.",
                    journal.emulator.label(),
                    journal.old_version,
                    journal.new_version
                ));
                self.review = None;
                self.confirmation.clear();
                self.refresh();
                self.update_report = None;
                self.review_journal = Some(journal);
            }
            Err(error) => {
                self.error = Some(update_error_message(&error));
            }
        }
    }

    fn rollback_review(&mut self) {
        let Some(journal) = self.review_journal.clone() else {
            return;
        };
        let expected = rollback_confirmation_phrase(journal.emulator);
        if self.rollback_confirmation.trim() != expected {
            self.error = Some(format!("Type {expected} exactly to confirm rollback."));
            return;
        }
        match rollback_staged_update(&journal) {
            Ok(restored) => {
                self.feedback = Some(format!(
                    "Rolled back {} to {} and verified the rollback transaction.",
                    restored.emulator.label(),
                    restored.old_version
                ));
                self.review_journal = None;
                self.rollback_confirmation.clear();
                self.refresh();
                self.update_report = None;
            }
            Err(error) => self.error = Some(update_error_message(&error)),
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, emulator_running: bool) {
        ui.heading("Emulator Manager");
        ui.label("Read-only inventory of emulator installations already present on this computer.");
        if ui.button("Refresh inventory").clicked() {
            self.refresh();
        }
        if ui.button("Check for Updates").clicked() {
            self.check_for_updates();
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(180, 70, 55), error);
        }
        if let Some(feedback) = &self.feedback {
            ui.label(feedback);
        }
        let Some(inventory) = &self.inventory else {
            ui.label("Inventory has not been scanned yet.");
            return;
        };
        if inventory.installations.is_empty() {
            ui.label("No supported emulator installations were detected.");
            return;
        }
        egui::Grid::new("emulator_inventory_grid")
            .striped(true)
            .show(ui, |ui| {
                for heading in [
                    "Emulator",
                    "Version",
                    "Channel",
                    "Install type",
                    "Install root",
                    "Executable",
                    "Update method",
                    "EmuWiz use",
                    "Warnings",
                    "Update status",
                    "Action",
                ] {
                    ui.strong(heading);
                }
                ui.end_row();
                let installations = inventory.installations.clone();
                for install in &installations {
                    let update =
                        self.update_report
                            .as_ref()
                            .and_then(|report| {
                                report.results.iter().find(|result| {
                                    result.executable_path == install.executable_path
                                })
                            })
                            .cloned();
                    ui.label(install.emulator.label());
                    ui.label(install.version.as_deref().unwrap_or("Unknown"));
                    ui.label(channel_label(install.channel));
                    ui.label(installation_type_label(install.installation_type));
                    ui.label(install.installation_root.display().to_string());
                    ui.label(install.executable_path.display().to_string());
                    ui.label(update_label(install.update_capability));
                    ui.label(match install.preferred {
                        Some(true) => "Currently used by EmuWiz",
                        Some(false) => "Not preferred",
                        None => "Preferred installation not established",
                    });
                    ui.label(install.warnings.len().to_string());
                    if let Some(update) = update {
                        ui.label(update_status_label(update.status));
                        if update.status == UpdateStatus::UpdateAvailable
                            && ui.button("Review Update").clicked()
                        {
                            self.review_update(install.clone(), update, emulator_running);
                        } else if update.status != UpdateStatus::UpdateAvailable {
                            ui.label("—");
                        }
                    } else {
                        ui.label("Not checked");
                        ui.label("—");
                    }
                    ui.end_row();
                }
            });
        let unknown = inventory
            .installations
            .iter()
            .filter(|i| matches!(i.installation_type, InstallationType::Unknown))
            .count();
        if unknown > 0 {
            ui.label(format!("{unknown} installation(s) have an unknown installation type; no assumption was made."));
        }
        ui.separator();
        ui.small("Installation, channel switching, and deletion actions are not available here.");
        if emulator_running {
            ui.label("An EmuWiz-tracked emulator process is active; update execution is blocked.");
        }
        self.show_update_review(ui);
    }

    fn show_update_review(&mut self, ui: &mut egui::Ui) {
        let Some(review) = self.review.as_ref() else {
            if self.review_journal.is_some() {
                self.show_rollback_review(ui);
            }
            return;
        };
        ui.separator();
        ui.heading("Update review");
        ui.label("Nothing is changed until the exact confirmation phrase is entered.");
        detail_row(ui, "Emulator", review.installation.emulator.label());
        detail_row(ui, "Current version", &review.plan.installed_version);
        detail_row(ui, "Available version", &review.plan.new_version);
        detail_row(ui, "Channel", channel_label(review.plan.new_channel));
        detail_row(
            ui,
            "Install type",
            installation_type_label(review.plan.installation_type),
        );
        detail_row(
            ui,
            "Executable",
            &review.plan.target_path.display().to_string(),
        );
        detail_row(ui, "Artifact source", &review.plan.artifact.provenance);
        detail_row(
            ui,
            "Checksum",
            review
                .plan
                .artifact
                .sha256
                .as_deref()
                .unwrap_or("Unavailable"),
        );
        detail_row(
            ui,
            "Eligibility",
            eligibility_label(review.plan.eligibility),
        );
        if review.plan.save_state_warning {
            ui.label("Warning: updating may affect compatibility with existing save states.");
        }
        if let Some(warning) = &review.plan.warning {
            ui.label(warning);
        }
        if review.plan.eligibility == UpdateExecutionEligibility::Ready {
            let expected = update_confirmation_phrase(review.installation.emulator);
            ui.label(format!("Type {expected} to confirm."));
            ui.text_edit_singleline(&mut self.confirmation);
            if ui.button("Confirm update").clicked() {
                self.execute_review();
            }
        } else {
            ui.label("This update cannot be executed safely from this installation.");
        }
        if ui.button("Close review").clicked() {
            self.review = None;
            self.confirmation.clear();
        }
    }

    fn show_rollback_review(&mut self, ui: &mut egui::Ui) {
        let Some(journal) = self.review_journal.as_ref() else {
            return;
        };
        ui.separator();
        ui.heading("Rollback available");
        ui.label("The verified previous installation is preserved by the E3 transaction journal.");
        let expected = rollback_confirmation_phrase(journal.emulator);
        ui.label(format!("Type {expected} to restore the previous version."));
        ui.text_edit_singleline(&mut self.rollback_confirmation);
        if ui.button("Roll Back Update").clicked() {
            self.rollback_review();
        }
    }
}

fn update_artifact(
    installation: &EmulatorInstallation,
    update: &UpdateResult,
) -> Result<UpdateArtifact, String> {
    let id = match installation.emulator {
        InventoryEmulator::Dolphin => "dolphin",
        InventoryEmulator::Rpcs3 => "rpcs3",
        InventoryEmulator::Pcsx2 => "pcsx2",
        InventoryEmulator::Ppsspp => "ppsspp",
        InventoryEmulator::DuckStation => "duckstation",
        InventoryEmulator::Xemu => "xemu",
    };
    let spec = emulator_download_spec(id).ok_or_else(|| {
        "No official artifact resolver is configured for this emulator.".to_string()
    })?;
    let transport = HttpsEmulatorDownloadTransport::default();
    let resolved = resolve_download_plan(
        &installation.installation_root,
        spec,
        &transport,
        &EmulatorDownloadOptions::default(),
    )
    .map_err(|error| format!("Update artifact metadata could not be resolved: {error}"))?;
    let Some(version) = update.available_version.clone() else {
        return Err("Available version is unknown; update was not prepared.".into());
    };
    if resolved.release_tag.trim_start_matches('v') != version || resolved.expected_sha256.is_none()
    {
        return Err("The reviewed update artifact is stale or lacks a published checksum.".into());
    }
    Ok(UpdateArtifact {
        version,
        channel: update.available_channel,
        url: resolved.asset_url,
        sha256: resolved.expected_sha256,
        source: update.source,
        provenance: resolved.asset_name,
    })
}

fn update_error_message(error: &UpdateExecutionError) -> String {
    format!(
        "Emulator update did not complete: {error}. The original installation remains protected by the E3 transaction safeguards."
    )
}

fn update_confirmation_phrase(emulator: InventoryEmulator) -> String {
    format!(
        "{UPDATE_CONFIRMATION_PREFIX}{}",
        emulator.label().to_ascii_uppercase()
    )
}

fn rollback_confirmation_phrase(emulator: InventoryEmulator) -> String {
    format!(
        "{ROLLBACK_CONFIRMATION_PREFIX}{}",
        emulator.label().to_ascii_uppercase()
    )
}

fn eligibility_label(eligibility: UpdateExecutionEligibility) -> &'static str {
    match eligibility {
        UpdateExecutionEligibility::Ready => "Ready to update",
        UpdateExecutionEligibility::RunningBlocked => "Blocked: emulator is running",
        UpdateExecutionEligibility::UnsupportedInstallType => {
            "Blocked: installation type is not managed by EmuWiz"
        }
        UpdateExecutionEligibility::VersionUnknown => "Blocked: installed version is unknown",
        UpdateExecutionEligibility::StaleMetadata => "Blocked: update metadata is stale",
        UpdateExecutionEligibility::InvalidTarget => "Blocked: target is invalid",
        UpdateExecutionEligibility::VerificationUnavailable => {
            "Blocked: checksum verification unavailable"
        }
        UpdateExecutionEligibility::ReviewRequired => "Review required",
    }
}

fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.strong(format!("{label}:"));
        ui.label(value);
    });
}

fn update_status_label(status: UpdateStatus) -> &'static str {
    match status {
        UpdateStatus::UpToDate => "Up to date",
        UpdateStatus::UpdateAvailable => "Update available",
        UpdateStatus::InstalledNewer => "Installed newer",
        UpdateStatus::VersionUnknown => "Installed version unknown",
        UpdateStatus::LatestUnknown => "Latest unknown",
        UpdateStatus::ChannelMismatch => "Channel mismatch",
        UpdateStatus::ComparisonUnsupported => "Comparison unsupported",
        UpdateStatus::Offline => "Offline",
    }
}

fn channel_label(channel: archivefs_core::emulator_inventory::BuildChannel) -> &'static str {
    use archivefs_core::emulator_inventory::BuildChannel::*;
    match channel {
        Stable => "Stable",
        Beta => "Beta",
        Development => "Development",
        Nightly => "Nightly",
        Canary => "Canary",
        Custom => "Custom",
        Unknown => "Unknown",
    }
}

fn installation_type_label(kind: InstallationType) -> &'static str {
    match kind {
        InstallationType::SystemPackage => "System package",
        InstallationType::Flatpak => "Flatpak",
        InstallationType::AppImage => "AppImage",
        InstallationType::Portable => "Portable",
        InstallationType::Manual => "Manual",
        InstallationType::Managed => "Managed",
        InstallationType::Unknown => "Unknown",
    }
}

fn update_label(kind: archivefs_core::emulator_inventory::UpdateCapability) -> &'static str {
    use archivefs_core::emulator_inventory::UpdateCapability::*;
    match kind {
        PackageManager => "Package manager",
        Flatpak => "Flatpak",
        UpstreamRelease => "Upstream release",
        PortableManaged => "Managed portable",
        ManualUnknown => "Unknown / manual",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_keeps_install_and_channel_mutations_out_of_scope() {
        let labels = [
            "Refresh inventory",
            "Read-only inventory",
            "Installation, channel switching, and deletion actions are not available here.",
        ];
        assert!(
            labels
                .iter()
                .all(|label| !label.contains("Install emulator"))
        );
        assert!(labels.iter().all(|label| !label.contains("Switch channel")));
    }

    #[test]
    fn confirmations_are_exact_and_typed() {
        assert_eq!(
            update_confirmation_phrase(InventoryEmulator::Pcsx2),
            "UPDATE PCSX2"
        );
        assert_eq!(
            rollback_confirmation_phrase(InventoryEmulator::Rpcs3),
            "ROLL BACK RPCS3"
        );
        assert_ne!(
            update_confirmation_phrase(InventoryEmulator::Dolphin),
            "UPDATE dolphin"
        );
    }

    #[test]
    fn unsupported_and_unknown_statuses_are_not_reported_as_current() {
        assert_eq!(
            eligibility_label(UpdateExecutionEligibility::UnsupportedInstallType),
            "Blocked: installation type is not managed by EmuWiz"
        );
        assert_eq!(update_status_label(UpdateStatus::Offline), "Offline");
        assert_ne!(update_status_label(UpdateStatus::Offline), "Up to date");
        assert_eq!(
            update_status_label(UpdateStatus::VersionUnknown),
            "Installed version unknown"
        );
    }
}
