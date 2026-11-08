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
    undo_status: Option<archivefs_core::patch_manager::SharedApplyStatus>,
    confirmation: String,
    error: Option<String>,
    remote_url: String,
    /// HackHash exports already sitting in the Downloads folder, found by
    /// name only (nothing is read or imported until the user picks one).
    downloaded_exports: Vec<PathBuf>,
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
        state.downloaded_exports = default_downloads_dir()
            .map(|dir| find_hackhash_exports_in(&dir))
            .unwrap_or_default();
        state
    }

    fn choose_and_validate(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose the HackHash file you downloaded")
            .add_filter("HackHash export", &["json"])
            .pick_file()
        else {
            return;
        };
        self.import_file_from(path);
    }

    fn refresh_downloaded_exports(&mut self) {
        self.downloaded_exports = default_downloads_dir()
            .map(|dir| find_hackhash_exports_in(&dir))
            .unwrap_or_default();
    }

    /// Validates a chosen or found export into the review stage. Nothing is
    /// used until the user reviews it and confirms.
    fn import_file_from(&mut self, path: PathBuf) {
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
                        self.undo_status = None;
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
        self.undo_status = None;
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
                self.undo_status = None;
                self.error = None;
                let _ = output;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("ROM hack identification (HackHash)");
        ui.label("Optional. HackHash is a community list of known ROM hacks that helps EmuWiz recognise hacked games. Your library, scanning and Play all work without it, so you can skip this.");
        ui.label("EmuWiz never uploads your games, file names or file fingerprints.");
        // Status, in plain words.
        if let Some(active) = &self.active {
            let hacks = active.record_count.unwrap_or(0);
            let platforms = self.active_export.as_ref().map(|export| {
                export
                    .machines
                    .iter()
                    .map(|record| record.platform.as_str())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            });
            ui.label(match platforms {
                Some(platforms) => {
                    format!("Installed list: {hacks} known hacks across {platforms} platform(s).")
                }
                None => format!("Installed list: {hacks} known hacks."),
            });
        } else {
            ui.label("No HackHash list added yet. That is fine: hacked games are simply not recognised until you add one.");
            ui.label("To add it: download the HackHash \"detailed\" export in your web browser, then press \"Add HackHash list\" below (EmuWiz shows you what it contains before anything is used).");
        }
        // One-click shortcut for a file that is already in Downloads.
        let mut chosen_download = None;
        for path in &self.downloaded_exports {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            if ui
                .button(format!("Use {name} from your Downloads folder"))
                .on_hover_text("Checks the file first and shows you what it contains. Nothing is used until you confirm.")
                .clicked()
            {
                chosen_download = Some(path.clone());
            }
        }
        if ui
            .small_button("Look in Downloads again")
            .on_hover_text("Checks your Downloads folder for a HackHash file.")
            .clicked()
        {
            self.refresh_downloaded_exports();
        }
        if let Some(path) = chosen_download {
            if path.is_file() {
                self.import_file_from(path);
            } else {
                // The file went away since it was listed: say so, and relist.
                self.error = Some("That file is no longer in your Downloads folder.".into());
                self.refresh_downloaded_exports();
            }
        }
        if let Some(path) = &self.selected_path {
            ui.label(format!("Chosen file: {}", path.display()));
        }
        // The web address is an advanced option; most people just add the file.
        if !self.remote_url.trim().is_empty() {
            ui.label(format!("Downloads from: {}", self.remote_url.trim()));
        }
        if let Some(staged) = &self.staged {
            ui.separator();
            ui.strong("A downloaded list is waiting for your review");
            ui.label(format!(
                "It contains {} known hacks.",
                staged.validation.export.machines.len()
            ));
            ui.label(
                if self.fetch_state == Some(HackHashFetchState::AlreadyCurrent) {
                    "It is the same as your installed list. Nothing changes unless you choose to use it."
                } else {
                    "It differs from your installed list. Nothing changes unless you choose to use it."
                },
            );
            if !staged.validation.warnings.is_empty() {
                ui.label(format!(
                    "{} thing(s) to be aware of:",
                    staged.validation.warnings.len()
                ));
                for warning in &staged.validation.warnings {
                    ui.colored_label(ui.visuals().warn_fg_color, warning);
                }
            }
        }
        if let Some(preview) = &self.preview {
            ui.separator();
            ui.strong("Ready to use");
            ui.label(format!("{}", preview.validation_status));
            if preview.old.is_some() {
                ui.label("Your previous list is kept, so this can be undone.");
            }
        }
        if let Some(error) = &self.error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("HackHash list: {error}"),
            );
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Add HackHash list…")
                .on_hover_text("Pick the HackHash file you downloaded. EmuWiz checks it and shows what it contains first.")
                .clicked()
            {
                self.choose_and_validate();
            }
            // Web-address features only appear once an address is set under
            // Advanced details, so the normal path never asks for a URL.
            let has_address = !self.remote_url.trim().is_empty();
            if has_address
                && ui
                    .button("Check for a newer list")
                    .on_hover_text("Asks the download address whether a newer list exists. Nothing is downloaded.")
                    .clicked()
            {
                self.check_for_update();
            }
            if has_address
                && ui
                    .button("Download list")
                    .on_hover_text("Downloads the list from the address shown above. It is not used until you review it and confirm.")
                    .clicked()
            {
                self.fetch_candidate();
            }
            if ui
                .add_enabled(self.staged.is_some(), egui::Button::new("Review downloaded list"))
                .clicked()
            {
                self.review();
            }
            if ui
                .add_enabled(
                    self.staged.is_some() && self.preview.is_some(),
                    egui::Button::new("Use this list"),
                )
                .clicked()
            {
                self.activate();
            }
        });
        if let Some(update) = &self.update {
            ui.label(match update {
                UpdateCheck::Available { .. } => {
                    "A newer list may be available. Downloading it is still your choice."
                }
                UpdateCheck::Unchanged { .. } => "Your list is up to date.",
                UpdateCheck::Offline { .. } => "No network check was made.",
            });
        }
        ui.collapsing("Advanced details", |ui| {
            ui.label(format!("Parser schema: {HACKHASH_PARSER_SCHEMA_VERSION}"));
            ui.horizontal_wrapped(|ui| {
                ui.label("Optional web address to download the list from automatically:");
                ui.add(egui::TextEdit::singleline(&mut self.remote_url).desired_width(360.0));
            });
            if let Some(active) = &self.active {
                ui.label(format!(
                    "Installed list: SHA-256 {} · imported {} · {} records",
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
            }
            if let Some(staged) = &self.staged {
                ui.label(format!(
                    "Waiting candidate: SHA-256 {} · retrieved {} · {} records",
                    staged.candidate.snapshot.sha256,
                    staged.candidate.snapshot.retrieved_at_unix_seconds,
                    staged.validation.export.machines.len()
                ));
            }
            if let Some(preview) = &self.preview
                && let Some(old) = &preview.old
            {
                ui.label(format!("Previous snapshot retained: {}", old.sha256));
            }
            ui.label("Hash matches are indexed as HackHash external evidence. They never become EmuWiz native Verified identity and conflicting local/No-Intro/Redump evidence is retained.");
        });
    }

    /// Render the selected-ROM evidence surface.  Hash acquisition remains an
    /// explicit inspection action owned by the caller; browsing a game never
    /// reads or hashes its media.
    pub(super) fn has_selected_evidence(&self) -> bool {
        self.selected_identity.is_some()
    }

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
            let mut rollback_status = None;
            if let Some(result) = &self.apply_result {
                show_apply_status(ui, &result.shared);
                ui.separator();
                ui.strong("Patch result provenance");
                ui.label(format!(
                    "Based on: {}",
                    result.provenance.base_path.display()
                ));
                ui.label(format!("Base hash: {}", result.provenance.base_sha256));
                ui.label(format!("Patch: {}", result.provenance.patch_path.display()));
                ui.label(format!(
                    "Patch identity: {} · {:?}",
                    result.provenance.patch_sha256, result.provenance.patch_format
                ));
                ui.label(format!(
                    "Result: {}",
                    result.provenance.output_path.display()
                ));
                ui.label(format!("Output hash: {}", result.provenance.output_sha256));
                ui.label(format!(
                    "Hack/version: {}",
                    if result.provenance.family_versions.is_empty() {
                        result.provenance.hack_titles.join(", ")
                    } else {
                        result.provenance.family_versions.join(", ")
                    }
                ));
                ui.label(format!(
                    "Provider snapshot: {}",
                    result.provenance.provider_snapshot_sha256
                ));
                ui.label(format!(
                    "Output verified against expected provider hash: {}",
                    result.provenance.actual_output == result.provenance.expected_output
                ));
                ui.label(format!(
                    "Native identity result: {:?} · {} · {}",
                    result.provenance.verification_state,
                    result.provenance.native_inspection.platform,
                    result.provenance.native_inspection.format
                ));
                for warning in &result.provenance.native_inspection.warnings {
                    ui.colored_label(ui.visuals().warn_fg_color, warning);
                }
                if result.shared.journal_path.is_some() {
                    ui.label(format!(
                        "History/undo: transaction {}",
                        result.provenance.transaction_id
                    ));
                    if ui.button("Undo generated output").clicked() {
                        if let (Ok(history), Ok(backup), Some(root)) = (
                            default_shared_history_root(),
                            default_shared_backup_root(),
                            self.selected_output.as_ref().and_then(|path| path.parent()),
                        ) {
                            let rollback = undo_hackhash_patch(result, root, history, backup);
                            rollback_status = Some(rollback.status);
                        }
                    }
                }
                if let Some(status) = self.undo_status {
                    ui.label(format!("Undo result: {status:?}"));
                    if status == archivefs_core::patch_manager::SharedApplyStatus::Success {
                        ui.label("Generated output removed; the provenance remains attached to the rolled-back history entry.");
                    }
                }
            }
            if rollback_status.is_some() {
                self.undo_status = rollback_status;
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

fn show_apply_status(ui: &mut egui::Ui, result: &archivefs_core::patch_manager::SharedApplyResult) {
    ui.label(format!("Apply result: {:?}", result.journal.status));
    crate::shared_apply_warnings::show(ui, &crate::shared_apply_warnings::lines(result));
}

/// The user's Downloads folder: the environment override, then the desktop's
/// own `~/.config/user-dirs.dirs` (where a localized or relocated folder is
/// actually recorded), then `~/Downloads`.
fn default_downloads_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    if let Some(dir) = std::env::var_os("XDG_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(dir);
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|text| downloads_dir_from_user_dirs(&text, &home))
        .or_else(|| Some(home.join("Downloads")))
}

/// Parses `XDG_DOWNLOAD_DIR="$HOME/Downloads"` from `user-dirs.dirs` text.
fn downloads_dir_from_user_dirs(text: &str, home: &std::path::Path) -> Option<PathBuf> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("XDG_DOWNLOAD_DIR="))?;
    let value = line.split_once('=')?.1.trim().trim_matches('"');
    let resolved = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    resolved.is_absolute().then_some(resolved)
}

