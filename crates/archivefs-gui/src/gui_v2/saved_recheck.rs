//! Check Games: "Saved results that need checking again".
//!
//! A saved catalogue result only counts while EmuWiz can show it still
//! describes the file that is there now. This card lists the saved results
//! that no longer do (saved before that proof was recorded, the file changed,
//! the catalogue changed), says why, and can check those games again.
//!
//! Listing reads saved rows and file details only. A check reads the game
//! files and saves results one bounded set at a time through the core
//! re-check engine, so stopping (or closing EmuWiz) keeps every finished set
//! and "Continue" picks up the rest. Nothing here upgrades a saved result
//! without reading the game again, and nothing here promises that a verified
//! game will run in an emulator.
use archivefs_core::SavedDatEvidenceStatus;
use archivefs_core::dat::library_verification::VerificationFindingKind;
use archivefs_core::dat::reaudit::{
    ReauditBlock, ReauditCatalogues, ReauditLimits, ReauditPlan, ReauditProgress, ReauditReport,
    ReauditStop, plan_reaudit, run_reaudit,
};
use archivefs_core::dat::sources::audit_run::DatAuditProgress;
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

#[cfg(test)]
#[path = "saved_recheck_tests.rs"]
mod tests;

pub(super) const TITLE: &str = "Saved results that need checking again";
pub(super) const NOT_A_PROMISE: &str = "Verified means the file matches its catalogue entry exactly. It does not promise that an emulator will run it.";

pub(super) enum Message {
    Plan(Result<ReauditPlan, String>),
    Progress(ReauditProgress),
    Done(Result<ReauditReport, String>, Arc<ReauditPlan>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Running {
    pub done: usize,
    pub planned: usize,
    pub verified: usize,
    pub no_match: usize,
    pub need_choice: usize,
    pub not_checked: usize,
    pub line: String,
    pub stopping: bool,
}

#[derive(Default)]
pub(super) enum Stage {
    /// Nothing listed yet; listing starts when the card is first shown.
    #[default]
    NotLoaded,
    Listing,
    Listed(Arc<ReauditPlan>),
    /// The person asked to start; this is the confirmation.
    Confirm(Arc<ReauditPlan>),
    Running(Running),
    /// The report, with the plan that was run (for the totals).
    Finished(Box<ReauditReport>, Arc<ReauditPlan>),
    CouldNotComplete(String),
}

#[derive(Default)]
pub(super) struct State {
    pub stage: Stage,
    receiver: Option<Receiver<Message>>,
    cancel: Option<Arc<AtomicBool>>,
    /// New evidence was saved: the library must be read again.
    saved: bool,
}

pub(super) fn reason_text(status: SavedDatEvidenceStatus) -> &'static str {
    match status {
        SavedDatEvidenceStatus::Current => "backed by a current check",
        SavedDatEvidenceStatus::MissingProof => {
            "saved before EmuWiz recorded which file and catalogue were checked"
        }
        SavedDatEvidenceStatus::FileChanged => "the file changed after it was checked",
        SavedDatEvidenceStatus::FileUnavailable => {
            "the file is missing or its drive is not connected"
        }
        SavedDatEvidenceStatus::CatalogueChanged => "the catalogue changed after the check",
        SavedDatEvidenceStatus::CatalogueUnavailable => "the catalogue is not available",
        SavedDatEvidenceStatus::DoesNotApply => "the saved result is not about this game",
    }
}

pub(super) fn block_text(block: ReauditBlock) -> &'static str {
    match block {
        ReauditBlock::CatalogueUnavailable => {
            "their catalogue is switched off, removed or a folder of several files (see DAT Management)"
        }
        ReauditBlock::SystemUnknown => "the game's system is not known (set it on Game Details)",
        ReauditBlock::FileUnavailable => "the file is missing or its drive is not connected",
        ReauditBlock::DoesNotApply => {
            "the saved result came from a catalogue for another system, so there is nothing to check"
        }
        ReauditBlock::LeftOutOfThisRun => "left out of this run",
    }
}

fn size_text(bytes: u64) -> String {
    let gib = bytes as f64 / (1u64 << 30) as f64;
    if gib >= 1.0 {
        format!("{gib:.1} GiB")
    } else {
        format!("{:.0} MiB", bytes as f64 / (1u64 << 20) as f64)
    }
}

fn count(n: usize) -> String {
    super::job_card::count(n as u64)
}

fn games(n: usize) -> String {
    format!("{} game{}", count(n), if n == 1 { "" } else { "s" })
}

