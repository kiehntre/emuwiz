//! Minimal offline HackHash detailed-export source controller.

use archivefs_core::app_dirs;
use archivefs_core::identity_source::hackhash::{
    HACKHASH_PARSER_SCHEMA_VERSION, HackHashExport, HackHashFetchResult, HackHashFetchState,
    HackHashStore, HackHashValidatedImport,
};
use archivefs_core::identity_source::hackhash_apply::{
    HackHashPatchApplyPlan, HackHashPatchApplyResult, apply_hackhash_patch,
    build_hackhash_patch_apply_plan, undo_hackhash_patch,
};
use archivefs_core::identity_source::hackhash_identity::{
    HackHashEvidenceClass, HackHashIdentityResult, HackHashObservedHashes, match_snapshot,
};
use archivefs_core::identity_source::hackhash_readiness::{
    HackHashBaseIdentityState, HackHashPatchExecutorAvailability, HackHashPatchReadinessRequest,
    HackHashPatchReadinessStatus, assess_hackhash_patch_readiness,
};
use archivefs_core::identity_source::hashing::hash_file;
use archivefs_core::identity_source::managed_snapshot::{
    ActivationPreview, HttpsManagedSourceTransport, ManagedSourceSnapshot, UpdateCheck,
};
use archivefs_core::identity_source::model::IdentityProvider;
use archivefs_core::identity_source::settings::default_identity_root;
use archivefs_core::identity_source::verification::VerificationStore;
use archivefs_core::patch_manager::{default_shared_backup_root, default_shared_history_root};
use archivefs_core::safe_read::TrustedRoots;
use archivefs_core::standalone_patch::{StandalonePatchInspection, inspect_standalone_patch};
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
    selected_base_hashes: Option<HackHashObservedHashes>,
    selected_base_path: Option<PathBuf>,
    selected_patch: Option<PathBuf>,
    selected_patch_hashes: Option<HackHashObservedHashes>,
    selected_patch_inspection: Option<StandalonePatchInspection>,
    selected_output: Option<PathBuf>,
    apply_plan: Option<HackHashPatchApplyPlan>,
    apply_result: Option<HackHashPatchApplyResult>,
    confirmation: String,
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
        self.selected_base_hashes = None;
        self.selected_base_path = Some(path.to_path_buf());
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
        self.selected_base_hashes = Some(observed.clone());
        self.selected_identity = Some(match_snapshot(
            snapshot,
            export,
            &observed,
            archivefs_core::identity_source::hackhash_identity::HackHashSnapshotState::Active,
        ));
    }

    fn choose_patch(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose local HackHash patch")
            .add_filter(
                "Patch",
                &["ips", "bps", "ups", "xdelta", "vcdiff", "ppf", "aps"],
            )
            .pick_file()
        else {
            return;
        };
        match inspect_standalone_patch(&path) {
            Ok(inspection) => {
                let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
                match hash_file(&path, &TrustedRoots::from_paths([parent]), None) {
                    Ok(hashes) => {
                        self.selected_patch = Some(path);
                        self.selected_patch_hashes = Some(HackHashObservedHashes::new(
                            Some(&hashes.sha1),
                            Some(&hashes.md5),
                            Some(&hashes.crc32),
                        ));
                        self.selected_patch_inspection = Some(inspection);
                        self.apply_plan = None;
                        self.apply_result = None;
                        self.error = None;
                    }
                    Err(error) => self.error = Some(format!("Patch hash: {error:?}")),
                }
            }
            Err(error) => self.error = Some(format!("Patch inspection: {error}")),
        }
    }

    fn choose_output(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose HackHash patched-ROM destination")
            .save_file()
        else {
            return;
        };
        self.selected_output = Some(path);
        self.apply_plan = None;
        self.apply_result = None;
    }

    fn build_apply_plan(
        &mut self,
        readiness: &archivefs_core::identity_source::hackhash_readiness::HackHashPatchReadinessEvidence,
    ) {
        let (Some(base), Some(inspection), Some(output), Some(snapshot)) = (
            self.selected_base_path.as_ref(),
            self.selected_patch_inspection.as_ref(),
            self.selected_output.as_ref(),
            self.active.as_ref(),
        ) else {
            return;
        };
        let Some(root) = output.parent() else {
            self.error = Some("The selected output has no managed parent directory.".into());
            return;
        };
        match archivefs_core::standalone_patch::build_standalone_patch_apply_plan(
            inspection, base, output, root,
        )
        .map_err(|error| error.to_string())
        .and_then(|standalone| {
            build_hackhash_patch_apply_plan(readiness, standalone, root, output, &snapshot.sha256)
                .map_err(|error| error.to_string())
        }) {
            Ok(plan) => {
                self.apply_plan = Some(plan);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("HackHash apply preview: {error}")),
        }
    }

    fn apply(&mut self) {
        let (Some(plan), Some(snapshot), Some(output)) = (
            self.apply_plan.as_ref(),
            self.active.as_ref(),
            self.selected_output.as_ref(),
        ) else {
            return;
        };
        let (Ok(history), Ok(backup)) =
            (default_shared_history_root(), default_shared_backup_root())
        else {
            self.error = Some("Shared transaction history storage is unavailable.".into());
            return;
        };
        match apply_hackhash_patch(
            plan,
            &snapshot.sha256,
            &plan.expected_output,
            &self.confirmation,
            history,
            backup,
        ) {
            Ok(result) => {
                self.apply_result = Some(result);
                self.error = None;
                let _ = output;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
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
    pub(super) fn show_selected_rom_evidence(&mut self, ui: &mut egui::Ui) {
        if let Some(result) = self.selected_identity.clone() {
            Self::show_identity_result(ui, &result);
            ui.separator();
            ui.strong("Hack readiness");
            if ui.button("Choose local patch").clicked() {
                self.choose_patch();
            }
            if let Some(path) = &self.selected_patch {
                ui.label(format!("Patch: {}", path.display()));
            } else {
                ui.label("Patch: none selected");
            }
            let mut current_readiness = None;
            if let (Some(base_hashes), Some(patch_hashes), Some(inspection), Some(export)) = (
                &self.selected_base_hashes,
                &self.selected_patch_hashes,
                &self.selected_patch_inspection,
                &self.active_export,
            ) {
                let readiness = assess_hackhash_patch_readiness(&HackHashPatchReadinessRequest {
                    base_identity_state: HackHashBaseIdentityState::Identified,
                    base_hashes: base_hashes.clone(),
                    patch_hashes: patch_hashes.clone(),
                    patch_format: inspection.format,
                    patch_inspection_state: inspection.state,
                    executor: HackHashPatchExecutorAvailability::Transactional,
                    snapshot_state: result.snapshot_state,
                    snapshot_sha256: Some(&result.snapshot_sha256),
                    provider_provenance: "active immutable HackHash snapshot",
                    export,
                    identity: &result,
                });
                current_readiness = Some(readiness.clone());
                ui.label(format!(
                    "Base ROM: {}",
                    readiness
                        .expected_base_identity
                        .as_deref()
                        .unwrap_or("not matched")
                ));
                ui.label(format!(
                    "Hack/version: {}",
                    readiness
                        .hack_titles
                        .iter()
                        .zip(&readiness.versions)
                        .map(|(title, version)| format!("{title} v{version}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                ui.label(format!(
                    "Expected output: {}",
                    readiness.expected_output_hashes.join("; ")
                ));
                ui.label(format!("Evidence: {}", readiness.provider_provenance));
                ui.label(format!("Status: {:?}", readiness.status));
                ui.label(format!("Why: {}", readiness.explanation));
                for reason in readiness
                    .refusal_reasons
                    .iter()
                    .chain(&readiness.missing_evidence)
                {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("Missing/refusal: {reason}"),
                    );
                }
            } else {
                ui.label("Status: NOT READY");
                ui.label("Why: select a valid local patch to compare its hash and format against the active evidence.");
            }
            ui.separator();
            ui.strong("Safe patch apply");
            if ui.button("Choose output destination").clicked() {
                self.choose_output();
            }
            if let Some(output) = &self.selected_output {
                ui.label(format!("Output destination: {}", output.display()));
            }
            if current_readiness.as_ref().is_some_and(|readiness| {
                readiness.status == HackHashPatchReadinessStatus::ReadyToPatch
            }) && self.selected_output.is_some()
            {
                if self.apply_plan.is_none() {
                    if ui.button("Preview Apply").clicked() {
                        if let Some(readiness) = current_readiness.as_ref() {
                            self.build_apply_plan(readiness);
                        }
                    }
                } else {
                    ui.label("Planned change: create one managed patched-ROM output; base and patch remain unchanged.");
                    ui.label("Apply is transactional, atomic, and recorded in shared history.");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.confirmation)
                            .hint_text("Type APPLY HACKHASH PATCH"),
                    );
                    if ui
                        .add_enabled(
                            self.confirmation == "APPLY HACKHASH PATCH",
                            egui::Button::new("Apply"),
                        )
                        .clicked()
                    {
                        self.apply();
                    }
                }
            } else {
                ui.label("Apply disabled: exact ReadyToPatch evidence and an explicit output destination are required.");
            }
            if let Some(result) = &self.apply_result {
                ui.label(format!("Apply result: {:?}", result.shared.journal.status));
                if result.shared.journal_path.is_some() {
                    ui.label("History entry available; the generated output can be undone.");
                    if ui.button("Undo generated output").clicked() {
                        if let (Ok(history), Ok(backup), Some(root)) = (
                            default_shared_history_root(),
                            default_shared_backup_root(),
                            self.selected_output.as_ref().and_then(|path| path.parent()),
                        ) {
                            let rollback = undo_hackhash_patch(result, root, history, backup);
                            ui.label(format!("Undo result: {:?}", rollback.status));
                        }
                    }
                }
            }
            return;
        }
        ui.separator();
        ui.strong("External hack evidence");
        ui.label("HackHash");
        ui.label("No locally inspected output hashes are available for this ROM.");
        ui.label("Inspect hashes explicitly to compare patched output evidence.");
        if ui.button("Choose local patch").clicked() {
            self.choose_patch();
        }
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
