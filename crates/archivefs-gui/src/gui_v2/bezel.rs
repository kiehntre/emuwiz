//! GUI-v2 presentation for the local-first bezel/decorations resolver.
//!
//! This page intentionally has no write path yet.  RetroArch discovery in the
//! current architecture is read-only and the shared write adapter is for
//! verified materialized mods/cheats, not emulator overlay configuration.  A
//! bezel must therefore remain an honest preview until an emulator-specific
//! adapter can prove its config format, ownership and rollback contract.

use archivefs_core::bezel_decorations::{
    DecorationAsset, DecorationEvidence, DecorationResolution, DecorationScope, DecorationSource,
    DecorationTarget, resolve_decoration,
};
use eframe::egui;

#[derive(Clone, Debug)]
pub(super) struct BezelPanelState {
    pub selected_game: Option<String>,
    pub target: DecorationTarget,
    pub candidates: Vec<DecorationAsset>,
    pub resolution: DecorationResolution,
}

impl Default for BezelPanelState {
    fn default() -> Self {
        let target = DecorationTarget {
            emulator: "retroarch".into(),
            core: None,
        };
        let candidates = Vec::new();
        let resolution = resolve_decoration(candidates.clone(), target.clone());
        Self {
            selected_game: None,
            target,
            candidates,
            resolution,
        }
    }
}

impl BezelPanelState {
    /// Re-resolve after a local cache/import refresh.  The GUI never changes
    /// provider data while doing this.
    pub(super) fn resolve(&mut self) {
        self.resolution = resolve_decoration(self.candidates.clone(), self.target.clone());
    }

    pub(super) fn apply_supported(&self) -> bool {
        false
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut BezelPanelState) {
    ui.heading("Bezel & decorations");
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
        state.resolve();
    }

    let Some(selected) = state.resolution.selected.as_ref() else {
        ui.separator();
        ui.strong("No compatible local bezel is ready");
        ui.label(&state.resolution.reason);
        if state.candidates.is_empty() {
            ui.label(
                "Add a reviewed local bezel asset to the local cache, then refresh this preview.",
            );
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
    if let Some(viewport) = selected.viewport.as_ref() {
        ui.label(format!(
            "Viewport / cutout: left {}, top {}, {}×{}",
            viewport.left, viewport.top, viewport.width, viewport.height
        ));
    } else {
        ui.label("Viewport / cutout: not supplied");
    }
    ui.label("Preview relationship: emulator viewport remains unchanged; no files are written.");

    // A real image renderer belongs to the decoder/cache lane.  Keeping this
    // bounded metadata preview visible is useful even when the image is not
    // currently decoded, and avoids making the resolver perform I/O.
    ui.group(|ui| {
        ui.centered_and_justified(|ui| {
            ui.label("Bezel preview\n(local asset selected; image decoding is deferred to the media lane)");
        });
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
}