/// Groups reasons with their counts, most common first.
pub(super) fn grouped<'a>(reasons: impl Iterator<Item = &'a str>) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for reason in reasons {
        *counts.entry(reason).or_insert(0) += 1;
    }
    let mut rows: Vec<_> = counts
        .into_iter()
        .map(|(reason, n)| (reason.to_string(), n))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl State {
    pub(super) fn busy(&self) -> bool {
        self.receiver.is_some()
    }

    /// Whether new evidence was saved since this was last asked.
    pub(super) fn take_saved(&mut self) -> bool {
        std::mem::take(&mut self.saved)
    }

    fn start(
        &mut self,
        stage: Stage,
        job: impl FnOnce(Sender<Message>, Arc<AtomicBool>) + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.receiver = Some(receiver);
        self.cancel = Some(cancel.clone());
        self.stage = stage;
        std::thread::spawn(move || job(sender, cancel));
    }

    /// Lists what needs checking again, with `list` run on a worker.
    pub(super) fn list_with(
        &mut self,
        list: impl FnOnce() -> Result<ReauditPlan, String> + Send + 'static,
    ) {
        if self.busy() {
            return;
        }
        self.start(Stage::Listing, move |sender, _| {
            let _ = sender.send(Message::Plan(list()));
        });
    }

    pub(super) fn list(&mut self) {
        self.list_with(|| {
            let database = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
            let catalogues = ReauditCatalogues::load_default()?;
            plan_reaudit(&database, &catalogues.configured())
        });
    }

    /// Checks exactly the confirmed plan, with `run` on a worker.
    pub(super) fn run_with(
        &mut self,
        plan: Arc<ReauditPlan>,
        run: impl FnOnce(
            &ReauditPlan,
            &AtomicBool,
            &dyn Fn(ReauditProgress),
        ) -> Result<ReauditReport, String>
        + Send
        + 'static,
    ) {
        if self.busy() {
            return;
        }
        let running = Running {
            planned: plan.games(),
            line: "Getting ready…".into(),
            ..Default::default()
        };
        self.start(Stage::Running(running), move |sender, cancel| {
            let progress = sender.clone();
            let result = run(&plan, &cancel, &move |p| {
                let _ = progress.send(Message::Progress(p));
            });
            let _ = sender.send(Message::Done(result, plan));
        });
    }

    fn run(&mut self, plan: Arc<ReauditPlan>) {
        self.run_with(plan, |plan, cancel, progress| {
            let database = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
            let catalogues = ReauditCatalogues::load_default()?;
            let trusted = archivefs_core::Config::load_default()
                .ok()
                .as_ref()
                .map(TrustedRoots::from_config)
                .unwrap_or_else(TrustedRoots::none);
            run_reaudit(
                &database,
                plan,
                &|source_id| catalogues.resolve(source_id),
                &trusted,
                cancel,
                ReauditLimits::default(),
                progress,
            )
        });
    }

    /// Asks the running check to stop. The set it is on is given up; sets
    /// already saved stay saved.
    pub(super) fn stop(&mut self) {
        if let (Some(cancel), Stage::Running(running)) = (&self.cancel, &mut self.stage) {
            cancel.store(true, Ordering::Relaxed);
            running.stopping = true;
        }
    }

    pub(super) fn poll(&mut self) {
        loop {
            let message = match self.receiver.as_ref().map(Receiver::try_recv) {
                Some(Ok(message)) => message,
                Some(Err(TryRecvError::Empty)) | None => return,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.receiver = None;
                    self.cancel = None;
                    if matches!(self.stage, Stage::Listing | Stage::Running(_)) {
                        // Finished sets are in the library whatever happened.
                        self.saved = true;
                        self.stage = Stage::CouldNotComplete(
                            "The check stopped unexpectedly. Sets that had finished are saved; nothing else was confirmed.".into(),
                        );
                    }
                    return;
                }
            };
            match message {
                Message::Plan(result) => {
                    self.receiver = None;
                    self.cancel = None;
                    self.stage = match result {
                        Ok(plan) => Stage::Listed(Arc::new(plan)),
                        Err(error) => Stage::CouldNotComplete(format!(
                            "The saved results could not be listed: {error} Nothing was checked."
                        )),
                    };
                }
                Message::Progress(progress) => {
                    let Stage::Running(running) = &mut self.stage else {
                        continue;
                    };
                    match progress {
                        ReauditProgress::SetStarted {
                            platform,
                            games: n,
                            done,
                            planned,
                            ..
                        } => {
                            running.done = done;
                            running.planned = planned;
                            running.line = format!("Reading {} {platform} games…", count(n));
                        }
                        ReauditProgress::Audit(DatAuditProgress::Hashing { file_name, .. }) => {
                            running.line = format!("Reading {file_name}…");
                        }
                        ReauditProgress::Audit(_) => {}
                        ReauditProgress::SetSaved {
                            verified,
                            no_match,
                            ambiguous,
                            not_checked,
                        } => {
                            running.verified += verified;
                            running.no_match += no_match;
                            running.need_choice += ambiguous;
                            running.not_checked += not_checked;
                            running.done += verified + no_match + ambiguous + not_checked;
                            running.line = "Saved this set.".into();
                        }
                        ReauditProgress::SetNotSaved { reason, retrying } => {
                            running.line = if retrying {
                                format!("A set was not saved ({reason}). Trying it again…")
                            } else {
                                format!("A set was not saved: {reason}")
                            };
                        }
                    }
                }
                Message::Done(result, plan) => {
                    self.receiver = None;
                    self.cancel = None;
                    self.stage = match result {
                        Ok(report) => {
                            self.saved = report.saved > 0;
                            Stage::Finished(Box::new(report), plan)
                        }
                        Err(error) => Stage::CouldNotComplete(error),
                    };
                }
            }
        }
    }

    /// The card. Returns nothing: every action it offers is its own.
    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        // Listing reads the person's own library and catalogues, so a test
        // that merely draws the page never starts it; tests list explicitly.
        if matches!(self.stage, Stage::NotLoaded) && !cfg!(test) {
            self.list();
        }
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        // Nothing to check and nothing waiting: no card. Old results that do
        // not apply to their game are mentioned in one line, not as a task.
        if let Stage::Listed(plan) = &self.stage
            && plan.games() == 0
            && plan.skipped() == 0
        {
            if plan.not_applicable() > 0 {
                ui.label(format!(
                    "Every saved catalogue result that can be checked is backed by a current check. {} older result{} not apply to their game (they came from another system's catalogue) and are kept as they are.",
                    count(plan.not_applicable()),
                    if plan.not_applicable() == 1 { " does" } else { "s do" }
                ));
            }
            return;
        }
        if matches!(self.stage, Stage::NotLoaded | Stage::Listing) {
            return;
        }
        let mut next: Option<Stage> = None;
        let mut start: Option<Arc<ReauditPlan>> = None;
        let mut relist = false;
        let mut stop = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(crate::ui::theme::card_title(TITLE));
            match &self.stage {
                Stage::NotLoaded | Stage::Listing => {}
                Stage::Listed(plan) => {
                    show_plan(ui, plan);
                    if plan.games() > 0
                        && super::pages::primary(ui, &format!("Check {} again", games(plan.games())))
                    {
                        next = Some(Stage::Confirm(plan.clone()));
                    }
                }
                Stage::Confirm(plan) => {
                    ui.label(format!(
                        "This reads {} ({} by recorded size) and compares each with its catalogue. Game files are only read, never changed.",
                        games(plan.games()),
                        size_text(plan.bytes())
                    ));
                    ui.label("Results are saved to the library a set at a time. You can stop at any point: every finished set stays saved, the set being read is given up, and Continue checks the rest later.");
                    ui.label("A saved match that no longer matches its file will be replaced by what the file is now; the earlier result is kept for the record.");
                    ui.label(NOT_A_PROMISE);
                    ui.horizontal(|ui| {
                        if super::pages::primary(ui, "Start checking") {
                            start = Some(plan.clone());
                        }
                        if ui.button("Not now").clicked() {
                            next = Some(Stage::Listed(plan.clone()));
                        }
                    });
                }
                Stage::Running(running) => {
                    ui.label(format!(
                        "Working… {} of {} checked",
                        count(running.done.min(running.planned)),
                        count(running.planned)
                    ));
                    ui.add(
                        egui::ProgressBar::new(if running.planned == 0 {
                            0.0
                        } else {
                            running.done.min(running.planned) as f32 / running.planned as f32
                        })
                        .desired_width(ui.available_width().min(420.0)),
                    );
                    ui.label(format!(
                        "{} verified · {} no match · {} need your choice · {} not checked",
                        count(running.verified),
                        count(running.no_match),
                        count(running.need_choice),
                        count(running.not_checked)
                    ));
                    ui.label(&running.line);
                    if running.stopping {
                        ui.label("Stopping. The set being read is not saved; every finished set is.");
                    } else if ui.button("Stop").clicked() {
                        stop = true;
                    }
                }
                Stage::Finished(report, plan) => {
                    show_report(ui, report, plan);
                    let unfinished = report.stopped.is_some() || report.not_reached > 0;
                    if unfinished {
                        if super::pages::primary(ui, "Continue") {
                            relist = true;
                        }
                    } else if ui.button("Look again").clicked() {
                        relist = true;
                    }
                }
                Stage::CouldNotComplete(error) => {
                    ui.label(format!("Could not complete. {error}"));
                    if ui.button("Look again").clicked() {
                        relist = true;
                    }
                }
            }
        });
        if stop {
            self.stop();
        }
        if let Some(stage) = next {
            self.stage = stage;
        }
        if let Some(plan) = start {
            self.run(plan);
        }
        if relist {
            self.list();
        }
    }
}

