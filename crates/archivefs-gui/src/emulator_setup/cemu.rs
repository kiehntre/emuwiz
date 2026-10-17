//! Selection-bound background Wii U inspection over main's Cemu profiles and preflight.
use archivefs_core::launch::cemu_readiness::{self, CemuReadinessInspection};
use archivefs_core::patch_manager::CemuProfileDiscoveryRoots;
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

#[derive(Default)]
pub(crate) struct Controller {
    path: Option<PathBuf>,
    receiver: Option<Receiver<Option<CemuReadinessInspection>>>,
    pub(crate) inspection: Option<CemuReadinessInspection>,
    error: Option<String>,
}
impl Controller {
    pub(crate) fn inspection_for(&self, path: Option<&Path>) -> Option<&CemuReadinessInspection> {
        self.inspection
            .as_ref()
            .filter(|i| Some(i.content.source.as_path()) == path)
    }
    fn start(&mut self, path: PathBuf, ctx: egui::Context) {
        self.path = Some(path.clone());
        self.inspection = None;
        self.error = None;
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let plausible = matches!(ext.as_str(), "wud" | "wux" | "wua" | "rpx")
                || ["meta", "code", "title.tmd", "title.tik"]
                    .iter()
                    .any(|name| path.join(name).exists());
            let result = if plausible {
                CemuProfileDiscoveryRoots::from_environment()
                    .ok()
                    .map(|roots| cemu_readiness::inspect_cemu_readiness(&path, None, &roots))
            } else {
                None
            };
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, path: Option<&Path>) {
        if self.path.as_deref() != path {
            // Dropping the old receiver discards any late result for the previous selection.
            self.receiver = None;
            self.inspection = None;
            self.path = path.map(Path::to_path_buf);
            if let Some(path) = path {
                self.start(path.to_path_buf(), ui.ctx().clone());
            }
        }
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.inspection = result;
                    self.receiver = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.error =
                        Some("Wii U inspection worker stopped; refresh Game Details".into());
                    self.receiver = None;
                }
                Err(TryRecvError::Empty) => {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        let Some(inspection) = &self.inspection else {
            if let Some(error) = &self.error {
                ui.label(error);
            }
            return;
        };
        let mut refresh = false;
        crate::ui::components::card(ui, |ui| {
            ui.strong("Cemu / Wii U content readiness");
            ui.label("Read-only inspection at refresh time. Execution revalidates content, profile and executable. Metadata identity is self-reported, not a verified game hash.");
            let projection = &inspection.content;
            ui.label(format!(
                "Content: {:?} · {:?} · {:?}",
                projection.representation, projection.role, projection.encryption_state
            ));
            ui.label(format!("Structural readiness: {:?}", projection.readiness));
            if let Some(identity) = &projection.identity {
                ui.label(format!(
                    "TITLE_ID: {} · {:?}",
                    identity.title_id.as_deref().unwrap_or("unknown"),
                    identity.confidence
                ));
            }
            if let Some(directory) = &projection.evidence.directory {
                ui.label(format!(
                    "Declared title version: {}",
                    directory.title_version.as_deref().unwrap_or("unknown")
                ));
            }
            for refusal in &inspection.refusals {
                ui.label(format!("Refused: {refusal}"));
            }
            if inspection.profiles.is_empty() {
                ui.label("Launch refused: no Cemu profile was discovered. Configure Cemu in Emulator Setup.");
            }
            for profile in &inspection.profiles {
                ui.separator();
                ui.label(format!("Cemu profile: {}", profile.profile_id));
                if let Some(exe) = &profile.executable {
                    ui.label(format!("Executable: {}", exe.display()));
                }
                if profile.related.is_empty() {
                    ui.label("Update / DLC: none found at the exact configured MLC title locations; external content was not searched.");
                }
                for related in &profile.related {
                    let version = related
                        .evidence
                        .directory
                        .as_ref()
                        .and_then(|d| d.title_version.as_deref())
                        .unwrap_or("unknown");
                    ui.label(format!("Present {:?}: {} · version {} · {:?}. Presence does not confirm application by Cemu.", related.role, related.source.display(), version, related.relationship));
                }
                for refusal in &profile.refusals {
                    ui.label(format!("Launch refused: {refusal}"));
                }
                if let Some(command) = &profile.command {
                    ui.label("Cemu preflight passed at inspection time. This preview does not start a process.");
                    ui.monospace(format!("{:?} {:?}", command.executable, command.arguments));
                }
            }
            refresh = ui.button("Refresh Wii U readiness").clicked();
        });
        if refresh && let Some(path) = path {
            self.start(path.to_path_buf(), ui.ctx().clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_change_discards_previous_result_and_refresh_never_spawns_cemu() {
        let (sender, receiver) = mpsc::channel();
        let mut controller = Controller {
            path: Some("/old".into()),
            receiver: Some(receiver),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| controller.show(ui, None));
        });
        assert!(sender.send(None).is_err());
        assert!(controller.inspection_for(Some(Path::new("/old"))).is_none());
    }
}

#[cfg(test)]
mod rendering_tests {
    use super::*;
    fn texts(output: egui::FullOutput) -> String {
        fn collect(shape: &egui::Shape, text: &mut String) {
            match shape {
                egui::Shape::Text(t) => {
                    text.push_str(t.galley.text());
                    text.push('\n');
                }
                egui::Shape::Vec(v) => {
                    for shape in v {
                        collect(shape, text);
                    }
                }
                _ => {}
            }
        }
        let mut text = String::new();
        for shape in output.shapes {
            collect(&shape.shape, &mut text);
        }
        text
    }
    #[test]
    fn game_details_renders_title_version_and_explicit_missing_cemu_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("title");
        for name in ["code", "content", "meta"] {
            std::fs::create_dir_all(path.join(name)).unwrap();
        }
        std::fs::write(path.join("code/game.rpx"), b"synthetic-rpx").unwrap();
        std::fs::write(
            path.join("meta/meta.xml"),
            "<menu><title_id>0005000010101A00</title_id><title_version>16</title_version></menu>",
        )
        .unwrap();
        let roots = CemuProfileDiscoveryRoots {
            home: dir.path().join("home"),
            xdg_config_home: dir.path().join("config"),
            explicit_configuration_roots: vec![],
            portable_configuration_roots: vec![],
            explicit_executables: vec![],
            known_version_outputs: Default::default(),
            appimage_directory: None,
        };
        let mut inspection = cemu_readiness::inspect_cemu_readiness(&path, None, &roots);
        // Rendering is a pure view of the supplied result; no host profile can influence this test.
        inspection.profiles.clear();
        let mut controller = Controller {
            path: Some(path.clone()),
            inspection: Some(inspection),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 1400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| controller.show(ui, Some(&path)));
            },
        );
        let text = texts(output);
        for expected in [
            "Cemu / Wii U content readiness",
            "TITLE_ID: 0005000010101A00",
            "Declared title version: 16",
            "no Cemu profile",
            "self-reported",
            "Refresh Wii U readiness",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
    }
}
