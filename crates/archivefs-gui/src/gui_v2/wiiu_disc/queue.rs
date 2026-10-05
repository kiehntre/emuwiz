//! Wii U WUD <-> WUX conversion card and the durable-queue controller behind it.
//!
//! All queue and planning IO runs on worker threads; the egui frame only reads
//! messages and snapshots. The backend (`DurableConversionQueue`) owns every
//! state transition, recovery rule and publication; this module only presents
//! and requests. Nothing here deletes, overwrites or edits the source.
//!
//! The controller lives in a module-level `thread_local` (the egui thread) so
//! the shared `mod.rs`/`pages.rs` need no new fields. ponytail: one controller
//! per UI thread; promote to an app field if a second consumer appears.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use archivefs_core::conversion_queue::durable::{
    ConversionJob, DurableConversionQueue, JobState, ProgressSnapshot, QueueSnapshot,
    RetryDisposition, ReviewedConversion, default_queue_directory,
};
use archivefs_core::wiiu_conversion::{
    WiiUConversionDirection, WiiUConversionIdentity, WiiUConversionPlan, WiiUConversionReadiness,
    WiiUConversionRefusal, WiiUConversionRequest, WiiUConversionToolInventory,
    plan_wiiu_conversion,
};
use eframe::egui;

use crate::gui_v2::library::Game;
use crate::romm_source::human_bytes;

// ---------------------------------------------------------------- plain text

pub(super) fn direction_headline(direction: WiiUConversionDirection) -> &'static str {
    match direction {
        WiiUConversionDirection::WudToWux => "Convert to WUX (smaller file)",
        WiiUConversionDirection::WuxToWud => "Convert to WUD (full disc image)",
    }
}

fn direction_meaning(direction: WiiUConversionDirection) -> &'static str {
    match direction {
        WiiUConversionDirection::WudToWux => {
            "Re-encodes the logical disc stream as WUX and verifies every logical byte against the WUD source. The physical container layout and file hash differ. Your original file is kept."
        }
        WiiUConversionDirection::WuxToWud => {
            "Re-encodes the logical disc stream as WUD and verifies every logical byte against the WUX source. Unmapped WUX payload blocks are not copied. Your original file is kept."
        }
    }
}

/// (badge, what it means and what to do next).
pub(super) fn job_text(
    state: JobState,
    retry: Option<RetryDisposition>,
) -> (&'static str, &'static str) {
    match state {
        JobState::Queued => (
            "Waiting",
            "This conversion is in the queue. It starts when you press Start.",
        ),
        JobState::Running => (
            "Converting",
            "Working in the background. It is safe to leave this page.",
        ),
        JobState::Completed => (
            "Done",
            "The new file was created and its complete logical disc stream matched the source. The container layout and file hash may differ; the original file is kept.",
        ),
        JobState::Failed => match retry {
            Some(RetryDisposition::RequiresReview) => (
                "Needs a look",
                "The conversion stopped part-way through publishing. EmuWiz kept its records; open History & Undo to review before trying again.",
            ),
            _ => (
                "Didn't finish",
                "The conversion stopped. Your original file is untouched. You can try again.",
            ),
        },
        JobState::Cancelled => (
            "Cancelled",
            "You cancelled this conversion. Nothing was published.",
        ),
        JobState::Interrupted => match retry {
            Some(RetryDisposition::RequiresReview) => (
                "Needs a look",
                "EmuWiz was closed during publishing. Open History & Undo to review before trying again.",
            ),
            _ => (
                "Interrupted",
                "EmuWiz was closed while converting. Trying again starts from the beginning.",
            ),
        },
        JobState::BlockedStale => (
            "Files changed",
            "The original file or the destination changed since you reviewed it. Check again to review a fresh plan.",
        ),
        JobState::BlockedInputMissing => (
            "Original missing",
            "The original file can no longer be found. Check the drive is connected.",
        ),
    }
}