/// The four groups every saved result falls into, each exactly one of them.
fn show_groups(ui: &mut egui::Ui, plan: &ReauditPlan) {
    let tally = plan.tally();
    ui.label(format!(
        "{} saved catalogue result{} in this library:",
        count(tally.saved_results),
        if tally.saved_results == 1 { "" } else { "s" }
    ));
    ui.label(format!(
        "• {} can be checked again (they lost their proof; the game has to be read)",
        count(tally.eligible)
    ));
    ui.label(format!(
        "• {} already backed by a current check (not read again)",
        count(tally.current)
    ));
    ui.label(format!("• {} cannot be checked now", count(tally.skipped)));
    ui.label(format!(
        "• {} do not apply to their game (old results from another system's catalogue; kept as they are, nothing to check)",
        count(tally.not_applicable)
    ));
}

fn show_plan(ui: &mut egui::Ui, plan: &ReauditPlan) {
    show_groups(ui, plan);
    if plan.games() > 0 {
        ui.label(format!(
            "Until checked again, EmuWiz treats {} as not verified. Why they lost their proof:",
            if plan.games() == 1 {
                "that game"
            } else {
                "those games"
            },
        ));
        for (status, n) in plan.by_status() {
            ui.label(format!("• {}: {}", count(n), reason_text(status)));
        }
        for group in &plan.groups {
            ui.label(format!(
                "  {} {} against {}",
                count(group.items.len()),
                group.platform,
                group.catalogue_name
            ));
        }
    }
    let mut blocks: BTreeMap<ReauditBlock, usize> = BTreeMap::new();
    for blocked in plan
        .blocked
        .iter()
        .filter(|blocked| blocked.block != ReauditBlock::DoesNotApply)
    {
        *blocks.entry(blocked.block).or_insert(0) += 1;
    }
    if !blocks.is_empty() {
        ui.label("Cannot be checked now:");
        for (block, n) in blocks {
            ui.label(format!("• {}: {}", count(n), block_text(block)));
        }
    }
}

