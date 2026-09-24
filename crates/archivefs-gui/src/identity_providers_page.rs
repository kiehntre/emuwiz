//! Thin GUI controller and presentation for the official MAME and ScummVM
//! identity providers. Provider parsing, validation, and snapshot storage stay
//! in archivefs-core; this module only wires explicit user actions to those
//! APIs and presents their state in plain language.

use archivefs_core::app_dirs;
use archivefs_core::identity_source::managed_snapshot::{
    ActivationPreview, ManagedSourceSnapshot, ValidatedCandidate, VerificationFreshness,
};
use archivefs_core::identity_source::model::IdentityProvider;
use archivefs_core::identity_source::providers::{
    DetectionClass, ManagedProviderStore, ProviderIdentityResult, ProviderSnapshot, check_provider,
    verify,
};
use eframe::egui;
use std::path::{Path, PathBuf};

use crate::ui::components as widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderAction {
    ChooseTool,
    CheckForUpdate,
    ReviewStaged,
    Activate,
    Verify,
}

pub(crate) struct IdentityProvidersPageState {
    mame: ProviderCardState,
    scummvm: ProviderCardState,
}

impl Default for IdentityProvidersPageState {
    fn default() -> Self {
        Self::new()
    }
}

struct ProviderCardState {
    provider: IdentityProvider,
    executable: Option<PathBuf>,
    store: Option<ManagedProviderStore>,
    active: Option<ManagedSourceSnapshot>,
    staged: Option<ValidatedCandidate>,
    preview: Option<ActivationPreview>,
    result: Option<ProviderIdentityResult>,
    error: Option<String>,
    needs_recheck: bool,
}

impl Default for ProviderCardState {
    fn default() -> Self {
        Self {
            provider: IdentityProvider::Mame,
            executable: None,
            store: None,
            active: None,
            staged: None,
            preview: None,
            result: None,
            error: None,
            needs_recheck: false,
        }
    }
}

impl IdentityProvidersPageState {
    pub(crate) fn new() -> Self {
        Self {
            mame: ProviderCardState::for_provider(IdentityProvider::Mame),
            scummvm: ProviderCardState::for_provider(IdentityProvider::ScummVm),
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        widgets::section_header(
            ui,
            "Identity Providers",
            Some("Official local MAME and ScummVM evidence used to establish game identity."),
        );
        ui.label("Provider updates are staged and verified before activation. Activating a newer snapshot can mark existing verification as needing re-check.");
        ui.add_space(8.0);
        ui.columns(2, |columns| {
            show_provider_card(&mut columns[0], &mut self.mame);
            show_provider_card(&mut columns[1], &mut self.scummvm);
        });
    }

    #[cfg(test)]
    pub(crate) fn card_for_test(provider: IdentityProvider) -> ProviderCardView {
        ProviderCardView::for_provider(provider)
    }
}

impl ProviderCardState {
    fn for_provider(provider: IdentityProvider) -> Self {
        Self {
            provider,
            ..Self::default()
        }
    }

    fn choose_tool(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title(format!(
                "Choose installed {} program (not verification data)",
                self.provider.label()
            ))
            .pick_file()
        else {
            return;
        };
        self.configure(path);
    }