pub(super) fn refusal_text(refusal: &WiiUConversionRefusal) -> String {
    use WiiUConversionRefusal as R;
    match refusal {
        R::WrongSourceFormat { .. } => "This file is not the kind of disc image EmuWiz expected.".into(),
        R::IncompleteSource(_) => "The disc image looks incomplete or damaged.".into(),
        R::SourcePathUnsafe | R::DestinationPathUnsafe => {
            "EmuWiz can't safely use this folder for the conversion.".into()
        }
        R::DestinationIsSource => "The new file would have the same name as the original.".into(),
        R::DestinationExists => {
            "A file with the new name already exists next to the original. EmuWiz will not overwrite it.".into()
        }
        R::HashStale | R::HashMissingForVerification | R::InvalidSourceIdentity => {
            "EmuWiz couldn't confirm the identity of the original file.".into()
        }
        R::ToolUnavailable | R::ToolCapabilityUnproven | R::ToolDoesNotSupportDirection => {
            "EmuWiz's built-in converter can't be used for this file.".into()
        }
        R::InsufficientDestinationSpace { required, available } => format!(
            "Not enough free space: needs about {}, only {} available.",
            human_bytes(*required),
            human_bytes(*available)
        ),
        R::OutputSizeUnknown => "EmuWiz can't work out how big the new file will be.".into(),
        R::UnsupportedFormat(_) => "This disc format can't be converted yet.".into(),
        R::AmbiguousSplit => "This disc image is split into parts in a way EmuWiz can't safely join.".into(),
        R::DeferredDirection => "This conversion direction is not available yet.".into(),
        R::InvalidWriterLayout(_) => "EmuWiz can't build a valid compressed layout for this disc.".into(),
    }
}

// ---------------------------------------------------------------- controller

#[derive(Debug)]
enum Msg {
    Snapshot(Result<QueueSnapshot, String>),
    Plan {
        path: PathBuf,
        generation: u64,
        plan: Box<WiiUConversionPlan>,
    },
    Progress {
        id: u64,
        progress: ProgressSnapshot,
    },
    Notice(String),
}

#[derive(Debug)]
enum Cmd {
    /// Admit a confirmed plan and start running the queue.
    Enqueue(Box<ReviewedConversion>),
    Retry(u64),
    Cancel(u64),
    /// Run queued jobs (explicit; never automatic after a restart).
    Start,
    Prune,
    /// Open the queue once so abandoned `Running` jobs become `Interrupted`.
    Recover,
}

#[derive(Default)]
struct Inbox {
    cmds: VecDeque<Cmd>,
    alive: bool,
}

pub(super) struct Controller {
    dir: Option<PathBuf>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    snapshot: Option<Result<QueueSnapshot, String>>,
    snapshot_requested: bool,
    recovered: bool,
    generation: u64,
    plans: HashMap<PathBuf, (u64, Option<Box<WiiUConversionPlan>>)>,
    progress: HashMap<u64, ProgressSnapshot>,
    inbox: Arc<Mutex<Inbox>>,
    cancel: Arc<AtomicBool>,
    notice: Option<String>,
    confirm: Option<PathBuf>,
}