fn show_report(ui: &mut egui::Ui, report: &ReauditReport, plan: &ReauditPlan) {
    let stopped = match &report.stopped {
        None => None,
        Some(ReauditStop::Cancelled) => Some("You stopped the check.".to_string()),
        Some(ReauditStop::Interrupted(reason)) => Some(format!("The check stopped: {reason}")),
    };
    ui.strong(if stopped.is_some() || report.not_reached > 0 {
        "Partly completed"
    } else {
        "Checked again"
    });
    let tally = report.tally(plan);
    ui.label(format!(
        "Of {} that could be checked: {} completed ({} verified · {} no match · {} need your choice) · {} not checked · {} not reached",
        count(tally.eligible),
        count(tally.completed),
        count(tally.verified),
        count(tally.no_match),
        count(tally.need_choice),
        count(tally.failed),
        count(tally.not_reached)
    ));
    ui.label(format!(
        "Not part of this check: {} already current · {} cannot be checked now · {} do not apply to their game (kept as they are)",
        count(tally.current),
        count(tally.skipped),
        count(tally.not_applicable)
    ));
    if let Some(stopped) = stopped {
        ui.label(format!(
            "{stopped} {} not reached. Every finished set is saved.",
            games(report.not_reached)
        ));
    }
    if !report.contradictions.is_empty() {
        ui.label(format!(
            "{} saved match{} no longer the file that is there now. The earlier result is kept for the record:",
            count(report.contradictions.len()),
            if report.contradictions.len() == 1 { " is" } else { "es are" }
        ));
        for item in report.contradictions.iter().take(5) {
            ui.label(format!(
                "• {}: now {}",
                file_name(&item.path),
                match item.now {
                    VerificationFindingKind::Verified => "verified",
                    VerificationFindingKind::NoMatch => "no match in the catalogue",
                    VerificationFindingKind::Ambiguous => "more than one possible match",
                }
            ));
        }
        if report.contradictions.len() > 5 {
            ui.label(format!(
                "  and {} more",
                count(report.contradictions.len() - 5)
            ));
        }
    }
    if !report.not_checked.is_empty() {
        ui.label(format!(
            "{} not checked. They stay as they were and will be listed again:",
            games(report.not_checked.len())
        ));
        for (reason, n) in grouped(report.not_checked.iter().map(|note| note.reason.as_str()))
            .into_iter()
            .take(6)
        {
            let example = report
                .not_checked
                .iter()
                .find(|note| note.reason == reason)
                .map(|note| file_name(&note.path))
                .unwrap_or_default();
            ui.label(format!("• {}: {reason} (for example {example})", count(n)));
        }
    }
    for warning in &report.warnings {
        ui.label(format!("Note: {warning}"));
    }
    ui.label(NOT_A_PROMISE);
}
