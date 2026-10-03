//! GUI-v2 presentation for the local-first bezel/decorations resolver.
//!
//! RetroArch apply is limited to the exact discovered config scope and uses
//! the core shared transaction/history executor for all mutation.

use crate::ui::components::{StatusTone, banner, technical_details};
use archivefs_core::bezel_apply::BezelPlanRefusal;
use archivefs_core::bezel_apply::{
    BezelApplyPlan, BezelApplyRequest, BezelApplyStatus, BezelPlanError,
};
use archivefs_core::bezel_decorations::{
    BezelMatchContext, DecorationAsset, DecorationEvidence, DecorationResolution, DecorationScope,
    DecorationSource, DecorationTarget, LocalBezelCatalogue, LocalBezelConfig, LocalBezelImageInfo,
    discover_local_bezel_catalogue, load_local_bezel_config, resolve_decoration,
    resolve_local_bezel_catalogue, save_local_bezel_config,
};
use archivefs_core::patch_manager::{
    RetroArchBezelApplyOptions, apply_retroarch_bezel_plan, default_shared_backup_root,
    default_shared_history_root, prepare_retroarch_bezel_plan,
};
use archivefs_core::patch_manager::{SharedApplyResult, SharedApplyStatus};
use eframe::egui;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub(super) struct BezelPanelState {
    pub selected_game: Option<String>,
    pub selected_platform: Option<String>,
    pub target: DecorationTarget,
    pub candidates: Vec<DecorationAsset>,
    pub resolution: DecorationResolution,
    pub config: LocalBezelConfig,
    pub catalogue: LocalBezelCatalogue,
    preview_id: Option<String>,
    preview_texture: Option<egui::TextureHandle>,
    preview_error: Option<String>,
    apply_plan: Option<BezelApplyPlan>,
    apply_error: Option<BezelPlanError>,
    apply_result: Option<ApplyOutcome>,
    apply_confirmation: bool,
    retroarch_config_path: Option<PathBuf>,
    retroarch_overlay_root: Option<PathBuf>,
}

impl Default for BezelPanelState {
    fn default() -> Self {
        let target = DecorationTarget {
            emulator: "retroarch".into(),
            core: None,
        };
        let config = load_local_bezel_config().unwrap_or_default().bounded();
        let catalogue = discover_local_bezel_catalogue(&config);
        let resolution =
            resolve_local_bezel_catalogue(&catalogue, &BezelMatchContext::default(), &target);
        Self {
            selected_game: None,
            selected_platform: None,
            target,
            candidates: resolution.candidates.clone(),
            resolution,
            config,
            catalogue,
            preview_id: None,
            preview_texture: None,
            preview_error: None,
            apply_plan: None,
            apply_error: None,
            apply_result: None,
            apply_confirmation: false,
            retroarch_config_path: None,
            retroarch_overlay_root: None,
        }
    }
}

impl BezelPanelState {
    /// Re-resolve after a local cache/import refresh.  The GUI never changes
    /// provider data while doing this.
    pub(super) fn resolve(&mut self) {
        if self.catalogue.assets.is_empty() && !self.candidates.is_empty() {
            self.resolution = resolve_decoration(self.candidates.clone(), self.target.clone());
            return;
        }
        let context = BezelMatchContext {
            game_title: self.selected_game.clone(),
            platform: self.selected_platform.clone(),
            ..BezelMatchContext::default()
        };
        self.resolution = resolve_local_bezel_catalogue(&self.catalogue, &context, &self.target);
        self.candidates = self.resolution.candidates.clone();
        self.preview_id = None;
        self.preview_texture = None;
        self.preview_error = None;
        self.apply_plan = None;
        self.apply_error = None;
        self.apply_result = None;
        self.apply_confirmation = false;
    }

    pub(super) fn set_game(&mut self, title: &str, platform: &str) {
        self.selected_game = Some(title.to_string());
        self.selected_platform = Some(platform.to_string());
        self.resolve();
    }