impl Controller {
    fn new(dir: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            dir,
            tx,
            rx,
            snapshot: None,
            snapshot_requested: false,
            recovered: false,
            generation: 0,
            plans: HashMap::new(),
            progress: HashMap::new(),
            inbox: Arc::new(Mutex::new(Inbox::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            notice: None,
            confirm: None,
        }
    }

    fn queue_dir(dir: &Option<PathBuf>) -> Result<PathBuf, String> {
        match dir {
            Some(dir) => Ok(dir.clone()),
            None => default_queue_directory().map_err(|e| e.to_string()),
        }
    }

    /// Applies finished worker messages. Cheap; never blocks.
    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Snapshot(snapshot) => {
                    // A queue change can change every plan (destination now exists, ...).
                    self.plans.clear();
                    self.progress.retain(|id, _| {
                        snapshot.as_ref().is_ok_and(|s| {
                            s.jobs
                                .iter()
                                .any(|j| j.id == *id && j.state == JobState::Running)
                        })
                    });
                    self.snapshot = Some(snapshot);
                }
                Msg::Plan {
                    path,
                    generation,
                    plan,
                } => {
                    // Stale-result protection: only the latest request for a path is kept.
                    if self.plans.get(&path).is_some_and(|(g, _)| *g == generation) {
                        self.plans.insert(path, (generation, Some(plan)));
                    }
                }
                Msg::Progress { id, progress } => {
                    self.progress.insert(id, progress);
                }
                Msg::Notice(text) => self.notice = Some(text),
            }
        }
    }

    fn request_snapshot(&mut self, ctx: &egui::Context) {
        self.snapshot_requested = true;
        let (tx, dir, ctx) = (self.tx.clone(), self.dir.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = Self::queue_dir(&dir)
                .and_then(|root| DurableConversionQueue::inspect(&root).map_err(|e| e.to_string()));
            let _ = tx.send(Msg::Snapshot(result));
            ctx.request_repaint();
        });
    }

    fn request_plan(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        direction: WiiUConversionDirection,
    ) {
        self.generation += 1;
        let generation = self.generation;
        self.plans.insert(path.to_path_buf(), (generation, None));
        let (tx, ctx, path) = (self.tx.clone(), ctx.clone(), path.to_path_buf());
        std::thread::spawn(move || {
            let destination = path.with_extension(match direction {
                WiiUConversionDirection::WudToWux => "wux",
                WiiUConversionDirection::WuxToWud => "wud",
            });
            // The built-in native converter needs no external tool; free space is probed by the planner.
            let plan = plan_wiiu_conversion(&WiiUConversionRequest {
                source: path.clone(),
                destination,
                direction,
                source_identity: WiiUConversionIdentity::HashMissing,
                available_free_space: None,
                tools: WiiUConversionToolInventory::default(),
            });
            let _ = tx.send(Msg::Plan {
                path,
                generation,
                plan: Box::new(plan),
            });
            ctx.request_repaint();
        });
    }

    fn send(&mut self, ctx: &egui::Context, cmd: Cmd) {
        let spawn = {
            let mut inbox = self.inbox.lock().expect("queue inbox");
            inbox.cmds.push_back(cmd);
            !std::mem::replace(&mut inbox.alive, true)
        };
        if spawn {
            let (tx, dir, ctx) = (self.tx.clone(), self.dir.clone(), ctx.clone());
            let (inbox, cancel) = (self.inbox.clone(), self.cancel.clone());
            std::thread::spawn(move || worker(dir, tx, ctx, inbox, cancel));
        }
    }
}

