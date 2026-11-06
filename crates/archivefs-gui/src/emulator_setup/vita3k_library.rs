//! Selection-bound Vita `.vpk` check worker and readiness note.
use crate::launch_readiness_page::vita3k::Context;
use archivefs_core::launch::is_vpk;
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

#[derive(Default)]
pub(crate) struct Controller {
    path: Option<PathBuf>,
    receiver: Option<(PathBuf, Receiver<Context>)>,
    context: Option<Context>,
}

impl Controller {
    /// A controller already holding a finished check, for background readiness.
    pub(crate) fn with_context(path: PathBuf, context: Context) -> Self {
        Self {
            path: Some(path),
            receiver: None,
            context: Some(context),
        }
    }

    pub(crate) fn context(&self, path: Option<&Path>) -> Option<Context> {
        self.context
            .as_ref()
            .filter(|c| {
                c.relevant() && Some(c.selected.as_path()) == path && self.path.as_deref() == path
            })
            .cloned()
    }

    fn start(&mut self, path: PathBuf, ctx: egui::Context) {
        self.path = Some(path.clone());
        self.context = None;
        let (tx, rx) = mpsc::channel();
        self.receiver = Some((path.clone(), rx));
        std::thread::spawn(move || {
            let _ = tx.send(Context::inspect(path));
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ui: &egui::Ui, path: Option<&Path>) {
        if self.path.as_deref() != path {
            self.context = None;
            self.path = path.map(Path::to_path_buf);
        }
        let Some((working, rx)) = &self.receiver else {
            return;
        };
        let belongs = self.path.as_ref() == Some(working);
        match rx.try_recv() {
            Ok(context) => {
                if belongs {
                    self.context = Some(context);
                }
                self.receiver = None;
            }
            Err(TryRecvError::Disconnected) => {
                if belongs {
                    self.context = Some(Context {
                        selected: working.clone(),
                        result: Err(archivefs_core::launch::VpkRefusal {
                            kind: archivefs_core::launch::VpkRefusalKind::Unreadable,
                            detail: "the Vita package check stopped unexpectedly; use Re-check"
                                .into(),
                        }),
                        extracted_folder: false,
                    });
                }
                self.receiver = None;
            }
            Err(TryRecvError::Empty) => ui
                .ctx()
                .request_repaint_after(std::time::Duration::from_millis(100)),
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, path: Option<&Path>) {
        self.poll(ui, path);
        let Some(path) = path.filter(|p| is_vpk(p) || p.extension().is_none()) else {
            return;
        };
        if self.receiver.is_none() && self.context.is_none() {
            self.start(path.into(), ui.ctx().clone());
        }
        let relevant = is_vpk(path) || self.context.as_ref().is_some_and(|c| c.extracted_folder);
        if !relevant {
            return;
        }
        ui.collapsing("PlayStation Vita package", |ui| {
            if self.receiver.is_some() {
                ui.label("Checking the .vpk and Vita3K…");
            }
            if let Some(context) = &self.context {
                ui.label(context.status_text());
            }
            if ui
                .add_enabled(
                    self.receiver.is_none(),
                    egui::Button::new("Re-check package"),
                )
                .clicked()
            {
                self.start(path.into(), ui.ctx().clone());
            }
        });
    }
}