/// Lists likely HackHash exports directly inside `dir` (no recursion, regular
/// non-symlink `.json` files with "hackhash" in the name, newest first, at
/// most three). Reads directory entries only; no file is opened.
fn find_hackhash_exports_in(dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .take(20_000)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            (metadata.file_type().is_file()
                && name.ends_with(".json")
                && name.contains("hackhash")
                && metadata.len() <= 512 * 1024 * 1024)
                .then(|| (metadata.modified().unwrap_or(std::time::UNIX_EPOCH), path))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, path)| path).take(3).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_result_shows_shared_transaction_warnings() {
        use crate::shared_apply_warnings::test_support::{
            assert_shows_warning, render, warned_result,
        };
        let result = warned_result();
        assert_shows_warning(&render(|ui| show_apply_status(ui, &result)));
    }

    #[test]
    fn initial_source_surface_is_offline_and_unactivated() {
        let state = HackHashPageState::default();
        assert!(state.active.is_none());
        assert!(state.staged.is_none());
        assert!(state.preview.is_none());
    }

    #[test]
    fn downloads_finder_offers_only_plain_hackhash_json_files() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str| std::fs::write(dir.path().join(name), b"{}").unwrap();
        write("HackHash-detailed-2026.JSON");
        write("hackhash_export.json");
        write("holiday-photos.json");
        write("hackhash-notes.txt");
        std::fs::create_dir(dir.path().join("hackhash-folder.json")).unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("holiday-photos.json"),
            dir.path().join("hackhash-link.json"),
        )
        .unwrap();
        let found = find_hackhash_exports_in(dir.path());
        let names: std::collections::BTreeSet<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["HackHash-detailed-2026.JSON", "hackhash_export.json"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        assert!(find_hackhash_exports_in(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn the_first_view_never_asks_for_an_address() {
        let state = HackHashPageState::default();
        assert!(state.remote_url.trim().is_empty());
        assert!(state.downloaded_exports.is_empty());
    }

    #[test]
    fn user_dirs_file_locates_a_relocated_downloads_folder() {
        let home = std::path::Path::new("/home/someone");
        assert_eq!(
            downloads_dir_from_user_dirs(
                "XDG_DOCUMENTS_DIR=\"$HOME/Docs\"\nXDG_DOWNLOAD_DIR=\"$HOME/Descargas\"\n",
                home
            ),
            Some(PathBuf::from("/home/someone/Descargas"))
        );
        assert_eq!(
            downloads_dir_from_user_dirs("XDG_DOWNLOAD_DIR=\"/mnt/big/dl\"\n", home),
            Some(PathBuf::from("/mnt/big/dl"))
        );
        assert_eq!(
            downloads_dir_from_user_dirs("XDG_DOWNLOAD_DIR=\"relative\"\n", home),
            None
        );
        assert_eq!(downloads_dir_from_user_dirs("nothing here", home), None);
    }
}