    pub(super) fn set_retroarch_scope(
        &mut self,
        config: Option<PathBuf>,
        overlays: Option<PathBuf>,
        core: Option<String>,
    ) {
        self.retroarch_config_path = config;
        self.retroarch_overlay_root = overlays;
        self.target.core = core;
    }

    fn refresh_catalogue(&mut self) {
        match load_local_bezel_config() {
            Ok(config) => {
                self.config = config.bounded();
                self.catalogue = discover_local_bezel_catalogue(&self.config);
            }
            Err(error) => {
                self.catalogue = LocalBezelCatalogue {
                    warnings: vec![error],
                    ..LocalBezelCatalogue::default()
                };
            }
        }
        self.resolve();
    }

    fn add_root(&mut self, root: PathBuf) {
        self.config.roots.push(root);
        self.config = self.config.clone().bounded();
        if let Err(error) = save_local_bezel_config(&self.config) {
            self.catalogue.warnings.push(error);
        }
        self.refresh_catalogue();
    }

    fn ensure_preview(&mut self, ui: &egui::Ui, asset: &DecorationAsset) {
        if self.preview_id.as_deref() == Some(asset.id.as_str()) {
            return;
        }
        self.preview_id = Some(asset.id.clone());
        self.preview_texture = None;
        self.preview_error = None;
        let (DecorationSource::LocalPack { path } | DecorationSource::UserOverride { path }) =
            &asset.source
        else {
            self.preview_error = Some("This provider does not expose a local preview.".into());
            return;
        };
        let path = PathBuf::from(path);
        let Ok(cache) = super::thumbnail::cache_root() else {
            self.preview_error = Some("The preview cache is unavailable.".into());
            return;
        };
        match super::thumbnail::load_local(&path, &cache) {
            Ok(pixels) => {
                self.preview_texture = Some(ui.ctx().load_texture(
                    format!("bezel-preview-{}", asset.id),
                    pixels.image,
                    egui::TextureOptions::LINEAR,
                ));
            }
            Err(error) => self.preview_error = Some(error),
        }
    }

    pub(super) fn apply_supported(&self) -> bool {
        self.apply_plan
            .as_ref()
            .is_some_and(|plan| plan.status == BezelApplyStatus::Ready)
    }

    fn preview_apply(&mut self) {
        self.apply_plan = None;
        self.apply_error = None;
        self.apply_result = None;
        self.apply_confirmation = false;
        let Some(asset) = self.resolution.selected.clone() else {
            self.apply_error = Some(BezelPlanError {
                refusal: archivefs_core::bezel_apply::BezelPlanRefusal::InvalidIdentity,
                path: None,
                detail: "a resolved bezel asset is required before planning apply".into(),
            });
            return;
        };
        let Some(image) = self.catalogue.images.get(&asset.id).cloned() else {
            self.apply_error = Some(BezelPlanError {
                refusal: archivefs_core::bezel_apply::BezelPlanRefusal::MissingSourceAsset,
                path: None,
                detail:
                    "the selected local bezel image is not available in the validated catalogue"
                        .into(),
            });
            return;
        };
        let identity = match &asset.evidence {
            DecorationEvidence::VerifiedIdentity { identity }
            | DecorationEvidence::CanonicalDatIdentity { identity } => identity.clone(),
            _ => self.selected_game.clone().unwrap_or_default(),
        };
        let request = BezelApplyRequest {
            source_asset: asset,
            source_image: image,
            resolved_identity: identity,
            platform: self.selected_platform.clone().unwrap_or_default(),
            emulator: self.target.emulator.clone(),
            core: self.target.core.clone(),
            config_path: self.retroarch_config_path.clone(),
            overlay_root: self.retroarch_overlay_root.clone(),
            approved_roots: self
                .config
                .roots
                .iter()
                .cloned()
                .chain(
                    self.retroarch_config_path
                        .as_ref()
                        .and_then(|path| path.parent())
                        .map(PathBuf::from),
                )
                .collect(),
        };
        match prepare_retroarch_bezel_plan(&request) {
            Ok(plan) => self.apply_plan = Some(plan),
            Err(error) => self.apply_error = Some(error),
        }
    }

