//! Standalone FBNeo's supported Blue boundary. Core's driver-only command
//! does not bind the selected archive to the runtime search path, so this
//! panel deliberately cannot mint a launch request.
use eframe::egui;

use super::{LaunchPlan, LaunchTarget};

pub(super) const RUNTIME_REFUSAL: &str = "Standalone FBNeo launch is blocked: its driver-name command does not yet prove which ROM archive the emulator will open. A complete set on disk is not proof of playability.";
pub(super) const SETUP: &str = "Use a standalone FBNeo executable and an FBNeo-specific DAT matching that build. A RetroArch FBNeo core or MAME catalogue cannot authorize this native route. Keep required BIOS, parent and device sets available; EmuWiz does not download them.";

pub(super) fn show(ui: &mut egui::Ui, plan: &LaunchPlan) {
    if plan.platform_id.as_deref() != Some("Arcade") {
        return;
    }
    ui.group(|ui| {
        ui.strong("Standalone FinalBurn Neo");
        ui.label(RUNTIME_REFUSAL);
        let native: Vec<_> = plan
            .candidates
            .iter()
            .filter(|candidate| {
                matches!(
                    candidate.target,
                    LaunchTarget::Standalone {
                        adapter_id: "fbneo",
                        ..
                    }
                )
            })
            .collect();
        if native.is_empty() {
            ui.label(
                "No standalone FBNeo profile is configured. RetroArch core discovery is separate.",
            );
        }
        for candidate in native {
            if let LaunchTarget::Standalone { profile_path, .. } = &candidate.target {
                match profile_path {
                    Some(path) => {
                        ui.small(format!("Configured executable: {}", path.display()));
                    }
                    None => {
                        ui.label("Choose a standalone FBNeo executable in emulator setup.");
                    }
                }
            }
            for blocker in &candidate.blockers {
                ui.label(&blocker.detail);
            }
        }
        ui.small(SETUP);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::LaunchPlanSummary;

    #[test]
    fn missing_native_profile_is_honest_and_opening_is_read_only() {
        let plan = LaunchPlan {
            platform_id: Some("Arcade".into()),
            game_key: Some("driver".into()),
            candidates: Vec::new(),
            summary: LaunchPlanSummary::default(),
            media_topology: None,
        };
        let context = egui::Context::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024., 600.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show(ui, &plan));
            },
        );
        fn text(shapes: &[egui::epaint::ClippedShape]) -> String {
            fn collect(shape: &egui::Shape, out: &mut String) {
                match shape {
                    egui::Shape::Text(t) => out.push_str(t.galley.text()),
                    egui::Shape::Vec(v) => {
                        for s in v {
                            collect(s, out);
                        }
                    }
                    _ => {}
                }
            }
            let mut out = String::new();
            for s in shapes {
                collect(&s.shape, &mut out);
            }
            out
        }
        let rendered = text(&output.shapes);
        assert!(rendered.contains("launch is blocked"));
        assert!(rendered.contains("No standalone FBNeo profile"));
        assert!(rendered.contains("RetroArch"));
        assert!(!rendered.contains("Launch fbneo"));
    }
}
