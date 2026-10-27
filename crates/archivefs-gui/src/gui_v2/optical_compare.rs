//! Two local CUE observations through the recovered Core comparator.
use archivefs_core::optical_layout_integrity::{OpticalLayoutComparison, compare_cue_paths};
use eframe::egui;
use std::sync::mpsc::{self, Receiver};
const LIMIT: &str = "Structure-only CUE comparison. Payload bytes are not read or verified: matching layouts do not establish preservation, game identity or playability. This is not an atomic capture or a live-file freshness promise. Compare again after external changes. No conversion or file writes.";
#[derive(Default)]
pub(super) struct State {
    left: String,
    right: String,
    result: Option<Result<OpticalLayoutComparison, String>>,
    worker: Option<Receiver<Result<OpticalLayoutComparison, String>>>,
}
impl State {
    fn invalidate(&mut self) {
        self.result = None;
        self.worker = None;
    }
    fn start(&mut self, ctx: egui::Context) {
        self.invalidate();
        let (tx, rx) = mpsc::channel();
        self.worker = Some(rx);
        let left = self.left.clone();
        let right = self.right.clone();
        std::thread::spawn(move || {
            let result =
                compare_cue_paths(std::path::Path::new(&left), std::path::Path::new(&right));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        let Some(rx) = &self.worker else {
            return;
        };
        match rx.try_recv() {
            Ok(r) => {
                self.result = Some(r);
                self.worker = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.result = Some(Err("Comparison worker stopped.".into()));
                self.worker = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}
pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll();
    ui.heading("Compare optical layouts");
    ui.label(LIMIT);
    ui.label("Source CUE path");
    let edited = ui
        .add(egui::TextEdit::singleline(&mut state.left).char_limit(4096))
        .changed();
    ui.label("Candidate CUE path");
    let edited = ui
        .add(egui::TextEdit::singleline(&mut state.right).char_limit(4096))
        .changed()
        || edited;
    if edited {
        state.invalidate();
    }
    if ui
        .add_enabled(
            state.worker.is_none() && !state.left.is_empty() && !state.right.is_empty(),
            egui::Button::new("Compare structure now"),
        )
        .clicked()
    {
        state.start(ui.ctx().clone());
    }
    if state.worker.is_some() {
        ui.spinner();
    }
    match &state.result {
        Some(Err(e)) => {
            ui.label(e);
        }
        Some(Ok(r)) => {
            ui.label(format!("Observed layout classification: {:?}", r.kind));
            for issue in &r.issues {
                ui.label(format!("{issue:?}"));
            }
            ui.label("No content-equivalence or preservation approval is provided.");
        }
        None => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edited_paths_drop_late_comparisons() {
        let (tx, rx) = mpsc::channel();
        let mut s = State {
            worker: Some(rx),
            ..Default::default()
        };
        s.invalidate();
        let _ = tx.send(Err("late".into()));
        s.poll();
        assert!(s.result.is_none());
    }
    #[test]
    fn blue_optical_compare_route_renders_refusal() {
        let mut s = State {
            result: Some(Err("missing CUE".into())),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut s));
        });
        assert!(
            super::super::all_tools::groups()
                .iter()
                .flat_map(|g| &g.tools)
                .any(|t| t.route
                    == super::super::routes::Route::Tool(
                        super::super::routes::ToolPage::OpticalCompare
                    ))
        );
    }
}