    fn apply(&mut self) {
        let Some(plan) = self.apply_plan.as_ref() else {
            return;
        };
        let history = match default_shared_history_root() {
            Ok(path) => path,
            Err(error) => {
                self.apply_result = Some(ApplyOutcome::refused(format!("{error:?}")));
                return;
            }
        };
        let backup = match default_shared_backup_root() {
            Ok(path) => path,
            Err(error) => {
                self.apply_result = Some(ApplyOutcome::refused(format!("{error:?}")));
                return;
            }
        };
        let operation_id = archivefs_core::patch_manager::generate_shared_operation_id();
        match apply_retroarch_bezel_plan(
            plan,
            &RetroArchBezelApplyOptions {
                general_approved: true,
                replacement_approved: !plan.conflicts.is_empty(),
                operation_id,
                timestamp_unix_seconds: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_secs()),
                history_root: history,
                backup_root: backup,
            },
        ) {
            Ok(result) => self.apply_result = Some(ApplyOutcome::from_result(&result)),
            Err(error) => self.apply_result = Some(ApplyOutcome::refused(error.to_string())),
        }
    }
}

/// User-facing result of an apply attempt. The headline is plain language;
/// the raw status/refusal text lives only in `technical` (Details).
#[derive(Clone, Debug, PartialEq)]
struct ApplyOutcome {
    tone: StatusTone,
    headline: &'static str,
    detail: &'static str,
    undo_available: bool,
    technical: String,
}

impl ApplyOutcome {
    fn refused(technical: String) -> Self {
        Self {
            tone: StatusTone::Blocked,
            headline: "Apply refused",
            detail: "Nothing has changed.",
            undo_available: false,
            technical,
        }
    }

    fn from_result(result: &SharedApplyResult) -> Self {
        let journal = result
            .journal_path
            .as_ref()
            .map_or_else(|| "not written".into(), |path| path.display().to_string());
        let technical = format!("{:?}; history journal: {journal}", result.journal.status);
        // Undo is only real when a history journal was persisted for a run
        // that actually wrote something.
        let (tone, headline, detail, wrote) = apply_status_text(result.journal.status);
        Self {
            tone,
            headline,
            detail,
            undo_available: wrote && result.journal_path.is_some(),
            technical,
        }
    }
}

/// Plain-language apply outcome for a finished shared-transaction journal:
/// tone, headline, detail, and whether anything was actually written.
/// `derive_status` only returns `Failed` when zero entries reached
/// `InstalledNew`/`ReplacedExisting`/`AlreadyInstalled`, so the `Failed`
/// detail states that provable fact rather than a general backup promise.
fn apply_status_text(status: SharedApplyStatus) -> (StatusTone, &'static str, &'static str, bool) {
    match status {
        SharedApplyStatus::Success => (
            StatusTone::Success,
            "Bezel applied",
            "The bezel configuration was written.",
            true,
        ),
        SharedApplyStatus::PartialFailure => (
            StatusTone::Warning,
            "Bezel only partly applied",
            "Some changes were made and some were not. Review the history before retrying.",
            true,
        ),
        SharedApplyStatus::Failed => (
            StatusTone::Blocked,
            "Apply failed",
            "The bezel could not be applied. Nothing was written, so your previous configuration was not changed.",
            false,
        ),
        SharedApplyStatus::DryRun => (
            StatusTone::Info,
            "Dry run only",
            "Nothing has changed.",
            false,
        ),
    }
}

