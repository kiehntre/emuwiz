//! Selection-bound Dragon/CoCo cassette verification worker and readiness panel.
use crate::launch_readiness_page::xroar::Context;
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(crate) fn is_cas(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cas"))
}

#[derive(Default)]
pub(crate) struct Controller {
    path: Option<PathBuf>,
    receiver: Option<(PathBuf, Receiver<Context>)>,
    context: Option<Context>,
}

impl Controller {
    /// A controller already holding a finished check for `path`, for
    /// background readiness (which never shows the panel or starts a worker).
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
                // A stopped worker is a visible refusal, never a silent restart loop.
                if belongs {
                    self.context = Some(Context {
                        selected: working.clone(),
                        result: Err(archivefs_core::launch::DragonCocoRefusal {
                            kind: archivefs_core::launch::DragonCocoRefusalKind::Unreadable,
                            detail: "the cassette check stopped unexpectedly; use Re-check".into(),
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
        let Some(path) = path.filter(|p| is_cas(p)) else {
            return;
        };
        if self.receiver.is_none() && self.context.is_none() {
            self.start(path.into(), ui.ctx().clone());
        }
        ui.collapsing("Dragon / CoCo cassette", |ui| {
            if self.receiver.is_some() {
                ui.label("Checking the cassette against your registered TOSEC catalogues…");
            }
            match self.context.as_ref().map(|c| &c.result) {
                Some(Ok(evidence)) => {
                    ui.label(format!(
                        "Exact file match in: {}",
                        evidence.catalogue_summary()
                    ));
                    ui.small(format!(
                        "Matched on size, SHA-1, MD5 and CRC32 against the DAT you registered \
                         (artifact SHA-256 {}). EmuWiz trusts that DAT's own header; it does not \
                         authenticate where the DAT came from.",
                        evidence
                            .matches
                            .first()
                            .map_or("unknown", |m| m.catalogue_artifact_sha256.as_str())
                    ));
                    let titles: Vec<&str> = {
                        let mut t: Vec<&str> =
                            evidence.matches.iter().map(|m| m.title.as_str()).collect();
                        t.sort_unstable();
                        t.dedup();
                        t
                    };
                    ui.small(format!(
                        "Catalogue title (display only): {}. This verifies which system family \
                         the file belongs to, not that the tape is complete or will load.",
                        titles.join(" / ")
                    ));
                    for step in evidence.loading_steps() {
                        ui.label(step);
                    }
                    ui.label(
                        "If your XRoar build provides a file chooser, its optional \
                         automatic loading is File > Run (Ctrl+Shift+L): select the cassette. XRoar detects \
                         the first program's type. Special loaders may still need \
                         the game's own instructions. EmuWiz only inserts the tape; \
                         loading starts when you choose it.",
                    );
                    ui.hyperlink_to(
                        "XRoar loading instructions",
                        "https://www.6809.org.uk/xroar/doc/xroar.shtml#Running-programs",
                    );
                    ui.small(
                        "Choose the machine with EMUWIZ_XROAR_MACHINE (dragon32, dragon64, coco2 or \
                         coco3) and, if needed, EMUWIZ_XROAR_FIRMWARE_DIRECTORY.",
                    );
                }
                Some(Err(refusal)) => {
                    ui.label(format!("Not verified as a Dragon/CoCo cassette: {}", refusal.detail));
                }
                None => {}
            }
            if ui
                .add_enabled(self.receiver.is_none(), egui::Button::new("Re-check cassette"))
                .clicked()
            {
                self.start(path.into(), ui.ctx().clone());
            }
        });
    }
}
