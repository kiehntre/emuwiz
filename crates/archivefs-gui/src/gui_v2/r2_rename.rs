//! Optional native Blue coordinator. Keys exist only in the operator subprocess.
mod backend;
mod copy;
mod view;
use backend::{Helper, Info, Preview, Review, Selection};
use eframe::egui;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};
enum ResultValue {
    Eligibility(PathBuf, Value),
    Info(Info),
    Preview(Preview),
    Review(Review, Info),
    Executed(Value, Info),
    Reuse(Value),
    History(Value),
    Retrieved(Value),
}
struct Job {
    receiver: Receiver<Result<ResultValue, String>>,
    selection_tag: String,
}
#[derive(Default)]
pub(super) struct State {
    root: Option<PathBuf>,
    info: Option<Info>,
    preview: Option<Preview>,
    review: Option<Review>,
    reuse: Option<Value>,
    eligibility: Option<Value>,
    history: Option<Value>,
    result: Option<Value>,
    message: String,
    needs_refresh: bool,
    job: Option<Job>,
    selection_tag: String,
    setup_confirm: bool,
    preview_confirm: bool,
    action_confirm: bool,
    reuse_confirm: bool,
}
impl State {
    pub(super) fn can_choose_standard(&self) -> bool {
        !self.needs_refresh
            && self.info.as_ref().is_none_or(|i| {
                i.status["state"] == "not_enrolled" || i.status["ownership"] == "released"
            })
    }
    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }
    pub(super) fn has_state(&self) -> bool {
        self.info.is_some()
    }
    fn start(
        &mut self,
        ctx: &egui::Context,
        f: impl FnOnce() -> Result<ResultValue, String> + Send + 'static,
    ) {
        if self.busy() {
            return;
        }
        self.setup_confirm = false;
        self.preview_confirm = false;
        self.action_confirm = false;
        self.reuse_confirm = false;
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job {
            receiver: rx,
            selection_tag: self.selection_tag.clone(),
        });
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_|Err("Safety worker stopped unexpectedly. Preserve the workspace; refresh durable state before retrying.".into()));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let r=match job.receiver.try_recv(){Ok(r)=>r,Err(TryRecvError::Empty)=>return,Err(_)=>Err("Safety worker disconnected; outcome unknown. Refresh durable state before taking another action.".into())};
        let stale = job.selection_tag != self.selection_tag;
        self.job = None;
        match r {
            Err(e) => {
                self.needs_refresh = self.info.is_some();
                self.message = e;
                self.preview = None;
                self.review = None;
                self.reuse = None;
            }
            Ok(v) => {
                self.message.clear();
                match v {
                    ResultValue::Eligibility(root, v) => {
                        self.root = Some(root);
                        self.eligibility = Some(v);
                    }
                    ResultValue::Info(i) => {
                        self.needs_refresh = false;
                        self.result = None;
                        self.history = None;
                        self.root = Some(i.root.clone());
                        self.info = Some(i);
                        self.preview = None;
                        self.review = None;
                        self.reuse = None;
                    }
                    ResultValue::Preview(_) if stale => {
                        self.message = "Selection changed; review a fresh preview.".into();
                    }
                    ResultValue::Review(r, i) if stale && r.action == "apply" => {
                        self.info = Some(i);
                        self.message = "Selection changed during review. No Apply was approved. Refresh and review the registered transaction.".into();
                    }
                    ResultValue::Preview(p) => {
                        self.preview = Some(p);
                        self.review = None;
                    }
                    ResultValue::Review(r, i) => {
                        self.info = Some(i);
                        self.preview = None;
                        self.review = Some(r);
                    }
                    ResultValue::Executed(v, i) => {
                        self.result = Some(v);
                        self.info = Some(i);
                        self.review = None;
                        self.preview = None;
                        self.reuse = None;
                    }
                    ResultValue::Reuse(v) => self.reuse = Some(v),
                    ResultValue::History(v) => self.history = Some(v),
                    ResultValue::Retrieved(v) => self.result = Some(v),
                }
            }
        }
    }
    fn select_root(&mut self, root: PathBuf) {
        self.root = Some(root);
        self.info = None;
        self.needs_refresh = false;
        self.eligibility = None;
        self.preview = None;
        self.review = None;
        self.reuse = None;
        self.result = None;
        self.history = None;
        self.setup_confirm = false;
        self.preview_confirm = false;
        self.action_confirm = false;
        self.reuse_confirm = false;
    }
    fn observe_selection(&mut self, input: &Result<Selection, String>) {
        let tag = match input {
            Ok(s) => s.fingerprint(),
            Err(e) => format!("invalid:{e}"),
        };
        if tag != self.selection_tag {
            self.selection_tag = tag;
            self.preview = None;
            if self.review.as_ref().is_some_and(|r| r.action == "apply") {
                self.review = None;
            }
            self.setup_confirm = false;
            self.preview_confirm = false;
            self.action_confirm = false;
            self.reuse_confirm = false;
        }
    }
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        selection: Result<
            (
                archivefs_core::dat::rom_organisation::OrganisationPlan,
                std::collections::BTreeSet<String>,
            ),
            String,
        >,
        history: bool,
    ) {
        let input = selection.map(|(plan, approved)| Selection { plan, approved });
        self.observe_selection(&input);
        self.poll();
        if history {
            egui::CollapsingHeader::new("Advanced Protection recovery")
                .default_open(self.has_state())
                .show(ui, |ui| view::show(self, ui, input, true));
        } else {
            view::show(self, ui, input, false);
        }
    }
}

#[cfg(test)]
mod tests;

pub(super) fn choice(ui: &mut egui::Ui, selected: &mut bool, busy: bool, can_standard: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(!busy && can_standard, |ui| {
            ui.radio_value(selected, false, copy::STANDARD);
        });
        ui.add_enabled_ui(!busy, |ui| {
            ui.radio_value(selected, true, copy::ADVANCED);
        });
    });
    ui.collapsing("Compare Rename options", |ui| {
        ui.label(copy::COMPARISON);
        ui.label(copy::SCOPE);
        ui.label(copy::LIMITS);
        ui.label(copy::BENCHMARKS);
    });
    if !can_standard {
        ui.label("Finish or explicitly recover the enrolled Advanced Protection transaction before switching engines for these files.");
    }
}