/// Owns the queue writer lock only while there is work; exits when idle.
fn worker(
    dir: Option<PathBuf>,
    tx: mpsc::Sender<Msg>,
    ctx: egui::Context,
    inbox: Arc<Mutex<Inbox>>,
    cancel: Arc<AtomicBool>,
) {
    let notify = |msg: Msg| {
        let _ = tx.send(msg);
        ctx.request_repaint();
    };
    let opened = Controller::queue_dir(&dir)
        .and_then(|root| DurableConversionQueue::open(&root).map_err(|e| e.to_string()));
    let mut queue = match opened {
        Ok(queue) => queue,
        Err(error) => {
            inbox.lock().expect("queue inbox").cmds.clear();
            inbox.lock().expect("queue inbox").alive = false;
            let busy = error.contains("busy");
            notify(Msg::Notice(if busy {
                "Another EmuWiz window is using the conversion queue. Try again when it finishes."
                    .into()
            } else {
                format!("The conversion queue couldn't be opened: {error}")
            }));
            if let Ok(root) = Controller::queue_dir(&dir) {
                notify(Msg::Snapshot(
                    DurableConversionQueue::inspect(&root).map_err(|e| e.to_string()),
                ));
            }
            return;
        }
    };
    notify(Msg::Snapshot(Ok(queue.snapshot().clone())));
    let mut running = false;
    loop {
        let cmd = {
            let mut guard = inbox.lock().expect("queue inbox");
            match guard.cmds.pop_front() {
                Some(cmd) => Some(cmd),
                None if running
                    && queue
                        .snapshot()
                        .jobs
                        .iter()
                        .any(|j| j.state == JobState::Queued) =>
                {
                    None
                }
                None => {
                    guard.alive = false;
                    break;
                }
            }
        };
        let outcome = match cmd {
            Some(Cmd::Enqueue(reviewed)) => {
                running = true;
                queue.enqueue(*reviewed).map(|_| ())
            }
            Some(Cmd::Retry(id)) => {
                running = true;
                queue.retry(id).map(|_| ())
            }
            Some(Cmd::Cancel(id)) => queue.cancel_queued(id),
            Some(Cmd::Start) => {
                running = true;
                Ok(())
            }
            Some(Cmd::Prune) => queue.prune_completed(0).map(|_| ()),
            Some(Cmd::Recover) => Ok(()),
            None => {
                cancel.store(false, Ordering::Relaxed);
                let progress_tx = tx.clone();
                let progress_ctx = ctx.clone();
                let current = queue
                    .snapshot()
                    .jobs
                    .iter()
                    .filter(|j| j.state == JobState::Queued)
                    .min_by_key(|j| j.order)
                    .map(|j| j.id);
                queue
                    .run_next(&cancel, &mut |p| {
                        if let Some(id) = current {
                            let _ = progress_tx.send(Msg::Progress {
                                id,
                                progress: p.clone(),
                            });
                            progress_ctx.request_repaint();
                        }
                    })
                    .map(|_| ())
            }
        };
        if let Err(error) = outcome {
            notify(Msg::Notice(format!("That didn't work: {error}")));
        }
        notify(Msg::Snapshot(Ok(queue.snapshot().clone())));
    }
    notify(Msg::Snapshot(Ok(queue.snapshot().clone())));
}

thread_local! {
    static CONTROLLER: RefCell<Controller> = RefCell::new(Controller::new(None));
}

#[cfg(test)]
pub(super) fn with_controller<R>(f: impl FnOnce(&mut Controller) -> R) -> R {
    CONTROLLER.with(|c| f(&mut c.borrow_mut()))
}

#[cfg(test)]
impl Controller {
    pub(super) fn pump_for_test(&mut self) {
        self.pump();
    }
    pub(super) fn request_plan_for_test(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        d: WiiUConversionDirection,
    ) {
        self.request_plan(ctx, path, d);
    }
    pub(super) fn plan_for_test(&self, path: &Path) -> Option<Box<WiiUConversionPlan>> {
        self.plans.get(path).and_then(|(_, plan)| plan.clone())
    }
    pub(super) fn jobs_for_test(&self) -> Vec<ConversionJob> {
        match &self.snapshot {
            Some(Ok(s)) => s.jobs.clone(),
            _ => vec![],
        }
    }
    pub(super) fn notice_for_test(&self) -> Option<String> {
        self.notice.clone()
    }
    pub(super) fn request_snapshot_for_test(&mut self, ctx: &egui::Context) {
        self.request_snapshot(ctx);
    }
    pub(super) fn send_enqueue_for_test(&mut self, ctx: &egui::Context, r: ReviewedConversion) {
        self.send(ctx, Cmd::Enqueue(Box::new(r)));
    }
    pub(super) fn send_recover_for_test(&mut self, ctx: &egui::Context) {
        self.send(ctx, Cmd::Recover);
    }
    pub(super) fn send_start_for_test(&mut self, ctx: &egui::Context) {
        self.send(ctx, Cmd::Start);
    }
    pub(super) fn send_cancel_for_test(&mut self, ctx: &egui::Context, id: u64) {
        self.send(ctx, Cmd::Cancel(id));
    }
    /// Re-requests a plan (new generation) then delivers a result tagged with the OLD one.
    pub(super) fn replan_and_inject_stale_for_test(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        old: Box<WiiUConversionPlan>,
    ) {
        let stale = self.plans.get(path).map_or(0, |(g, _)| *g);
        self.plans.insert(path.to_path_buf(), (stale + 100, None));
        let _ = self.tx.send(Msg::Plan {
            path: path.to_path_buf(),
            generation: stale,
            plan: old,
        });
        let _ = ctx;
    }
}

