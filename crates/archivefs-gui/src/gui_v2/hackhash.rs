//! Minimal offline HackHash detailed-export source controller.

use archivefs_core::app_dirs;
use archivefs_core::identity_source::hackhash::{
    HACKHASH_PARSER_SCHEMA_VERSION, HackHashExport, HackHashStore, HackHashValidatedImport,
};
use archivefs_core::identity_source::managed_snapshot::{ActivationPreview, ManagedSourceSnapshot};
use eframe::egui;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct HackHashPageState {
    store: Option<HackHashStore>,
    selected_path: Option<PathBuf>,
    staged: Option<HackHashValidatedImport>,
    preview: Option<ActivationPreview>,
    active: Option<ManagedSourceSnapshot>,
    active_export: Option<HackHashExport>,
    error: Option<String>,
}

impl HackHashPageState {
    pub(super) fn new() -> Self {
        let mut state = Self::default();
        let Ok(data_root) = app_dirs::data_dir() else {
            return state;
        };
        let Ok(store) = HackHashStore::new(data_root.join("provider-snapshots").join("hackhash"))
        else {
            return state;
        };
        match store.active_export() {
            Ok(Some((snapshot, export, _))) => {
                state.active = Some(snapshot);
                state.active_export = Some(export);
            }
            Ok(None) => {}
            Err(error) => state.error = Some(error.to_string()),
        }
        state.store = Some(store);
        state
    }

    fn choose_and_validate(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose HackHash detailed JSON export")
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        let Some(store) = self.store.as_ref() else {
            self.error = Some("HackHash snapshot storage is unavailable.".into());
            return;
        };
        match store.import_file(&path) {
            Ok(import) => {
                self.selected_path = Some(path);
                self.staged = Some(import);
                self.preview = None;
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn review(&mut self) {
        let (Some(store), Some(staged)) = (self.store.as_ref(), self.staged.as_ref()) else {
            return;
        };
        match store.preview_activation(staged) {
            Ok(preview) => {
                self.preview = Some(preview);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn activate(&mut self) {
        let (Some(store), Some(staged)) = (self.store.as_ref(), self.staged.as_ref()) else {
            return;
        };
        let expected = self
            .active
            .as_ref()
            .map(|snapshot| snapshot.sha256.as_str());
        match store.activate_snapshot(staged, expected) {
            Ok(result) => {
                self.active = Some(result.active);
                self.active_export = Some(staged.validation.export.clone());
                self.staged = None;
                self.preview = None;
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("HackHash provider snapshot");
        ui.label("Offline supplemental evidence only. EmuWiz never logs in, uploads ROMs, or contacts HackHash here.");
        ui.label(format!("Parser schema: {HACKHASH_PARSER_SCHEMA_VERSION}"));
        if let Some(active) = &self.active {
            ui.label(format!(
                "Active snapshot: {} · imported {} · {} records",
                active.sha256,
                active.retrieved_at_unix_seconds,
                active.record_count.unwrap_or(0)
            ));
            if let Some(export) = &self.active_export {
                let platforms = export
                    .machines
                    .iter()
                    .map(|record| record.platform.as_str())
                    .collect::<std::collections::BTreeSet<_>>();
                ui.label(format!(
                    "Platform coverage: {}",
                    platforms.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
        } else {
            ui.label("No active HackHash snapshot.");
        }
        if let Some(path) = &self.selected_path {
            ui.label(format!("Selected: {}", path.display()));
        }
        if let Some(staged) = &self.staged {
            ui.separator();
            ui.strong("Validated export waiting for review");
            ui.label(format!(
                "{} records",
                staged.validation.export.machines.len()
            ));
            if !staged.validation.warnings.is_empty() {
                ui.label(format!(
                    "{} validation warning(s)",
                    staged.validation.warnings.len()
                ));
                for warning in &staged.validation.warnings {
                    ui.colored_label(ui.visuals().warn_fg_color, warning);
                }
            }
        }
        if let Some(preview) = &self.preview {
            ui.separator();
            ui.strong("Activation preview");
            ui.label(format!("{}", preview.validation_status));
            if let Some(old) = &preview.old {
                ui.label(format!("Previous snapshot retained: {}", old.sha256));
            }
        }
        if let Some(error) = &self.error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("HackHash import: {error}"),
            );
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Choose detailed JSON").clicked() {
                self.choose_and_validate();
            }
            if ui
                .add_enabled(
                    self.staged.is_some(),
                    egui::Button::new("Review validation"),
                )
                .clicked()
            {
                self.review();
            }
            if ui
                .add_enabled(
                    self.staged.is_some() && self.preview.is_some(),
                    egui::Button::new("Activate snapshot"),
                )
                .clicked()
            {
                self.activate();
            }
        });
        ui.collapsing("Evidence boundary", |ui| {
            ui.label("Hash matches are indexed as HackHash external evidence. They never become EmuWiz native Verified identity and conflicting local/No-Intro/Redump evidence is retained.");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_source_surface_is_offline_and_unactivated() {
        let state = HackHashPageState::default();
        assert!(state.active.is_none());
        assert!(state.staged.is_none());
        assert!(state.preview.is_none());
    }
}
