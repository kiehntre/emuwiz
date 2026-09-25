//! GUI-v2 presentation for the local-first bezel/decorations resolver.
//!
//! This page intentionally has no write path yet.  RetroArch discovery in the
//! current architecture is read-only and the shared write adapter is for
//! verified materialized mods/cheats, not emulator overlay configuration.  A
//! bezel must therefore remain an honest preview until an emulator-specific
//! adapter can prove its config format, ownership and rollback contract.

use archivefs_core::bezel_decorations::{
    BezelMatchContext, DecorationAsset, DecorationEvidence, DecorationResolution, DecorationScope,
    DecorationSource, DecorationTarget, LocalBezelCatalogue, LocalBezelConfig, LocalBezelImageInfo,
    discover_local_bezel_catalogue, load_local_bezel_config, resolve_decoration,
    resolve_local_bezel_catalogue, save_local_bezel_config,
};
use eframe::egui;
use std::path::PathBuf;

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
    }

    pub(super) fn set_game(&mut self, title: &str, platform: &str) {
        self.selected_game = Some(title.to_string());
        self.selected_platform = Some(platform.to_string());
        self.resolve();
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
        false
    }
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
    if state.apply_supported() {
        ui.label("Apply is available after confirmation.");
    } else {
        ui.label("Preview only / apply unsupported");
    }
    ui.small("No emulator-specific bezel configuration adapter is currently proven. Source artwork and ROMs are untouched.");
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
}