#[cfg(test)]
pub(super) fn reset_for_test(dir: PathBuf) {
    CONTROLLER.with(|c| *c.borrow_mut() = Controller::new(Some(dir)));
}

// ---------------------------------------------------------------------- view

/// What the card shows for one game, decided without any IO.
#[derive(Debug, PartialEq)]
pub(super) enum CardState<'a> {
    Checking,
    Job(&'a ConversionJob),
    Blocked(&'a [WiiUConversionRefusal]),
    NeedsCheck,
    Ready,
}

pub(super) fn job_for<'a>(
    snapshot: &'a QueueSnapshot,
    path: &Path,
    direction: WiiUConversionDirection,
) -> Option<&'a ConversionJob> {
    snapshot
        .jobs
        .iter()
        .rev()
        .filter(|job| job.reviewed.source() == path)
        .filter(|job| {
            matches!(
                (&job.reviewed, direction),
                (
                    ReviewedConversion::WudToWux(_),
                    WiiUConversionDirection::WudToWux
                ) | (
                    ReviewedConversion::WuxToWud(_),
                    WiiUConversionDirection::WuxToWud
                )
            )
        })
        // A cancelled job leaves nothing behind; offer a fresh preview instead.
        .find(|job| job.state != JobState::Cancelled)
}

pub(super) fn card_state<'a>(
    job: Option<&'a ConversionJob>,
    plan: Option<&'a WiiUConversionPlan>,
) -> CardState<'a> {
    if let Some(job) = job {
        return CardState::Job(job);
    }
    match plan {
        None => CardState::Checking,
        Some(plan) if !plan.refusals.is_empty() => CardState::Blocked(&plan.refusals),
        Some(plan) => match plan.readiness {
            WiiUConversionReadiness::NotReady
            | WiiUConversionReadiness::Unsupported
            | WiiUConversionReadiness::Ambiguous => CardState::NeedsCheck,
            _ => CardState::Ready,
        },
    }
}

pub(super) fn show(ui: &mut egui::Ui, game: &Game, direction: WiiUConversionDirection) {
    CONTROLLER.with(|c| show_with(ui, game, direction, &mut c.borrow_mut()));
}

