//! Minimal offline HackHash detailed-export source controller.

use archivefs_core::app_dirs;
use archivefs_core::identity_source::hackhash::{
    HACKHASH_PARSER_SCHEMA_VERSION, HackHashExport, HackHashFetchResult, HackHashFetchState,
    HackHashStore, HackHashValidatedImport,
};
use archivefs_core::identity_source::hackhash_identity::{
    HackHashEvidenceClass, HackHashIdentityResult, HackHashObservedHashes, match_snapshot,
};
use archivefs_core::identity_source::managed_snapshot::{
    ActivationPreview, HttpsManagedSourceTransport, ManagedSourceSnapshot, UpdateCheck,
};
use archivefs_core::identity_source::model::IdentityProvider;
use archivefs_core::identity_source::settings::default_identity_root;
use archivefs_core::identity_source::verification::VerificationStore;
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
    selected_identity: Option<HackHashIdentityResult>,
    error: Option<String>,
    remote_url: String,
    update: Option<UpdateCheck>,
    fetch_state: Option<HackHashFetchState>,
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
        state.remote_url = String::new();
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

    fn check_for_update(&mut self) {
        let url = self.remote_url.trim();
        if url.is_empty() {
            self.error = Some("Enter the URL of a HackHash detailed JSON export first.".into());
            return;
        }
        let Some(store) = self.store.as_ref() else {
            self.error = Some("HackHash snapshot storage is unavailable.".into());
            return;
        };
        match store.check_for_update(url, &HttpsManagedSourceTransport::default()) {
            Ok(update) => {
                self.update = Some(update);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn fetch_candidate(&mut self) {
        let url = self.remote_url.trim();
        if url.is_empty() {
            self.error = Some("Enter the URL of a HackHash detailed JSON export first.".into());
            return;
        }
        let Some(store) = self.store.as_ref() else {
            self.error = Some("HackHash snapshot storage is unavailable.".into());
            return;
        };
        match store.fetch_candidate(url, &HttpsManagedSourceTransport::default()) {
            Ok(HackHashFetchResult { import, state }) => {
                self.staged = Some(import);
                self.fetch_state = Some(state);
                self.preview = None;
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

    pub(super) fn inspect_selected_rom(&mut self, path: &std::path::Path) {
        self.selected_identity = None;
        let (Some(snapshot), Some(export)) = (&self.active, &self.active_export) else {
            return;
        };
        let Ok(root) = default_identity_root() else {
            return;
        };
        // This reads only an existing, fingerprint-checked explicit hash cache;
        // opening or hashing the ROM is never part of ordinary browsing.
        let cache = VerificationStore::new(&root, IdentityProvider::Romm).load();
        let Some(hashes) = cache.get(path) else {
            return;
        };
        let observed =
            HackHashObservedHashes::new(Some(&hashes.sha1), Some(&hashes.md5), Some(&hashes.crc32));
        self.selected_identity = Some(match_snapshot(
            snapshot,
            export,
            &observed,
            archivefs_core::identity_source::hackhash_identity::HackHashSnapshotState::Active,
        ));
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("HackHash provider snapshot");
        ui.label("HackHash is external community evidence. Network access occurs only after you press Check for update or Fetch candidate.");
        ui.label("EmuWiz sends only this export URL; it never uploads ROMs, sends ROM paths or local ROM hashes, logs in, or invents authentication.");
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
                "Candidate: {} · retrieved {} · {} records",
                staged.candidate.snapshot.sha256,
                staged.candidate.snapshot.retrieved_at_unix_seconds,
                staged.validation.export.machines.len()
            ));
            ui.label(
                if self.fetch_state == Some(HackHashFetchState::AlreadyCurrent) {
                    "Candidate content is already current. Activation is still explicit."
                } else {
                    "Candidate content differs from the active snapshot."
                },
            );
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
            ui.label("Detailed JSON URL:");
            ui.add(egui::TextEdit::singleline(&mut self.remote_url).desired_width(360.0));
            if ui.button("Check for update").clicked() {
                self.check_for_update();
            }
            if ui.button("Fetch candidate").clicked() {
                self.fetch_candidate();
            }
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
        if let Some(update) = &self.update {
            ui.label(match update {
                UpdateCheck::Available { .. } => {
                    "Update metadata says a candidate may be available; fetch is still explicit."
                }
                UpdateCheck::Unchanged { .. } => "Update metadata says the source is unchanged.",
                UpdateCheck::Offline { .. } => "No network check performed.",
            });
        }
        ui.collapsing("Evidence boundary", |ui| {
            ui.label("Hash matches are indexed as HackHash external evidence. They never become EmuWiz native Verified identity and conflicting local/No-Intro/Redump evidence is retained.");
        });
    }

    /// Render the selected-ROM evidence surface.  Hash acquisition remains an
    /// explicit inspection action owned by the caller; browsing a game never
    /// reads or hashes its media.
    pub(super) fn show_selected_rom_evidence(&self, ui: &mut egui::Ui) {
        if let Some(result) = &self.selected_identity {
            Self::show_identity_result(ui, result);
            return;
        }
        ui.separator();
        ui.strong("External hack evidence");
        ui.label("HackHash");
        ui.label("No locally inspected output hashes are available for this ROM.");
        ui.label("Inspect hashes explicitly to compare patched output evidence.");
        ui.label("HackHash is external community evidence and never native Verified identity.");
    }

    pub(super) fn show_identity_result(ui: &mut egui::Ui, result: &HackHashIdentityResult) {
        ui.separator();
        ui.strong("External hack evidence");
        ui.label("HackHash");
        for item in &result.matches {
            let label = match item.evidence_class {
                HackHashEvidenceClass::ExactPatchedOutput => "Exact patched-output hash match",
                HackHashEvidenceClass::ProbableHackFamily => "Probable hack-family relationship",
                HackHashEvidenceClass::BaseRomRelationship => "Known base-ROM relationship",
                HackHashEvidenceClass::KnownPatchRelationship => "Known patch relationship",
                HackHashEvidenceClass::ConflictingExternalClaims => "Conflicting external claims",
            };
            ui.label(label);
            ui.label(format!("{} v{}", item.hack_title, item.version));
            ui.label(format!(
                "Provider snapshot: {}",
                item.provider_snapshot_sha256
            ));
            ui.label(&item.provider_provenance);
        }
        for conflict in &result.conflicts {
            ui.colored_label(ui.visuals().warn_fg_color, &conflict.reason);
        }
        ui.label("HackHash is external community evidence and never native Verified identity.");
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