fn refusal_label(refusal: &BezelPlanRefusal) -> &'static str {
    use BezelPlanRefusal::*;
    match refusal {
        MissingSourceAsset => "The bezel image is no longer available.",
        SourceSymlink => "The bezel image is a shortcut (symlink), which isn't allowed.",
        SourceOutsideApprovedRoot => "The bezel image is outside your approved bezel folders.",
        SourceTooLarge => "The bezel image is too large.",
        UnsupportedSource => "This kind of bezel image isn't supported.",
        InvalidIdentity => "No game was identified for this bezel.",
        InvalidViewport => "The bezel's screen cutout is not valid.",
        UnsupportedEmulator => "Applying bezels isn't supported for this emulator yet.",
        MissingRetroArchConfigPath => "RetroArch's config location was not found.",
        MissingRetroArchOverlayRoot => "RetroArch's overlay folder was not found.",
        DestinationOutsideApprovedRoot => "The destination is outside RetroArch's folders.",
        RetroArchConfigWriterMissing => "Writing RetroArch config is not available yet.",
        RetroArchCoreRequired => "Choose a RetroArch core first.",
        RetroArchConfigScopeInvalid => "The RetroArch config scope is not valid.",
        RetroArchOverlayOutsideConfigRoot => {
            "The overlay folder is outside RetroArch's config folder."
        }
        RetroArchUnsafeName => "The game or core name can't be used safely as a file name.",
        RetroArchDestinationUnsafe => "The destination isn't safe to write to.",
    }
}

