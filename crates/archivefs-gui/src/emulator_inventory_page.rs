//! Installed-emulator inventory with explicitly reviewed updates and Undo.
//! Saved records support conservative restart recovery for each installation.

use archivefs_core::emulator_download::{
    EmulatorDownloadOptions, HttpsEmulatorDownloadTransport, emulator_download_spec,
    resolve_download_plan,
};
use archivefs_core::emulator_inventory::{
    self, EmulatorInstallation, EmulatorInventory, InstallationType, InventoryEmulator,
};
use archivefs_core::emulator_update::{
    self, HttpsUpdateDownloader, OfficialMetadataProvider, QuiescenceEvidence, UpdateArtifact,
    UpdateExecutionEligibility, UpdateExecutionError, UpdateExecutionPlan, UpdateJournal,
    UpdateRecordEntry, UpdateReport, UpdateResult, UpdateStatus, actionable_undo,
    discover_update_records, execute_staged_update, plan_staged_update,
    probe_executable_quiescence, probe_update_quiescence, recover_update,
    remembered_installation_roots, rollback_staged_update,
};
use archivefs_core::managed_emulator_install::{
    ManagedInstallHealth, ManagedInstallVersionRole, ManagedOwnershipState,
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
    tracked_running: bool,
    /// Durable update records found on disk (survive a restart).
    records: Vec<UpdateRecordEntry>,
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
        let inventory = emulator_inventory::discover_installed_emulators();
        let mut remembered = match remembered_installation_roots() {
            Ok(roots) => roots,
            Err(e) => {
                self.error = Some(format!("Update recovery discovery needs review: {e}"));
                return;
            }
        };
        remembered.extend(std::env::var_os("PATH").into_iter().flat_map(|p| {
            std::env::split_paths(&p)
                .take(emulator_inventory::MAX_PATH_ENTRIES)
                .collect::<Vec<_>>()
        }));
        self.refresh_with_inventory(inventory, remembered);
    }

    fn refresh_with_inventory(
        &mut self,
        inventory: EmulatorInventory,
        roots: Vec<std::path::PathBuf>,
    ) {
        self.records = discover_records(&inventory, &roots);
        // Undo is selected explicitly for an installation, never globally by
        // whichever random transaction filename happens to sort last.
        self.review_journal = None;
        self.inventory = Some(inventory);
        self.error = None;
    }

    /// Fresh evidence: EmuWiz's own launch tracking AND a process scan for the
    /// executable.  Anything but a clear "stopped" blocks the operation.
    fn quiescence_for(&self, executable: &std::path::Path) -> QuiescenceEvidence {
        QuiescenceEvidence::from_tracked_flag(self.tracked_running)
            .combine(probe_executable_quiescence(executable))
    }

    pub(crate) fn check_for_updates(&mut self) {
        let Some(inventory) = self.inventory.clone() else {
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
        quiescence: QuiescenceEvidence,
    ) {
        let artifact = match update_artifact(&installation, &update) {
            Ok(artifact) => artifact,
            Err(error) => {
                self.error = Some(error);
                self.review = None;
                return;
            }
        };
        let plan = plan_staged_update(&installation, &update, artifact, quiescence);
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
        let tracked = self.tracked_running;
        let executable = review.plan.target_path.clone();
        // Re-evaluated at the moment of apply (and again right before the
        // executable moves), not the snapshot taken when the review opened.
        let evidence = move || {
            QuiescenceEvidence::from_tracked_flag(tracked)
                .combine(probe_executable_quiescence(&executable))
        };
        match execute_staged_update(
            &review.plan,
            &review.installation,
            &review.update,
            evidence,
            &mut downloader,
        ) {
            Ok(journal) => {
                self.feedback = Some(format!(
                    "Updated {} from {} to {}. The download matched its published SHA-256, and the previous executable is preserved at {}. Undo is available while the installed file is unchanged.",
                    journal.emulator.label(),
                    journal.old_version,
                    journal.new_version,
                    journal.rollback_path.display()
                ));
                self.review = None;
                self.confirmation.clear();
                self.refresh();
                self.update_report = None;
                self.review_journal = actionable_undo(&self.records, &journal.target_path);
            }
            Err(error) => {
                self.feedback = None;
                self.error = Some(update_error_message(&error));
                self.review = None;
                self.confirmation.clear();
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
        let tracked = self.tracked_running;
        let probed_record = journal.clone();
        let evidence = move || {
            QuiescenceEvidence::from_tracked_flag(tracked)
                .combine(emulator_update::probe_update_quiescence(&probed_record))
        };
        match rollback_staged_update(&journal, evidence) {
            Ok(restored) => {
                let kept = restored
                    .displaced_path
                    .as_ref()
                    .map(|path| format!(" The replaced executable was kept at {}.", path.display()))
                    .unwrap_or_default();
                self.feedback = Some(format!(
                    "Restored {} {}: the file now at the install path matches the recorded original SHA-256.{kept}",
                    restored.emulator.label(),
                    restored.old_version
                ));
                self.review_journal = None;
                self.rollback_confirmation.clear();
                self.refresh();
                self.update_report = None;
            }
            Err(error) => {
                self.feedback = None;
                self.error = Some(update_error_message(&error));
            }
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, emulator_running: bool) {
        self.tracked_running = emulator_running;
        ui.heading("Emulator Manager");
        ui.label("Review installed emulators and saved update records. Changes require explicit confirmation.");
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
        let Some(inventory) = self.inventory.clone() else {
            ui.label("Inventory has not been scanned yet.");
            return;
        };
        if inventory.installations.is_empty() && inventory.managed_installations.is_empty() {
            ui.label("No installed executable was detected. Saved update records remain available for review.");
            self.show_recovery(ui);
            self.show_undo_choices(ui);
            self.show_update_review(ui);
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
                            let evidence = self.quiescence_for(&install.executable_path);
                            self.review_update(install.clone(), update, evidence);
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
        if !inventory.managed_installations.is_empty() {
            ui.separator();
            ui.heading("EmuWiz-managed installations");
            ui.label("Managed versions are shown separately from external installs. No adoption or update controls are available here.");
            egui::Grid::new("managed_emulator_inventory_grid")
                .striped(true)
                .show(ui, |ui| {
                    for heading in [
                        "Emulator",
                        "Version",
                        "Channel",
                        "Status",
                        "Install root",
                        "Health",
                        "Provenance",
                    ] {
                        ui.strong(heading);
                    }
                    ui.end_row();
                    for managed in &inventory.managed_installations {
                        ui.label(&managed.emulator_id);
                        ui.label(managed.installed_version.as_deref().unwrap_or("Unknown"));
                        ui.label(managed.channel.map(channel_label).unwrap_or("Unknown"));
                        ui.label(managed_role_label(managed.role));
                        ui.label(managed.install_root.display().to_string());
                        ui.label(managed_health_label(managed.health));
                        ui.label(match managed.ownership {
                            ManagedOwnershipState::Managed => "Managed by EmuWiz",
                            ManagedOwnershipState::BrokenManagedState => {
                                "Managed record needs review"
                            }
                            _ => "Unknown ownership",
                        });
                        ui.end_row();
                    }
                });
        }
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
        self.show_undo_choices(ui);
        self.show_update_review(ui);
        self.show_recovery(ui);
    }

    fn review_undo_for(&mut self, target: &std::path::Path) {
        self.review_journal = actionable_undo(&self.records, target);
        self.rollback_confirmation.clear();
    }

    fn show_undo_choices(&mut self, ui: &mut egui::Ui) {
        let mut targets: Vec<_> = self
            .records
            .iter()
            .filter_map(|e| e.journal.as_ref().ok())
            .map(|j| j.target_path.clone())
            .collect();
        targets.sort();
        targets.dedup();
        for target in targets {
            if actionable_undo(&self.records, &target).is_some() {
                ui.horizontal(|ui| {
                    ui.label(format!("Undo for {}", target.display()));
                    if ui.button("Review Undo").clicked() {
                        self.review_undo_for(&target);
                    }
                });
            }
        }
    }

    fn show_recovery(&mut self, ui: &mut egui::Ui) {
        let attention: Vec<UpdateRecordEntry> = self
            .records
            .iter()
            .filter(|entry| entry.needs_attention())
            .cloned()
            .collect();
        if attention.is_empty() {
            return;
        }
        ui.separator();
        ui.heading("Interrupted or unreadable update records");
        ui.label("An earlier update or undo did not finish cleanly. Recovery only acts when file hashes prove the outcome; otherwise it changes nothing and reports why.");
        for entry in attention {
            match &entry.journal {
                Ok(journal) => {
                    ui.label(format!(
                        "{} {} -> {}: {:?} — {}",
                        journal.emulator.label(),
                        journal.old_version,
                        journal.new_version,
                        journal.state,
                        journal.target_path.display()
                    ));
                    if ui.button("Recover (conservative)").clicked() {
                        let tracked = self.tracked_running;
                        let j = journal.clone();
                        self.recover_record_with(&entry.path, move || {
                            QuiescenceEvidence::from_tracked_flag(tracked)
                                .combine(probe_update_quiescence(&j))
                        });
                    }
                }
                Err(problem) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 70, 55),
                        format!("Record requires review at {}: {problem}. Undo and recovery are refused; nothing was changed.", entry.path.display()),
                    );
                }
            }
        }
    }

    fn recover_record_with(
        &mut self,
        path: &std::path::Path,
        evidence: impl FnMut() -> QuiescenceEvidence,
    ) {
        self.feedback = None;
        match recover_update(path, evidence) {
            Ok(done) => {
                self.error = None;
                self.feedback = Some(format!(
                    "Recovery reconciled {}: record is now {:?}. Executable bytes were checked against the recorded hashes.",
                    done.target_path.display(),
                    done.state
                ));
                // Refresh only saved evidence here; discovery must not execute
                // version probes on bytes that were just recovered.
                if let Some(inventory) = self.inventory.clone() {
                    let roots = self
                        .records
                        .iter()
                        .filter_map(|e| {
                            e.path
                                .parent()
                                .and_then(std::path::Path::parent)
                                .map(std::path::Path::to_path_buf)
                        })
                        .collect();
                    self.refresh_with_inventory(inventory, roots);
                }
            }
            Err(e) => self.error = Some(update_error_message(&e)),
        }
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
        detail_row(
            ui,
            "Installation",
            &journal.target_path.display().to_string(),
        );
        ui.label("The previous executable is preserved and recorded by hash. Undo re-checks both the installed file and the preserved copy, and refuses if either changed.");
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
        InventoryEmulator::Mame => "mame",
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
    let state = match error {
        UpdateExecutionError::NeedsReconciliation(_) | UpdateExecutionError::Record(_) => {
            "The operation may be partly applied; see the interrupted-update records below."
        }
        _ => "No executable was replaced or removed by this attempt.",
    };
    format!("Emulator update did not complete: {error}. {state}")
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

fn managed_role_label(role: ManagedInstallVersionRole) -> &'static str {
    match role {
        ManagedInstallVersionRole::Current => "Current managed version",
        ManagedInstallVersionRole::Previous => "Previous managed version",
        ManagedInstallVersionRole::Historical => "Historical managed version",
        ManagedInstallVersionRole::BrokenReference => "Broken current reference",
        ManagedInstallVersionRole::Unknown => "Unknown managed status",
    }
}

fn managed_health_label(health: ManagedInstallHealth) -> &'static str {
    match health {
        ManagedInstallHealth::Healthy => "Healthy",
        ManagedInstallHealth::StaleManifest => "Managed install changed",
        ManagedInstallHealth::BrokenManagedState => "Broken managed state",
        ManagedInstallHealth::MissingExecutable => "Executable missing",
        ManagedInstallHealth::HashMismatch => "Executable hash mismatch",
        ManagedInstallHealth::InvalidManifest => "Invalid manifest",
        ManagedInstallHealth::OutsideManagedRoot => "Outside managed root",
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
    use archivefs_core::emulator_update::UpdateTransactionState;

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
    include!("emulator_inventory_page/recovery_tests.rs");
}

fn discover_records(
    inventory: &EmulatorInventory,
    remembered: &[std::path::PathBuf],
) -> Vec<UpdateRecordEntry> {
    let mut roots: Vec<std::path::PathBuf> = inventory
        .installations
        .iter()
        .map(|i| i.installation_root.clone())
        .chain(
            inventory
                .managed_installations
                .iter()
                .map(|i| i.install_root.clone()),
        )
        .chain(remembered.iter().cloned())
        .collect();
    roots.sort();
    roots.dedup();
    roots
        .iter()
        .flat_map(|p| discover_update_records(p))
        .collect()
}
