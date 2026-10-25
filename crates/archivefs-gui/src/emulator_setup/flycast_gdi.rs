//! Selection-bound read-only GDI preview worker.
use crate::launch_readiness_page::flycast::Context;
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};
#[derive(Default)]
pub(crate) struct Controller {
    path: Option<PathBuf>,
    receiver: Option<(PathBuf, Receiver<Result<Context, String>>)>,
    context: Option<Context>,
    error: Option<String>,
}
impl Controller {
    pub(crate) fn context(&self, path: Option<&Path>) -> Option<Context> {
        self.context
            .as_ref()
            .filter(|c| Some(c.selected.as_path()) == path && self.path.as_deref() == path)
            .cloned()
    }
    fn start(&mut self, path: PathBuf, ctx: egui::Context) {
        self.path = Some(path.clone());
        self.context = None;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.receiver = Some((path.clone(), rx));
        std::thread::spawn(move || {
            let _ = tx.send(Context::inspect(path));
            ctx.request_repaint();
        });
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, path: Option<&Path>) {
        if self.path.as_deref() != path {
            // Keep a bounded old worker until completion; never queue parallel
            // whole-disc hashing jobs while the user changes selection.
            self.context = None;
            self.error = None;
            self.path = path.map(Path::to_path_buf);
        }
        if let Some((working, rx)) = &self.receiver {
            let belongs = self.path.as_ref() == Some(working);
            match rx.try_recv() {
                Ok(result) => {
                    if belongs {
                        match result {
                            Ok(c) => self.context = Some(c),
                            Err(e) => self.error = Some(e),
                        }
                    }
                    self.receiver = None;
                }
                Err(TryRecvError::Disconnected) => {
                    if belongs {
                        self.error =
                            Some("GDI inspection worker stopped; refresh readiness".into());
                    }
                    self.receiver = None;
                }
                Err(TryRecvError::Empty) => ui
                    .ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        let Some(path) =
            path.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gdi")))
        else {
            return;
        };
        if self.receiver.is_none() && self.context.is_none() && self.error.is_none() {
            self.start(path.into(), ui.ctx().clone());
        }
        ui.collapsing("Dreamcast GDI readiness",|ui| {
            ui.label("Product-code identity and track freshness do not verify the full disc payload or prove playability.");
            if self.receiver.is_some() {ui.label("Reading complete GDI freshness digests and checking the native Flycast profile…");}
            if let Some(e)=&self.error {ui.label(format!("Launch refused: {e}"));}
            if let Some(c)=&self.context {
                if c.previews.is_empty() {ui.label("No Flycast profile discovered; configure Flycast in Emulator Setup.");}
                for (id,result) in &c.previews {match result {
                    Ok(p)=>{ui.label(format!("{id}: {}",if p.expired(){"Preview expired; refresh required"}else{"Preflight passed; launch rechecks every companion"}));},
                    Err(e)=>{ui.label(format!("{id}: launch refused: {e}"));},
                }}
            }
            if ui.add_enabled(self.receiver.is_none(),egui::Button::new("Refresh Dreamcast readiness")).clicked() {self.start(path.into(),ui.ctx().clone());}
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_change_discards_a_late_worker_result() {
        let roots = archivefs_core::patch_manager::FlycastProfileDiscoveryRoots::from_environment()
            .unwrap();
        let old = PathBuf::from("/synthetic/old.gdi");
        let context = Context {
            selected: old.clone(),
            identity: archivefs_core::launch::CanonicalIdentityStatus::Unknown,
            roots,
            previews: vec![],
        };
        let (tx, rx) = mpsc::channel();
        let mut controller = Controller {
            path: Some(old.clone()),
            receiver: Some((old, rx)),
            context: None,
            error: None,
        };
        tx.send(Ok(context)).unwrap();
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                controller.show(ui, Some(Path::new("/synthetic/other.iso")))
            });
        });
        assert!(controller.receiver.is_none());
        assert!(controller.context.is_none());
        assert!(
            controller
                .context(Some(Path::new("/synthetic/other.iso")))
                .is_none()
        );
    }
    #[test]
    fn new_selection_does_not_start_parallel_whole_disc_hashing() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.gdi");
        let new = dir.path().join("new.gdi");
        let (_tx, rx) = mpsc::channel();
        let mut controller = Controller {
            path: Some(old.clone()),
            receiver: Some((old.clone(), rx)),
            context: None,
            error: None,
        };
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| controller.show(ui, Some(&new)));
        });
        assert_eq!(controller.path.as_ref(), Some(&new));
        assert_eq!(controller.receiver.as_ref().map(|(p, _)| p), Some(&old));
        assert!(controller.context(Some(&new)).is_none());
    }
}