    fn configure(&mut self, executable: PathBuf) {
        if let Err(error) = validate_program(&executable, self.provider) {
            self.error = Some(error);
            return;
        }
        self.error = None;
        self.active = None;
        self.staged = None;
        self.preview = None;
        self.result = None;
        self.needs_recheck = false;
        match app_dirs::data_dir() {
            Ok(root) => match ManagedProviderStore::new(
                root.join("identity-providers").join(self.provider.slug()),
                self.provider,
                &executable,
            ) {
                Ok(store) => {
                    match store.active_managed_snapshot() {
                        Ok(active) => self.active = active,
                        Err(error) => self.error = Some(error),
                    }
                    self.store = Some(store);
                    self.executable = Some(executable);
                }
                Err(error) => self.error = Some(error),
            },
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn apply(&mut self, action: ProviderAction) {
        match action {
            ProviderAction::ChooseTool => self.choose_tool(),
            ProviderAction::CheckForUpdate => self.check_for_update(),
            ProviderAction::ReviewStaged => self.review_staged(),
            ProviderAction::Activate => self.activate(),
            ProviderAction::Verify => self.verify_game(),
        }
    }

    fn check_for_update(&mut self) {
        let (Some(executable), Some(store)) = (self.executable.clone(), self.store.as_ref()) else {
            self.error = Some("Choose the installed official tool first.".into());
            return;
        };
        if let Err(error) = validate_program(&executable, self.provider) {
            self.error = Some(error);
            return;
        }
        match check_provider(self.provider, &executable)
            .and_then(|snapshot| store.stage_snapshot(&snapshot))
        {
            Ok(candidate) => {
                self.preview = None;
                self.staged = Some(candidate);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn review_staged(&mut self) {
        let (Some(store), Some(candidate)) = (self.store.as_ref(), self.staged.as_ref()) else {
            return;
        };
        match store.preview_activation(candidate) {
            Ok(preview) => {
                self.preview = Some(preview);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn activate(&mut self) {
        let (Some(store), Some(candidate)) = (self.store.as_ref(), self.staged.as_ref()) else {
            return;
        };
        let expected = self
            .active
            .as_ref()
            .map(|snapshot| snapshot.sha256.as_str());
        match store.activate_snapshot(candidate, expected) {
            Ok(result) => {
                self.active = Some(result.active);
                self.staged = None;
                self.preview = None;
                self.result = None;
                self.needs_recheck =
                    matches!(result.change.freshness, VerificationFreshness::NeedsRecheck);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn verify_game(&mut self) {
        let Some(snapshot) = self.active_snapshot() else {
            self.error =
                Some("Activate an official provider snapshot before verifying a game.".into());
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose a game or archive to verify")
            .pick_file()
        else {
            return;
        };
        match verify(&snapshot, &path) {
            Ok(result) => {
                self.result = Some(result);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn active_snapshot(&self) -> Option<ProviderSnapshot> {
        self.store.as_ref()?.active_snapshot().ok().flatten()
    }

    fn status_label(&self) -> (&'static str, widgets::StatusTone) {
        if self.error.is_some() {
            return (
                "Provider data needs attention",
                widgets::StatusTone::Blocked,
            );
        }
        if self.staged.is_some() {
            return (
                "New data staged — activation required",
                widgets::StatusTone::Pending,
            );
        }
        if self.needs_recheck {
            return (
                "Existing verification needs re-check",
                widgets::StatusTone::Warning,
            );
        }
        if let Some(result) = &self.result {
            return match result.status_against(self.active.as_ref().map(|s| s.sha256.as_str())) {
                archivefs_core::identity_source::providers::MatchStatus::NeedsRecheck => {
                    ("Needs re-check", widgets::StatusTone::Warning)
                }
                _ => ("Game verification available", widgets::StatusTone::Success),
            };
        }
        if self.active.is_some() {
            return ("Active snapshot", widgets::StatusTone::Success);
        }
        if self.executable.is_some() {
            return ("No active snapshot", widgets::StatusTone::Info);
        }
        ("Not configured", widgets::StatusTone::Info)
    }
}

fn show_provider_card(ui: &mut egui::Ui, state: &mut ProviderCardState) {
    let (status, tone) = state.status_label();
    widgets::card(ui, |ui| {
        ui.heading(state.provider.label());
        widgets::status_badge(ui, status, tone);
        ui.add_space(4.0);
        ui.label(match state.provider {
            IdentityProvider::Mame => "Official MAME machine and software-list evidence supports arcade identity.",
            IdentityProvider::ScummVm => "Official ScummVM detection supports game identity, but coverage is not complete for every engine or release.",
            IdentityProvider::Romm => "Unsupported provider.",
        });

        if let Some(active) = &state.active {
            ui.label(format!(
                "Active snapshot: {}",
                active
                    .provider_version
                    .as_deref()
                    .unwrap_or("version not reported")
            ));
            ui.label(format!("{} records", active.record_count.unwrap_or(0)));
            if matches!(state.provider, IdentityProvider::ScummVm) {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Some recognised games may remain Coverage gap.",
                );
            }
        } else {
            ui.label("No active provider snapshot is available yet.");
        }
        if state.staged.is_some() {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "A validated snapshot is waiting for your explicit activation.",
            );
        }
        if let Some(result) = &state.result {
            show_identity_result(ui, result);
        }
        if let Some(error) = &state.error {
            widgets::banner(
                ui,
                "Provider action needs attention",
                &plain_error(error),
                widgets::StatusTone::Blocked,
            );
        }

        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                if state.provider == IdentityProvider::Mame {
                    "Choose installed MAME"
                } else {
                    "Choose installed ScummVM"
                },
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                state.apply(ProviderAction::ChooseTool);
            }
            if widgets::action_button(
                ui,
                "Check for update",
                widgets::ActionStyle::Primary,
                state.executable.is_some(),
            )
            .clicked()
            {
                state.apply(ProviderAction::CheckForUpdate);
            }
            if widgets::action_button(
                ui,
                "Review staged data",
                widgets::ActionStyle::Secondary,
                state.staged.is_some(),
            )
            .clicked()
            {
                state.apply(ProviderAction::ReviewStaged);
            }
            if widgets::action_button(
                ui,
                "Activate staged snapshot",
                widgets::ActionStyle::Primary,
                state.staged.is_some() && state.preview.is_some(),
            )
            .clicked()
            {
                state.apply(ProviderAction::Activate);
            }
            if widgets::action_button(
                ui,
                if state.provider == IdentityProvider::Mame {
                    "Verify Arcade"
                } else {
                    "Verify Games"
                },
                widgets::ActionStyle::Secondary,
                state.active.is_some(),
            )
            .clicked()
            {
                state.apply(ProviderAction::Verify);
            }
        });
        if let Some(preview) = &state.preview {
            ui.label(format!("Review: {}", preview.validation_status));
            for warning in &preview.warnings {
                ui.colored_label(ui.visuals().warn_fg_color, warning);
            }
        }
        widgets::technical_details(
            ui,
            format!("identity-provider-{}", state.provider.slug()),
            |ui| {
                if let Some(error) = &state.error {
                    ui.label(error);
                }
                if let Some(path) = &state.executable {
                    ui.label(format!("Tool: {}", path.display()));
                }
                if let Some(active) = &state.active {
                    ui.label(format!("Snapshot hash: {}", active.sha256));
                    ui.label(format!("Source: {:?}", active.source));
                    ui.label(format!("Parser schema: {}", active.parser_schema_version));
                }
                if let Some(result) = &state.result {
                    ui.label(format!(
                        "Detector: {}",
                        result.detector_method.as_deref().unwrap_or("not reported")
                    ));
                    if let Some(id) = &result.official_game_id {
                        ui.label(format!("Official game ID: {id}"));
                    }
                    if let Some(target) = &result.configured_target_id {
                        ui.label(format!("Configured target ID: {target}"));
                    }
                    for detail in &result.details {
                        ui.label(detail);
                    }
                }
            },
        );
    });
}

fn show_identity_result(ui: &mut egui::Ui, result: &ProviderIdentityResult) {
    let (label, explanation) = confidence_copy(result.detection_class);
    ui.strong(format!("Identity confidence: {label}"));
    ui.label(explanation);
    if result.detection_class == DetectionClass::OfficialDetectionCoverageGap {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "This remains a Coverage gap; it is not promoted to Exact.",
        );
    }
}

fn confidence_copy(class: DetectionClass) -> (&'static str, &'static str) {
    match class {
        DetectionClass::OfficialExact => ("Exact", "Official evidence proves the release."),
        DetectionClass::OfficialFallback => (
            "Fallback",
            "The official detector recognised the game through fallback detection.",
        ),
        DetectionClass::OfficialDetectionCoverageGap => (
            "Coverage gap",
            "Official software recognises it, but the available reference data cannot prove the exact release.",
        ),
        DetectionClass::EmuwizDerivedProbable => (
            "Probable",
            "EmuWiz has strong supporting evidence but not exact official proof.",
        ),
        DetectionClass::Unknown => ("Unknown", "There is not enough evidence yet."),
    }
}

fn plain_error(error: &str) -> String {
    if error.starts_with("That file") || error.starts_with("Choose an installed") {
        error.to_string()
    } else if error.contains("integrity") {
        "The active provider data failed its integrity check. EmuWiz will not use it until it is repaired or another snapshot is activated.".into()
    } else {
        "EmuWiz could not read data from the installed program. This does not stop checks using imported verification data. Choose the installed program again, or return to Check My Games and import verification data. The original error is in Technical details.".into()
    }
}

/// Validate before configuring *and* before executing, including executable
/// bits on Unix. Selecting a data file must never reach process spawning.
pub(crate) fn validate_program(path: &Path, provider: IdentityProvider) -> Result<(), String> {
    if provider == IdentityProvider::Mame && is_verification_file(path) {
        return Err(WRONG_MAME_FILE.into());
    }
    if crate::emulator_setup_overrides::classify_picked_executable(path)
        != crate::emulator_setup_overrides::PickedExecutable::Usable
    {
        return Err(format!(
            "Choose an installed {} program that you have permission to run. This file cannot be started. Your imported verification data is safe and can still be used in Check My Games.",
            provider.label()
        ));
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err("Choose an installed program (.exe). Your imported verification data is safe; return to Check My Games to use it.".into());
    }
    Ok(())
}

pub(crate) const WRONG_MAME_FILE: &str = "That file is verification data, not the MAME program. EmuWiz has not tried to run it and your imported data is safe. Choose installed MAME, or return to Check My Games → Arcade → Change setup → Import verification data. MAME is optional when using imported data.";

pub(crate) fn is_verification_file(path: &Path) -> bool {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("dat") || ext.eq_ignore_ascii_case("xml"))
    {
        return true;
    }
    if !std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    // Also refuse renamed XML; the check is bounded and never executes bytes.
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut prefix = [0; 512];
    let Ok(read) = file.read(&mut prefix) else {
        return false;
    };
    let prefix = String::from_utf8_lossy(&prefix[..read]);
    let prefix = prefix.trim_start_matches('\u{feff}').trim_start();
    prefix.starts_with('<') || prefix.starts_with("clrmamepro")
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderCardView {
    pub(crate) title: &'static str,
    pub(crate) states: Vec<&'static str>,
    pub(crate) actions: Vec<&'static str>,
    pub(crate) advanced_details_collapsed: bool,
}

#[cfg(test)]
impl ProviderCardView {
    fn for_provider(provider: IdentityProvider) -> Self {
        Self {
            title: provider.label(),
            states: vec!["Exact", "Fallback", "Coverage gap", "Probable", "Unknown"],
            actions: vec![
                "Check for update",
                "Review staged data",
                "Activate staged snapshot",
            ],
            advanced_details_collapsed: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::identity_source::managed_snapshot::{
        ManagedSourceReference, ManagedSourceTrust,
    };

    #[test]
    fn mame_verification_file_is_rejected_before_configuration_or_execution() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["MAME.0.174.Arcade.XML.dat", "mame.XML", "mame"] {
            let path = dir.path().join(name);
            std::fs::write(&path, b"<?xml version=\"1.0\"?><mame build=\"0.174\"/>").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let mut card = ProviderCardState::for_provider(IdentityProvider::Mame);
            card.configure(path);
            assert!(card.executable.is_none() && card.store.is_none());
            let message = plain_error(card.error.as_deref().unwrap());
            for phrase in [
                "not the MAME program",
                "has not tried to run",
                "safe",
                "Import verification data",
                "optional",
            ] {
                assert!(message.contains(phrase), "{message}");
            }
        }
    }

    #[test]
    fn mame_missing_directory_and_non_executable_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_program(dir.path(), IdentityProvider::Mame).is_err());
        assert!(validate_program(&dir.path().join("missing"), IdentityProvider::Mame).is_err());
        #[cfg(unix)]
        {
            let path = dir.path().join("mame");
            std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
            assert!(validate_program(&path, IdentityProvider::Mame).is_err());
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(validate_program(&path, IdentityProvider::Mame).is_ok());
        }
    }

    fn snapshot() -> ManagedSourceSnapshot {
        ManagedSourceSnapshot {
            provider_id: "scummvm".into(),
            source: ManagedSourceReference::LocalPath("/usr/bin/scummvm".into()),
            provider_version: Some("2.8.0".into()),
            retrieved_at_unix_seconds: 1,
            sha256: "a".repeat(64),
            size_bytes: 42,
            etag: None,
            last_modified: None,
            expected_media_type: "application/json".into(),
            attribution_url: None,
            parser_schema_version: "1".into(),
            trust: ManagedSourceTrust::Official,
            validation_summary: "valid".into(),
            record_count: Some(1),
            warnings: Vec::new(),
        }
    }

    fn staged() -> ValidatedCandidate {
        ValidatedCandidate {
            snapshot: snapshot(),
            object_path: "/tmp/provider-object".into(),
        }
    }

    #[test]
    fn mame_card_exposes_expected_user_surface() {
        let card = IdentityProvidersPageState::card_for_test(IdentityProvider::Mame);
        assert_eq!(card.title, "MAME");
        assert!(card.actions.contains(&"Activate staged snapshot"));
        assert!(card.advanced_details_collapsed);
    }

    #[test]
    fn scummvm_card_exposes_expected_user_surface() {
        let card = IdentityProvidersPageState::card_for_test(IdentityProvider::ScummVm);
        assert_eq!(card.title, "ScummVM");
        assert!(card.states.contains(&"Coverage gap"));
    }

    #[test]
    fn confidence_labels_stay_distinct() {
        assert_eq!(DetectionClass::OfficialExact.label(), "Official exact");
        assert_eq!(
            DetectionClass::OfficialFallback.label(),
            "Official fallback"
        );
        assert_eq!(
            DetectionClass::OfficialDetectionCoverageGap.label(),
            "Official detection coverage gap"
        );
        assert_eq!(
            DetectionClass::EmuwizDerivedProbable.label(),
            "EmuWiz-derived probable"
        );
        assert_eq!(DetectionClass::Unknown.label(), "Unknown");
    }

    #[test]
    fn integrity_error_has_plain_language() {
        assert!(
            plain_error("active provider snapshot failed payload integrity validation")
                .starts_with("The active provider data failed")
        );
    }

    #[test]
    fn card_states_distinguish_unconfigured_active_staged_and_recheck() {
        let mut card = ProviderCardState::for_provider(IdentityProvider::ScummVm);
        assert_eq!(card.status_label().0, "Not configured");
        card.executable = Some("/usr/bin/scummvm".into());
        assert_eq!(card.status_label().0, "No active snapshot");
        card.staged = Some(staged());
        assert_eq!(
            card.status_label().0,
            "New data staged — activation required"
        );
        assert!(
            card.active.is_none(),
            "staging must not activate a snapshot"
        );
        card.staged = None;
        card.active = Some(snapshot());
        card.needs_recheck = true;
        assert_eq!(
            card.status_label().0,
            "Existing verification needs re-check"
        );
    }

    #[test]
    fn coverage_and_exact_copy_remain_distinct() {
        assert_ne!(
            confidence_copy(DetectionClass::OfficialExact),
            confidence_copy(DetectionClass::OfficialDetectionCoverageGap)
        );
        assert_eq!(
            confidence_copy(DetectionClass::OfficialDetectionCoverageGap).0,
            "Coverage gap"
        );
        assert!(
            confidence_copy(DetectionClass::OfficialDetectionCoverageGap)
                .1
                .contains("cannot prove")
        );
    }

    #[test]
    fn normal_copy_does_not_expose_backend_debug_names() {
        for class in [
            DetectionClass::OfficialExact,
            DetectionClass::OfficialFallback,
            DetectionClass::OfficialDetectionCoverageGap,
            DetectionClass::EmuwizDerivedProbable,
            DetectionClass::Unknown,
        ] {
            let (label, explanation) = confidence_copy(class);
            assert!(!label.contains("ProviderSnapshot"));
            assert!(!explanation.contains("ProviderSnapshot"));
        }
    }
}