fn show_apply_outcome(ui: &mut egui::Ui, outcome: &ApplyOutcome) {
    banner(ui, outcome.headline, outcome.detail, outcome.tone);
    ui.label(if outcome.undo_available {
        "Undo available"
    } else {
        "Undo not available"
    });
    technical_details(ui, "bezel_apply_outcome_details", |ui| {
        ui.monospace(&outcome.technical);
    });
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut BezelPanelState) {
    ui.label("Local-first preview. Bezel resolution is separate from ordinary artwork.");

    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "Game: {}",
            state.selected_game.as_deref().unwrap_or("no game selected")
        ));
        ui.label(format!("Target: {}", state.target.emulator));
        if let Some(core) = &state.target.core {
            ui.label(format!("Core: {core}"));
        }
    });
    if ui.button("Refresh local bezel preview").clicked() {
        state.refresh_catalogue();
    }
    if ui.button("Add local bezel folder").clicked()
        && let Some(root) = rfd::FileDialog::new().pick_folder()
    {
        state.add_root(root);
    }
    ui.collapsing("Local bezel folders", |ui| {
        if state.config.roots.is_empty() {
            ui.label("No local bezel folders configured.");
        } else {
            for root in &state.config.roots {
                ui.label(root.display().to_string());
            }
        }
        ui.small(format!(
            "Bounded discovery: depth {}, {} image assets",
            state.config.max_depth, state.config.max_assets
        ));
    });

    let Some(selected) = state.resolution.selected.as_ref() else {
        ui.separator();
        ui.strong("No compatible local bezel is ready");
        ui.label(&state.resolution.reason);
        if state.catalogue.assets.is_empty() {
            ui.label("No local bezel assets found");
        }
        for warning in state.catalogue.warnings.iter().take(3) {
            ui.small(warning);
        }
        if state.apply_supported() {
            ui.label("Apply is available after confirmation.");
        } else {
            ui.label("Preview only / apply unsupported");
        }
        return;
    };

    ui.separator();
    ui.strong(format!("Resolved bezel: {}", selected.id));
    ui.label(format!("Precedence: {}", precedence_reason(selected)));
    ui.label(format!("Provider: {}", selected.provenance.provider));
    ui.label(format!("Reference: {}", selected.provenance.reference));
    ui.label(format!("Evidence: {}", evidence_label(&selected.evidence)));
    if let DecorationSource::LocalPack { path } | DecorationSource::UserOverride { path } =
        &selected.source
    {
        ui.label(format!("Local asset: {path}"));
    }
    if let Some(info) = state.catalogue.images.get(&selected.id) {
        ui.label(format!(
            "Resolution: {}×{} ({})",
            info.width, info.height, info.format
        ));
    }
    if let Some(viewport) = selected.viewport.as_ref() {
        ui.label(format!(
            "Viewport / cutout: left {}, top {}, {}×{}",
            viewport.left, viewport.top, viewport.width, viewport.height
        ));
    } else {
        ui.label("Viewport / cutout: not supplied");
    }
    ui.label("Preview relationship: emulator viewport remains unchanged; no files are written.");

    let selected = selected.clone();
    state.ensure_preview(ui, &selected);
    ui.group(|ui| match (&state.preview_texture, &state.preview_error) {
        (Some(texture), _) => paint_preview(
            ui,
            texture,
            &selected,
            state.catalogue.images.get(&selected.id),
        ),
        (_, Some(error)) => {
            ui.centered_and_justified(|ui| ui.label(format!("Bezel preview unavailable\n{error}")));
        }
        (None, None) => {
            ui.centered_and_justified(|ui| ui.spinner());
        }
    });

    if !state.resolution.conflicts.is_empty() {
        ui.collapsing("Conflicts and alternate candidates", |ui| {
            for conflict in &state.resolution.conflicts {
                ui.label(conflict);
            }
            for alternate in state.resolution.candidates.iter().skip(1) {
                ui.label(format!(
                    "{} · {} · {}",
                    alternate.id,
                    alternate.provenance.provider,
                    evidence_label(&alternate.evidence)
                ));
            }
        });
    }

    ui.separator();
    if ui.button("Preview Apply").clicked() {
        state.preview_apply();
    }
    if let Some(error) = &state.apply_error {
        banner(
            ui,
            "Can't apply this bezel",
            &format!("{} Nothing has changed.", refusal_label(&error.refusal)),
            StatusTone::Blocked,
        );
        technical_details(ui, "bezel_plan_refusal_details", |ui| {
            ui.label(&error.detail);
            ui.monospace(format!("Refusal: {:?}", error.refusal));
            if let Some(path) = &error.path {
                ui.monospace(path.display().to_string());
            }
        });
    }
    if let Some(plan) = state.apply_plan.clone() {
        ui.collapsing("Planned changes", |ui| {
            ui.label(format!("Plan: {}", plan.plan_id));
            ui.label(format!("Source: {}", plan.source.path.display()));
            ui.label(format!("SHA-256: {}", plan.source.sha256));
            ui.label(format!("Target: {}", plan.target.emulator));
            if let Some(destination) = &plan.target.destination_root {
                ui.label(format!("Overlay destination: {}", destination.display()));
            }
            ui.label(format!("Files to change: {}", plan.files.len()));
            ui.label(format!("Config entries: {}", plan.config_entries.len()));
            for file in &plan.files {
                ui.monospace(format!(
                    "{}{}",
                    if file.exists { "change " } else { "create " },
                    file.path.display()
                ));
            }
            for conflict in &plan.conflicts {
                ui.colored_label(egui::Color32::YELLOW, format!("Conflict: {conflict}"));
            }
            for warning in &plan.warnings {
                ui.small(warning);
            }
            for refusal in &plan.refusals {
                ui.small(format!("Blocked: {}", refusal_label(refusal)));
            }
            ui.checkbox(
                &mut state.apply_confirmation,
                "I reviewed this plan and explicitly confirm it",
            );
            if ui
                .add_enabled(
                    state.apply_confirmation && state.apply_supported(),
                    egui::Button::new("Apply bezel configuration"),
                )
                .clicked()
            {
                state.apply();
            }
            ui.label(format!(
                "Undo: {}",
                if plan.rollback.supported {
                    "Available"
                } else {
                    "Not available"
                }
            ));
            ui.small(&plan.rollback.reason);
        });
    }
    if let Some(result) = &state.apply_result {
        show_apply_outcome(ui, result);
    }
    if state.apply_supported() {
        ui.label("Apply is available after confirmation.");
    } else {
        ui.label("Preview Apply is available; Apply requires a discovered RetroArch scope and explicit confirmation.");
    }
    ui.small(
        "Source artwork and ROMs are untouched; retroarch.cfg is never edited by this workflow.",
    );
}