pub(super) fn show_with(
    ui: &mut egui::Ui,
    game: &Game,
    direction: WiiUConversionDirection,
    c: &mut Controller,
) {
    let ctx = ui.ctx().clone();
    c.pump();
    if !c.snapshot_requested {
        c.request_snapshot(&ctx);
    }
    let path = game.archive.absolute_path.clone();
    if !c.plans.contains_key(&path) {
        c.request_plan(&ctx, &path, direction);
    }
    // After a restart, abandoned "Running" rows must be recovered by the backend, once.
    if !c.recovered
        && let Some(Ok(snapshot)) = &c.snapshot
    {
        c.recovered = true;
        if snapshot.jobs.iter().any(|j| j.state == JobState::Running) {
            c.send(&ctx, Cmd::Recover);
        }
    }

    ui.strong(direction_headline(direction));
    ui.label(direction_meaning(direction));
    if let Some(notice) = c.notice.clone() {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(ui.visuals().warn_fg_color, notice);
            if ui.button("Dismiss").clicked() {
                c.notice = None;
            }
        });
    }

    let snapshot = match &c.snapshot {
        Some(Ok(snapshot)) => Some(snapshot.clone()),
        Some(Err(error)) => {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "The conversion queue can't be read right now.",
            );
            ui.collapsing("Details", |ui| {
                ui.label(error);
            });
            None
        }
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the conversion queue…");
            });
            None
        }
    };
    let plan = c
        .plans
        .get(&path)
        .and_then(|(_, plan)| plan.as_deref().cloned());
    let job = snapshot
        .as_ref()
        .and_then(|s| job_for(s, &path, direction))
        .cloned();
    let mut action = None;
    match card_state(job.as_ref(), plan.as_ref()) {
        CardState::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking this disc image…");
            });
        }
        CardState::Job(job) => {
            let live = c.progress.get(&job.id).or(job.progress.as_ref());
            job_card(ui, job, live, &mut action);
        }
        CardState::Blocked(refusals) => {
            ui.colored_label(ui.visuals().warn_fg_color, "Can't convert this one yet");
            for refusal in refusals {
                ui.label(format!("• {}", refusal_text(refusal)));
            }
            ui.collapsing("Details", |ui| {
                for refusal in refusals {
                    ui.monospace(format!("{refusal:?}"));
                }
            });
            if ui.button("Check again").clicked() {
                action = Some(Action::Replan);
            }
        }
        CardState::NeedsCheck => {
            ui.label(
                "EmuWiz needs to check this disc image more closely before it can convert it.",
            );
            if ui.button("Check again").clicked() {
                action = Some(Action::Replan);
            }
        }
        CardState::Ready => {
            if let Some(plan) = &plan {
                ready_card(ui, plan, &mut action);
            }
        }
    }

    if let Some(snapshot) = &snapshot {
        queue_list(ui, snapshot, c, &mut action);
    }
    confirm_dialog(&ctx, c, plan.as_ref(), &path, &mut action);

    match action {
        Some(Action::Replan) => {
            c.plans.remove(&path);
            c.notice = None;
        }
        Some(Action::AskConfirm) => c.confirm = Some(path),
        Some(Action::Enqueue(reviewed)) => {
            c.confirm = None;
            c.send(&ctx, Cmd::Enqueue(reviewed));
        }
        Some(Action::Cmd(cmd)) => {
            c.send(&ctx, cmd);
        }
        Some(Action::CancelRunning) => c.cancel.store(true, Ordering::Relaxed),
        None => {}
    }
}

enum Action {
    Replan,
    AskConfirm,
    Enqueue(Box<ReviewedConversion>),
    Cmd(Cmd),
    CancelRunning,
}

fn ready_card(ui: &mut egui::Ui, plan: &WiiUConversionPlan, action: &mut Option<Action>) {
    ui.label("Ready to convert");
    ui.label(format!(
        "Original size: {}",
        human_bytes(plan.space.source_bytes)
    ));
    if let Some(max) = plan
        .space
        .destination_maximum_bytes
        .or(plan.space.destination_exact_bytes)
    {
        ui.label(format!("New file: up to {}", human_bytes(max)));
    }
    ui.label("EmuWiz will compare every logical disc byte with the source. The output is a re-encoded container, so its physical layout and file hash differ.");
    if !plan.warnings.is_empty() {
        ui.label("This disc image has some unusual details. EmuWiz will still check the new file when it finishes.");
    }
    ui.collapsing("Details", |ui| {
        for warning in &plan.warnings {
            ui.monospace(warning);
        }
        ui.label(format!("Will create: {}", plan.destination.display()));
        ui.label(format!(
            "Verification: {}",
            plan.verification.steps.join(" → ")
        ));
    });
    if ui.button("Add to conversion queue…").clicked() {
        *action = Some(Action::AskConfirm);
    }
}

