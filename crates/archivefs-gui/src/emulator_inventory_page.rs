//! Read-only installed-emulator inventory. There are intentionally no
//! lifecycle mutation controls in this page.

use archivefs_core::emulator_inventory::{self, EmulatorInventory, InstallationType};
use eframe::egui;

#[derive(Default)]
pub(crate) struct EmulatorInventoryPageState {
    pub inventory: Option<EmulatorInventory>,
    pub error: Option<String>,
}

impl EmulatorInventoryPageState {
    pub(crate) fn refresh(&mut self) {
        self.inventory = Some(emulator_inventory::discover_installed_emulators());
        self.error = None;
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("Emulator Manager");
        ui.label("Read-only inventory of emulator installations already present on this computer.");
        if ui.button("Refresh inventory").clicked() {
            self.refresh();
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
                ] {
                    ui.strong(heading);
                }
                ui.end_row();
                for install in &inventory.installations {
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
        ui.small("Installation, update, switching, rollback, and deletion actions are not available here.");
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
    #[test]
    fn page_is_read_only() {
        let labels = [
            "Refresh inventory",
            "Read-only inventory",
            "No installation actions",
        ];
        assert!(labels
            .iter()
            .all(|label| !label.contains("Install emulator")));
    }
}