fn paint_preview(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    asset: &DecorationAsset,
    info: Option<&LocalBezelImageInfo>,
) {
    let outer_size = egui::vec2(ui.available_width().min(560.0), 320.0);
    let (outer, _) = ui.allocate_exact_size(outer_size, egui::Sense::hover());
    let texture_size = texture.size_vec2();
    let scale = (outer.width() / texture_size.x).min(outer.height() / texture_size.y);
    let image_size = texture_size * scale;
    let image_rect = egui::Rect::from_center_size(outer.center(), image_size);
    ui.painter().image(
        texture.id(),
        image_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    if let (Some(viewport), Some(info)) = (asset.viewport.as_ref(), info) {
        let x = image_rect.left() + image_rect.width() * viewport.left as f32 / info.width as f32;
        let y = image_rect.top() + image_rect.height() * viewport.top as f32 / info.height as f32;
        let right = image_rect.left()
            + image_rect.width() * viewport.left.saturating_add(viewport.width) as f32
                / info.width as f32;
        let bottom = image_rect.top()
            + image_rect.height() * viewport.top.saturating_add(viewport.height) as f32
                / info.height as f32;
        ui.painter().rect_stroke(
            egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(right, bottom)),
            0.0,
            egui::Stroke::new(2.0_f32, egui::Color32::LIGHT_GREEN),
            egui::StrokeKind::Inside,
        );
        ui.small("Green rectangle: configured game viewport / cutout");
    }
}

fn precedence_reason(asset: &DecorationAsset) -> &'static str {
    match (&asset.source, &asset.scope) {
        (DecorationSource::UserOverride { .. }, _) => "explicit user override",
        (_, DecorationScope::Game) => "game-specific asset",
        (_, DecorationScope::System) => "system fallback",
        (_, DecorationScope::Default) => "provider/default fallback",
    }
}

