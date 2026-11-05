//! Selection-bound `.3dsx` homebrew check worker and readiness note.
use crate::launch_readiness_page::azahar::{Context, is_three_dsx};
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
            .filter(|c| Some(c.selected.as_path()) == path && self.path.as_deref() == path)
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
                        result: Err(archivefs_core::launch::ThreeDsxRefusal {
                            kind: archivefs_core::launch::ThreeDsxRefusalKind::Unreadable,
                            detail: "the .3dsx check stopped unexpectedly; use Re-check".into(),
                        }),
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
        let Some(path) = path.filter(|p| is_three_dsx(p)) else {
            return;
        };
        if self.receiver.is_none() && self.context.is_none() {
            self.start(path.into(), ui.ctx().clone());
        }
        ui.collapsing("Nintendo 3DS homebrew", |ui| {
            if self.receiver.is_some() {
                ui.label("Checking the .3dsx file…");
            }
            match self.context.as_ref().map(|c| &c.result) {
                Some(Ok(evidence)) => {
                    ui.label("Valid 3DSX homebrew file.");
                    if let Some(title) = &evidence.display_title {
                        ui.small(format!("Embedded title (display only): {title}"));
                    }
                    ui.small(format!(
                        "SHA-256 {}. This checks the file's structure so Azahar can be started \
                         with it; it is not a catalogue match and does not verify the game.",
                        evidence.sha256
                    ));
                }
                Some(Err(refusal)) => {
                    ui.label(format!(
                        "Not launchable as 3DS homebrew: {}",
                        refusal.detail
                    ));
                }
                None => {}
            }
            if ui
                .add_enabled(self.receiver.is_none(), egui::Button::new("Re-check file"))
                .clicked()
            {
                self.start(path.into(), ui.ctx().clone());
            }
        });
    }
}