fn job_card(
    ui: &mut egui::Ui,
    job: &ConversionJob,
    live: Option<&ProgressSnapshot>,
    action: &mut Option<Action>,
) {
    let (badge, meaning) = job_text(job.state, job.retry);
    ui.horizontal_wrapped(|ui| {
        ui.strong(badge);
        ui.label(meaning);
    });
    if job.state == JobState::Running
        && let Some(p) = live
    {
        let fraction = match (p.bytes_processed, p.total_bytes) {
            (Some(done), Some(total)) if total > 0 => (done as f32 / total as f32).clamp(0.0, 1.0),
            _ => 0.0,
        };
        ui.add(egui::ProgressBar::new(fraction).show_percentage());
        ui.label(&p.phase);
    }
    if let Some(record) = &job.result {
        ui.label(format!("New file: {}", record.destination.display()));
        ui.label("To undo, open History & Undo.");
    }
    if let Some(diagnostic) = &job.diagnostic {
        ui.collapsing("Details", |ui| {
            ui.label(diagnostic);
        });
    }
    ui.horizontal_wrapped(|ui| match job.state {
        JobState::Queued => {
            if ui.button("Start").clicked() {
                *action = Some(Action::Cmd(Cmd::Start));
            }
            if ui.button("Cancel").clicked() {
                *action = Some(Action::Cmd(Cmd::Cancel(job.id)));
            }
        }
        JobState::Running => {
            if ui.button("Cancel").clicked() {
                *action = Some(Action::CancelRunning);
            }
        }
        JobState::Failed
        | JobState::Interrupted
        | JobState::BlockedStale
        | JobState::BlockedInputMissing => {
            if job.retry != Some(RetryDisposition::RequiresReview)
                && ui.button("Try again").clicked()
            {
                *action = Some(Action::Cmd(Cmd::Retry(job.id)));
            }
        }
        JobState::Completed | JobState::Cancelled => {}
    });
}

fn queue_list(
    ui: &mut egui::Ui,
    snapshot: &QueueSnapshot,
    c: &Controller,
    action: &mut Option<Action>,
) {
    if snapshot.jobs.is_empty() {
        return;
    }
    ui.add_space(6.0);
    egui::CollapsingHeader::new(format!("Conversion queue ({})", snapshot.jobs.len()))
        .default_open(false)
        .show(ui, |ui| {
            for job in snapshot.jobs.iter().rev() {
                ui.push_id(("wiiu_queue_job", job.id), |ui| {
                    let name = job
                        .reviewed
                        .source()
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(name);
                        ui.strong(job_text(job.state, job.retry).0);
                        if let Some(p) = c.progress.get(&job.id).or(job.progress.as_ref())
                            && job.state == JobState::Running
                            && let (Some(d), Some(t)) = (p.bytes_processed, p.total_bytes)
                            && t > 0
                        {
                            ui.label(format!("{}%", d * 100 / t));
                        }
                    });
                });
            }
            if snapshot
                .jobs
                .iter()
                .any(|j| matches!(j.state, JobState::Completed | JobState::Cancelled))
                && ui.button("Remove finished").clicked()
            {
                *action = Some(Action::Cmd(Cmd::Prune));
            }
        });
}

fn confirm_dialog(
    ctx: &egui::Context,
    c: &mut Controller,
    plan: Option<&WiiUConversionPlan>,
    path: &Path,
    action: &mut Option<Action>,
) {
    if c.confirm.as_deref() != Some(path) {
        return;
    }
    let Some(plan) = plan else {
        c.confirm = None;
        return;
    };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        c.confirm = None;
        return;
    }
    egui::Window::new("Add this conversion to the queue?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label(format!("From: {}", plan.source.display()));
            ui.label(format!("To: {}", plan.destination.display()));
            ui.label(format!("Original size: {}", human_bytes(plan.space.source_bytes)));
            ui.label("Your original file stays where it is. EmuWiz will not overwrite anything, and you can undo this afterwards in History & Undo.");
            ui.horizontal(|ui| {
                let cancel = ui.button("Cancel");
                cancel.request_focus();
                if cancel.clicked() {
                    c.confirm = None;
                }
                if ui.button("Add and start").clicked() {
                    let reviewed = match plan.direction {
                        WiiUConversionDirection::WudToWux => ReviewedConversion::WudToWux(Box::new(plan.clone())),
                        WiiUConversionDirection::WuxToWud => ReviewedConversion::WuxToWud(Box::new(plan.clone())),
                    };
                    *action = Some(Action::Enqueue(Box::new(reviewed)));
                }
            });
        });
}