fn evidence_label(evidence: &DecorationEvidence) -> String {
    match evidence {
        DecorationEvidence::VerifiedIdentity { identity } => {
            format!("verified identity {identity}")
        }
        DecorationEvidence::CanonicalDatIdentity { identity } => format!("DAT identity {identity}"),
        DecorationEvidence::PlatformIdentity { platform } => format!("platform {platform}"),
        DecorationEvidence::ExplicitUserMapping { mapping } => format!("user mapping {mapping}"),
        DecorationEvidence::FilenameHint { value } => format!("filename hint {value}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, scope: DecorationScope) -> DecorationAsset {
        DecorationAsset {
            id: id.into(),
            scope,
            source: DecorationSource::LocalPack {
                path: format!("{id}.png"),
            },
            evidence: DecorationEvidence::VerifiedIdentity {
                identity: "game".into(),
            },
            targets: vec![DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            }],
            readiness: archivefs_core::bezel_decorations::DecorationReadiness::Ready,
            provenance: archivefs_core::bezel_decorations::DecorationProvenance {
                provider: "local-test".into(),
                reference: id.into(),
                retrieved_at_unix_seconds: None,
            },
            viewport: Some(archivefs_core::bezel_decorations::ViewportMetadata {
                left: 10,
                top: 10,
                width: 100,
                height: 80,
            }),
        }
    }

    #[test]
    fn preview_state_exposes_provenance_and_precedence_without_apply() {
        let mut state = BezelPanelState {
            selected_game: Some("Game".into()),
            target: DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            },
            candidates: vec![
                asset("system", DecorationScope::System),
                asset("game", DecorationScope::Game),
            ],
            resolution: resolve_decoration(
                vec![
                    asset("system", DecorationScope::System),
                    asset("game", DecorationScope::Game),
                ],
                DecorationTarget {
                    emulator: "retroarch".into(),
                    core: None,
                },
            ),
            ..BezelPanelState::default()
        };
        state.resolve();
        assert_eq!(
            state
                .resolution
                .selected
                .as_ref()
                .map(|asset| asset.id.as_str()),
            Some("game")
        );
        assert_eq!(
            state.resolution.selected.as_ref().map(precedence_reason),
            Some("game-specific asset")
        );
        assert_eq!(
            state
                .resolution
                .selected
                .as_ref()
                .unwrap()
                .provenance
                .provider,
            "local-test"
        );
        assert!(!state.apply_supported());
    }

    #[test]
    fn unsupported_target_is_preview_only() {
        let state = BezelPanelState::default();
        assert!(!state.apply_supported());
    }

    #[test]
    fn unavailable_source_renders_a_safe_preview_fallback() {
        let candidate = asset("missing", DecorationScope::Game);
        let mut state = BezelPanelState {
            selected_game: Some("Game".into()),
            selected_platform: Some("SNES".into()),
            target: DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            },
            candidates: vec![candidate.clone()],
            resolution: resolve_decoration(
                vec![candidate.clone()],
                DecorationTarget {
                    emulator: "retroarch".into(),
                    core: None,
                },
            ),
            config: LocalBezelConfig::default().bounded(),
            catalogue: LocalBezelCatalogue {
                assets: vec![candidate],
                ..LocalBezelCatalogue::default()
            },
            ..BezelPanelState::default()
        };
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
        });
        assert!(state.preview_error.is_some());
    }

    fn texts(output: &egui::FullOutput) -> String {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(outcome: &ApplyOutcome) -> String {
        let context = egui::Context::default();
        let output = context.run(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| show_apply_outcome(ui, outcome));
        });
        texts(&output)
    }

    #[test]
    fn refusal_is_plain_blocked_and_says_nothing_changed() {
        let refusal = BezelPlanRefusal::SourceOutsideApprovedRoot;
        let label = refusal_label(&refusal);
        assert_ne!(label, format!("{refusal:?}"));
        let outcome = ApplyOutcome::refused("SourceOutsideApprovedRoot".into());
        assert_eq!(outcome.tone, StatusTone::Blocked);
        assert!(outcome.detail.contains("Nothing has changed"));
        let rendered = render(&outcome);
        assert!(rendered.contains("Nothing has changed"));
        assert!(rendered.contains("Undo not available"));
        // Raw code stays behind the collapsed Technical details disclosure.
        assert!(rendered.contains("Technical details"));
        assert!(!rendered.contains("SourceOutsideApprovedRoot"));
    }

    #[test]
    fn undo_wording_follows_whether_anything_was_written() {
        let ok = ApplyOutcome {
            tone: StatusTone::Success,
            headline: "Bezel applied",
            detail: "",
            undo_available: true,
            technical: "Success".into(),
        };
        assert!(render(&ok).contains("Undo available"));
        assert!(!render(&ok).contains("Undo not available"));
        assert_eq!(
            ApplyOutcome::refused("x".into()).tone,
            StatusTone::Blocked,
            "failure must never use the success tone"
        );
    }

    /// A state with a selected, resolved asset and a stubbed preview texture
    /// (so `ensure_preview`'s "unavailable" fallback - which claims unbounded
    /// remaining height via `centered_and_justified` - never runs and pushes
    /// the rest of the page outside this frame's rendered bounds).
    fn state_with_selected_asset_and_plan(plan: BezelApplyPlan) -> BezelPanelState {
        let candidate = asset("game", DecorationScope::Game);
        BezelPanelState {
            selected_game: Some("Game".into()),
            selected_platform: Some("SNES".into()),
            target: DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            },
            candidates: vec![candidate.clone()],
            resolution: resolve_decoration(
                vec![candidate.clone()],
                DecorationTarget {
                    emulator: "retroarch".into(),
                    core: None,
                },
            ),
            config: LocalBezelConfig::default().bounded(),
            catalogue: LocalBezelCatalogue {
                assets: vec![candidate],
                ..LocalBezelCatalogue::default()
            },
            apply_plan: Some(plan),
            preview_id: Some("game".to_string()),
            ..BezelPanelState::default()
        }
    }

    fn plan_with_rollback(supported: bool, reason: &str) -> BezelApplyPlan {
        BezelApplyPlan {
            schema_version: 1,
            plan_id: "test-plan".into(),
            status: BezelApplyStatus::Ready,
            source: archivefs_core::bezel_apply::BezelPlanSource {
                path: "game.png".into(),
                sha256: "0".repeat(64),
                size_bytes: 1,
                provenance: archivefs_core::bezel_decorations::DecorationProvenance {
                    provider: "local-test".into(),
                    reference: "game".into(),
                    retrieved_at_unix_seconds: None,
                },
            },
            transfer_strategy:
                archivefs_core::bezel_apply::BezelAssetTransferStrategy::CopyReadOnlySource,
            target: archivefs_core::bezel_apply::BezelPlanTarget {
                emulator: "retroarch".into(),
                core: None,
                resolved_identity: "game".into(),
                platform: "SNES".into(),
                config_path: None,
                destination_root: None,
            },
            viewport: None,
            files: Vec::new(),
            config_entries: Vec::new(),
            conflicts: Vec::new(),
            warnings: Vec::new(),
            refusals: Vec::new(),
            rollback: archivefs_core::bezel_apply::BezelRollbackInfo {
                supported,
                backup_paths: Vec::new(),
                exact_restore: supported,
                reason: reason.to_string(),
            },
        }
    }

    fn text_bounds(output: &egui::FullOutput, wanted: &str) -> Vec<egui::Rect> {
        fn gather(shape: &egui::Shape, wanted: &str, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == wanted => {
                    out.push(egui::Rect::from_min_size(text.pos, text.galley.size()));
                }
                egui::Shape::Vec(nested) => {
                    for shape in nested {
                        gather(shape, wanted, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            gather(&clipped.shape, wanted, &mut out);
        }
        out
    }

    /// "Planned changes" is an `egui::CollapsingHeader`, closed by default,
    /// so its body (including the Undo line) never renders until clicked
    /// open - this simulates exactly that click, the same way the project's
    /// own `gui_v2::tests::click_label` helper does.
    fn render_planned_changes(plan: BezelApplyPlan) -> String {
        let mut state = state_with_selected_asset_and_plan(plan);
        let context = egui::Context::default();
        context.style_mut(|style| style.animation_time = 0.0);
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 4000.0),
            )),
            ..Default::default()
        };
        let mut texture = None;
        let _ = context.run(raw_input.clone(), |ctx| {
            texture = Some(ctx.load_texture(
                "bezel-test-stub",
                egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
                egui::TextureOptions::LINEAR,
            ));
        });
        state.preview_texture = texture;
        let layout = context.run(raw_input.clone(), |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
        });
        let point = text_bounds(&layout, "Planned changes")
            .into_iter()
            .next()
            .expect("\"Planned changes\" header must be on screen")
            .center();
        for pressed in [true, false] {
            let mut input = raw_input.clone();
            input.events = vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let _ = context.run(input, |context| {
                egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
            });
        }
        let _ = context.run(raw_input.clone(), |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
        });
        let output = context.run(raw_input, |context| {
            egui::CentralPanel::default().show(context, |ui| show(ui, &mut state));
        });
        texts(&output)
    }

    #[test]
    fn planned_changes_explicitly_states_undo_available() {
        let rendered =
            render_planned_changes(plan_with_rollback(true, "An exact backup was recorded."));
        assert!(rendered.contains("Undo: Available"));
        assert!(!rendered.contains("Undo: Not available"));
        assert!(rendered.contains("An exact backup was recorded."));
    }

    #[test]
    fn planned_changes_explicitly_states_undo_not_available() {
        let rendered =
            render_planned_changes(plan_with_rollback(false, "No backup root was configured."));
        assert!(rendered.contains("Undo: Not available"));
        assert!(!rendered.contains("Undo: Available"));
        assert!(rendered.contains("No backup root was configured."));
    }

    /// `derive_status` only returns `Failed` when zero entries reached
    /// InstalledNew/ReplacedExisting/AlreadyInstalled, so this reassurance
    /// must describe that provable fact, not a general backup promise.
    #[test]
    fn failed_apply_reassurance_matches_the_derive_status_guarantee() {
        let (tone, headline, detail, wrote) = apply_status_text(SharedApplyStatus::Failed);
        assert_eq!(tone, StatusTone::Blocked);
        assert_eq!(headline, "Apply failed");
        assert!(detail.contains("Nothing was written"));
        assert!(!wrote, "Failed must never claim anything was written");
    }
}
