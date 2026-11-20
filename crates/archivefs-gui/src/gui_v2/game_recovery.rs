//! Blue page: Game recovery.
//!
//! Shows why one game cannot start, with the evidence behind each finding,
//! and offers the recovery engine's verified preparation when one applies
//! (see `archivefs_core::recovery`). Diagnosis, preparation, verification and
//! removal all run on a worker thread; this module owns only the page state
//! and its drawing. The game file is only ever read. Preparation creates new
//! files in EmuWiz's own prepared-media folder after an explicit confirmation.

use crate::ui::components::{
    ActionStyle, StatusTone, action_button, banner, card, section_header, status_badge,
};
use archivefs_core::recovery::{
    self, ActionEffect, ActionKind, ActionPerformer, Blocking, Confidence, DiscSetLimits, Finding,
    HostFacts, PrepareProgress, PreparedMediaStore, RecoveryContext, RecoveryReport,
    RecoverySubject, RecoveryVerdict,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

const INTRO: &str = "When a game cannot start, this page looks at the game file and the \
emulators on this computer, says what is wrong and what would fix it, and can prepare a playable \
copy where that is known to be safe. Your game file is only ever read.";

/// The game this page is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Target {
    pub game_id: i64,
    pub title: String,
    pub path: PathBuf,
    /// The library's recorded system and how it was recorded. A claim only.
    pub platform: Option<String>,
    pub platform_source: Option<String>,
}

impl Target {
    fn subject(&self) -> RecoverySubject {
        RecoverySubject::new(&self.path, self.platform.as_deref())
            .with_platform_source(self.platform_source.as_deref())
    }
}

/// What the page asks the app to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Request {
    /// Open this game's Launch page.
    OpenLaunch(i64),
    /// Go back to this game's page.
    OpenGame(i64),
}

enum Outcome {
    Diagnosed(PathBuf, Box<RecoveryReport>),
    /// A change was attempted: its message, then a fresh diagnosis.
    Changed(PathBuf, bool, String, Box<RecoveryReport>),
    Verified(bool, String),
}

/// Where the page reads and writes; replaced in tests.
#[derive(Clone)]
struct Environment {
    store: Option<PreparedMediaStore>,
    host: Option<HostFacts>,
}

impl Environment {
    fn store(&self) -> Result<PreparedMediaStore, String> {
        match &self.store {
            Some(store) => Ok(store.clone()),
            None => PreparedMediaStore::for_profile(),
        }
    }

    fn diagnose(&self, target: &Target, cancel: &AtomicBool) -> Result<RecoveryReport, String> {
        let store = self.store()?;
        let host = self.host.clone().unwrap_or_else(HostFacts::probe);
        Ok(recovery::diagnose(
            &target.subject(),
            &RecoveryContext {
                store: &store,
                trusted: &TrustedRoots::none(),
                limits: DiscSetLimits::default(),
                host: &host,
                cancel,
            },
        ))
    }
}

pub(super) struct State {
    environment: Environment,
    target: Option<Target>,
    report: Option<RecoveryReport>,
    job: Option<Receiver<Outcome>>,
    busy: Option<&'static str>,
    progress: Arc<Mutex<String>>,
    cancel: Arc<AtomicBool>,
    message: Option<(bool, String)>,
    confirm_prepare: bool,
    confirm_remove: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            environment: Environment {
                store: None,
                host: None,
            },
            target: None,
            report: None,
            job: None,
            busy: None,
            progress: Arc::default(),
            cancel: Arc::default(),
            message: None,
            confirm_prepare: false,
            confirm_remove: false,
        }
    }
}

impl State {
    #[cfg(test)]
    pub(super) fn report(&self) -> Option<&RecoveryReport> {
        self.report.as_ref()
    }

    #[cfg(test)]
    pub(super) fn is_idle(&self) -> bool {
        self.job.is_none()
    }

    /// The finished diagnosis for exactly this file, if the page has one.
    /// Reads only page state: safe on every frame.
    pub(super) fn report_for(&self, path: &Path) -> Option<&RecoveryReport> {
        self.report
            .as_ref()
            .filter(|report| report.subject.path == path)
    }

    pub(super) fn is_working_on(&self, path: &Path) -> bool {
        self.job.is_some()
            && self
                .target
                .as_ref()
                .is_some_and(|target| target.path == path)
    }

    fn start(
        &mut self,
        ctx: &egui::Context,
        busy: &'static str,
        work: impl FnOnce() -> Outcome + Send + 'static,
    ) {
        if self.job.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.job = Some(receiver);
        self.busy = Some(busy);
        self.cancel.store(false, Ordering::Relaxed);
        if let Ok(mut progress) = self.progress.lock() {
            progress.clear();
        }
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(work());
            ctx.request_repaint();
        });
    }

    pub(super) fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let outcome = match job.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.busy = None;
                self.message = Some((false, "The recovery worker stopped. Try again.".into()));
                return;
            }
        };
        self.job = None;
        self.busy = None;
        let current = |path: &Path, target: &Option<Target>| {
            target.as_ref().is_some_and(|target| target.path == path)
        };
        match outcome {
            Outcome::Diagnosed(path, report) => {
                if current(&path, &self.target) {
                    self.report = Some(*report);
                }
            }
            Outcome::Changed(path, ok, text, report) => {
                crate::emulator_setup::prepared_media::forget();
                if current(&path, &self.target) {
                    self.report = Some(*report);
                    self.message = Some((ok, text));
                }
            }
            Outcome::Verified(ok, text) => self.message = Some((ok, text)),
        }
    }

    /// Points the page at a game and diagnoses it in the background. Asking
    /// again for the game already shown keeps its finished diagnosis.
    pub(super) fn open_for(&mut self, ctx: &egui::Context, target: Target) {
        if self.target.as_ref() == Some(&target) && (self.report.is_some() || self.job.is_some()) {
            return;
        }
        if self.job.is_some() {
            // One job at a time; the page shows the running one first.
            return;
        }
        self.target = Some(target);
        self.report = None;
        self.message = None;
        self.confirm_prepare = false;
        self.confirm_remove = false;
        self.diagnose(ctx);
    }

    fn diagnose(&mut self, ctx: &egui::Context) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let environment = self.environment.clone();
        let cancel = self.cancel.clone();
        self.start(ctx, "Looking at the game file…", move || {
            let report = environment
                .diagnose(&target, &cancel)
                .unwrap_or_else(|detail| unavailable_report(&target, &detail));
            Outcome::Diagnosed(target.path, Box::new(report))
        });
    }

    fn prepare(&mut self, ctx: &egui::Context) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let Some(platform) = target.platform.clone() else {
            return;
        };
        let environment = self.environment.clone();
        let cancel = self.cancel.clone();
        let progress = self.progress.clone();
        let repaint = ctx.clone();
        self.start(ctx, "Preparing a playable copy…", move || {
            let result = environment.store().and_then(|store| {
                store
                    .prepare(
                        &target.path,
                        &platform,
                        &TrustedRoots::none(),
                        &DiscSetLimits::default(),
                        &cancel,
                        &|step| {
                            let text = match step {
                                PrepareProgress::Extracting { name, index, count } => {
                                    format!("Reading file {} of {count}: {name}", index + 1)
                                }
                                PrepareProgress::Verifying { name, index, count } => {
                                    format!("Checking file {} of {count}: {name}", index + 1)
                                }
                                PrepareProgress::Publishing => "Finishing…".to_string(),
                            };
                            if let Ok(mut shown) = progress.lock() {
                                *shown = text;
                            }
                            repaint.request_repaint();
                        },
                    )
                    .map_err(|error| error.to_string())
            });
            let (ok, text) = match result {
                Ok(outcome) if outcome.link_warning.is_some() => (
                    false,
                    format!(
                        "The copy is in place in {}, but EmuWiz could not record that this \
                         game uses it ({}). Press Prepare again. Your game file was only read.",
                        outcome.summary.directory.display(),
                        outcome.link_warning.unwrap_or_default()
                    ),
                ),
                Ok(outcome) if outcome.already_prepared => (
                    true,
                    "This ZIP was read in full and holds exactly the files of a copy prepared \
                     earlier, so that copy is used; nothing was extracted. Your game file was \
                     only read."
                        .to_string(),
                ),
                Ok(outcome) => (
                    true,
                    format!(
                        "A copy was prepared and checked against the ZIP: {} files in {}. Your \
                         game file was only read.",
                        outcome.summary.file_count,
                        outcome.summary.directory.display()
                    ),
                ),
                Err(detail) => (
                    false,
                    format!("No copy was prepared: {detail}. Your game file was only read."),
                ),
            };
            let report = environment
                .diagnose(&target, &AtomicBool::new(false))
                .unwrap_or_else(|detail| unavailable_report(&target, &detail));
            Outcome::Changed(target.path, ok, text, Box::new(report))
        });
    }

    fn remove(&mut self, ctx: &egui::Context) {
        let (Some(target), Some(set_id)) = (self.target.clone(), self.removable_set_id()) else {
            return;
        };
        let environment = self.environment.clone();
        self.start(ctx, "Removing the prepared copy…", move || {
            let result = environment
                .store()
                .and_then(|store| store.remove(&set_id).map_err(|error| error.to_string()));
            let (ok, text) = match result {
                Ok(removal) if removal.kept.is_empty() => (
                    true,
                    format!(
                        "The prepared copy was removed ({} files). Your game file was not \
                         touched.",
                        removal.removed_files.len()
                    ),
                ),
                Ok(removal) => (
                    false,
                    format!(
                        "Only partly removed: {} files were removed. {} item(s) were left in place because they are \
                         not exactly what EmuWiz created: {}",
                        removal.removed_files.len(),
                        removal.kept.len(),
                        removal
                            .kept
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ),
                Err(detail) => (false, format!("Nothing was removed: {detail}")),
            };
            let report = environment
                .diagnose(&target, &AtomicBool::new(false))
                .unwrap_or_else(|detail| unavailable_report(&target, &detail));
            Outcome::Changed(target.path, ok, text, Box::new(report))
        });
    }

    fn clean(&mut self, ctx: &egui::Context) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let environment = self.environment.clone();
        self.start(ctx, "Cleaning up…", move || {
            let result = environment
                .store()
                .and_then(|store| store.clean_interrupted().map_err(|error| error.to_string()));
            let (ok, text) = match result {
                Ok(reports) => {
                    let left: usize = reports
                        .iter()
                        .filter(|report| !report.kept.is_empty())
                        .count();
                    (
                        left == 0,
                        format!(
                            "Cleaned up {} unfinished job(s); {left} could not be removed \
                             completely and were left in place. Your game file was not touched.",
                            reports.len() - left
                        ),
                    )
                }
                Err(detail) => (false, format!("Nothing was cleaned up: {detail}")),
            };
            let report = environment
                .diagnose(&target, &AtomicBool::new(false))
                .unwrap_or_else(|detail| unavailable_report(&target, &detail));
            Outcome::Changed(target.path, ok, text, Box::new(report))
        });
    }

    fn verify(&mut self, ctx: &egui::Context) {
        let Some(set_id) = self
            .report
            .as_ref()
            .and_then(|report| report.prepared.as_ref())
            .map(|prepared| prepared.set_id.clone())
        else {
            return;
        };
        let environment = self.environment.clone();
        let cancel = self.cancel.clone();
        self.start(
            ctx,
            "Checking the prepared copy…",
            move || match environment
                .store()
                .and_then(|store| store.verify_full(&set_id, &cancel))
            {
                Ok(summary) => Outcome::Verified(
                    true,
                    format!(
                        "All {} prepared files match their recorded checksums.",
                        summary.file_count
                    ),
                ),
                Err(detail) => Outcome::Verified(
                    false,
                    format!("The prepared copy did not pass: {detail}. Nothing was changed."),
                ),
            },
        );
    }

    /// The prepared copy the report says may be removed: a ready one, or a
    /// damaged one named by the plan's id.
    fn removable_set_id(&self) -> Option<String> {
        let report = self.report.as_ref()?;
        if let Some(prepared) = &report.prepared {
            return Some(prepared.set_id.clone());
        }
        report.damaged_copy.clone()
    }
}

/// The report shown when the prepared-media folder itself cannot be located.
fn unavailable_report(target: &Target, detail: &str) -> RecoveryReport {
    RecoveryReport {
        subject: target.subject(),
        verdict: RecoveryVerdict::NotAssessed,
        findings: vec![Finding {
            code: recovery::FindingCode::NotAssessed,
            title: "Game recovery is not available right now".into(),
            explanation: format!("EmuWiz's prepared-media folder could not be located: {detail}"),
            confidence: Confidence::Certain,
            blocking: Blocking::Nothing,
            evidence: Vec::new(),
            actions: Vec::new(),
        }],
        plan: None,
        prepared: None,
        damaged_copy: None,
    }
}

pub(super) fn verdict_words(verdict: RecoveryVerdict) -> (&'static str, &'static str, StatusTone) {
    match verdict {
        RecoveryVerdict::PlayableFromPreparedCopy => (
            "Ready from a prepared copy",
            "EmuWiz prepared a copy of this game's disc files and checked it against the ZIP. Start the game from its Launch page.",
            StatusTone::Success,
        ),
        RecoveryVerdict::FixableByPreparation => (
            "EmuWiz can prepare a playable copy",
            "The game is stored in a form the emulator cannot open. EmuWiz can extract its disc files as new files and check them against the ZIP; your game file stays as it is.",
            StatusTone::Info,
        ),
        RecoveryVerdict::FixableWithYourHelp => (
            "Fixable, with a step only you can do",
            "Something below needs you (for example your own firmware files). EmuWiz can still do its part.",
            StatusTone::Warning,
        ),
        RecoveryVerdict::Unsupported => (
            "EmuWiz cannot make this file playable",
            "A reviewed rule says there is no safe way to turn this file into one an installed emulator starts. Nothing was changed.",
            StatusTone::Blocked,
        ),
        RecoveryVerdict::Refused => (
            "EmuWiz will not work on this file",
            "The file could not be read safely. Nothing was extracted and nothing was changed.",
            StatusTone::Blocked,
        ),
        RecoveryVerdict::NotAssessed => (
            "No recovery rule covers this game yet",
            "The recovery engine makes no claim about it. The game's own page still explains what it knows.",
            StatusTone::Pending,
        ),
    }
}

fn confidence_words(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Certain => "certain",
        Confidence::High => "high confidence",
        Confidence::Medium => "medium confidence",
        Confidence::Low => "a hint only",
    }
}

fn blocking_badge(ui: &mut egui::Ui, blocking: Blocking) {
    match blocking {
        Blocking::Launch => status_badge(ui, "Stops the game starting", StatusTone::Blocked),
        Blocking::Preparation => status_badge(ui, "Stops preparation", StatusTone::Blocked),
        Blocking::Nothing => status_badge(ui, "Information", StatusTone::Pending),
    };
}

fn effect_badge(ui: &mut egui::Ui, effect: ActionEffect) {
    match effect {
        ActionEffect::ReadOnly => status_badge(ui, "Only reads", StatusTone::Pending),
        ActionEffect::CreatesNewFiles => status_badge(ui, "Creates new files", StatusTone::Info),
        ActionEffect::ChangesExistingState => {
            status_badge(ui, "Changes or removes something", StatusTone::Warning)
        }
    };
}

fn human_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= 1024 * MIB {
        format!("{:.1} GiB", bytes as f64 / (1024 * MIB) as f64)
    } else if bytes >= MIB {
        format!("{:.0} MiB", bytes as f64 / MIB as f64)
    } else {
        format!("{} KiB", bytes.div_ceil(1024))
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State) -> Option<Request> {
    state.poll();
    let mut request = None;
    section_header(ui, "Game recovery", Some(INTRO));
    let Some(target) = state.target.clone() else {
        banner(
            ui,
            "Choose a game first",
            "Open a game that does not start and choose “Game recovery” on its page.",
            StatusTone::Pending,
        );
        return None;
    };
    let idle = state.job.is_none();
    card(ui, |ui| {
        ui.strong(&target.title);
        ui.label(format!(
            "System recorded in your library: {}",
            target
                .platform
                .as_deref()
                .map_or("none", archivefs_core::platform::display_name_for)
        ));
        ui.collapsing("File", |ui| {
            ui.monospace(target.path.display().to_string());
        });
        ui.horizontal(|ui| {
            if action_button(ui, "Back to the game", ActionStyle::Secondary, true).clicked() {
                request = Some(Request::OpenGame(target.game_id));
            }
            if action_button(ui, "Check again", ActionStyle::Secondary, idle).clicked() {
                state.report = None;
                state.message = None;
                state.diagnose(ui.ctx());
            }
            if let Some(busy) = state.busy {
                ui.spinner();
                ui.label(busy);
                let progress = state
                    .progress
                    .lock()
                    .map(|text| text.clone())
                    .unwrap_or_default();
                if !progress.is_empty() {
                    ui.label(progress);
                }
                if action_button(ui, "Cancel", ActionStyle::Secondary, true).clicked() {
                    state.cancel.store(true, Ordering::Relaxed);
                }
            }
        });
    });
    if let Some((ok, text)) = &state.message {
        let (title, tone) = if *ok {
            ("Done", StatusTone::Success)
        } else {
            ("Not done", StatusTone::Blocked)
        };
        banner(ui, title, text, tone);
    }
    let Some(report) = state.report.clone() else {
        return request;
    };
    let (headline, explanation, tone) = verdict_words(report.verdict);
    banner(ui, headline, explanation, tone);

    // The one thing the engine itself can do next.
    let mut prepare = false;
    let mut remove = false;
    let mut verify = false;
    let mut clean = false;
    if let Some(plan) = &report.plan {
        card(ui, |ui| {
            ui.strong("What EmuWiz can do");
            ui.horizontal(|ui| effect_badge(ui, ActionEffect::CreatesNewFiles));
            ui.label(format!(
                "{} new files, {} in total, in EmuWiz's own folder. Needs {} free. Your game \
                 file is only read; nothing is downloaded.",
                plan.files.len(),
                human_bytes(plan.total_bytes),
                human_bytes(plan.required_free_bytes)
            ));
            ui.collapsing("What exactly will happen", |ui| {
                for step in &plan.steps {
                    ui.label(format!("• {step}"));
                }
                ui.label("New files:");
                for file in &plan.files {
                    ui.monospace(format!("{}  ({})", file.name, human_bytes(file.size_bytes)));
                }
                ui.label("Into:");
                ui.monospace(plan.destination.display().to_string());
            });
            ui.checkbox(
                &mut state.confirm_prepare,
                "Yes, create these new files. My game file stays as it is.",
            );
            if action_button(
                ui,
                "Prepare a playable copy",
                ActionStyle::Primary,
                idle && state.confirm_prepare,
            )
            .clicked()
            {
                prepare = true;
            }
        });
    }
    if let Some(prepared) = &report.prepared {
        card(ui, |ui| {
            ui.strong("Prepared copy");
            ui.label(format!(
                "{} files, {}. Play starts this copy; the Launch page still asks you to confirm the system and checks the emulator, its firmware and these files.",
                prepared.file_count,
                human_bytes(prepared.total_bytes)
            ));
            ui.collapsing("Where it is", |ui| {
                ui.monospace(prepared.directory.display().to_string());
            });
            ui.horizontal_wrapped(|ui| {
                if action_button(ui, "Open the Launch page", ActionStyle::Primary, true).clicked() {
                    request = Some(Request::OpenLaunch(target.game_id));
                }
                if action_button(ui, "Check the copy again", ActionStyle::Secondary, idle).clicked()
                {
                    verify = true;
                }
            });
        });
    }
    if state.removable_set_id().is_some() {
        card(ui, |ui| {
            ui.strong("Undo");
            ui.horizontal(|ui| effect_badge(ui, ActionEffect::ChangesExistingState));
            ui.label(
                "Deletes only the files EmuWiz created for this game. Your game file is not \
                 touched, and you can prepare the copy again at any time.",
            );
            ui.checkbox(&mut state.confirm_remove, "Yes, remove the prepared copy.");
            if action_button(
                ui,
                "Remove the prepared copy",
                ActionStyle::Destructive,
                idle && state.confirm_remove,
            )
            .clicked()
            {
                remove = true;
            }
        });
    }
    if report
        .finding(recovery::FindingCode::PreparationInterrupted)
        .is_some()
        && action_button(
            ui,
            "Clean up the unfinished preparation",
            ActionStyle::Secondary,
            idle,
        )
        .clicked()
    {
        clean = true;
    }

    ui.add_space(8.0);
    ui.strong("What EmuWiz found");
    for finding in &report.findings {
        show_finding(ui, finding);
    }

    if prepare {
        state.confirm_prepare = false;
        state.prepare(ui.ctx());
    }
    if remove {
        state.confirm_remove = false;
        state.remove(ui.ctx());
    }
    if verify {
        state.verify(ui.ctx());
    }
    if clean {
        state.clean(ui.ctx());
    }
    request
}

fn show_finding(ui: &mut egui::Ui, finding: &Finding) {
    ui.push_id(finding.code, |ui| {
        card(ui, |ui| {
            ui.strong(&finding.title);
            ui.horizontal_wrapped(|ui| {
                blocking_badge(ui, finding.blocking);
                ui.label(format!(
                    "How sure: {}",
                    confidence_words(finding.confidence)
                ));
            });
            ui.label(&finding.explanation);
            if !finding.evidence.is_empty() {
                ui.collapsing(format!("Evidence ({})", finding.evidence.len()), |ui| {
                    for evidence in &finding.evidence {
                        ui.label(format!("• {}", evidence.statement));
                    }
                });
            }
            for action in &finding.actions {
                // Engine actions have their own confirmed buttons above.
                if action.performer == ActionPerformer::Engine
                    && matches!(
                        action.kind,
                        ActionKind::PrepareDiscSetFromZip
                            | ActionKind::RemovePreparedCopy
                            | ActionKind::VerifyPreparedCopy
                    )
                {
                    continue;
                }
                ui.horizontal_wrapped(|ui| {
                    effect_badge(ui, action.effect);
                    ui.label(format!("{}: {}", action.title, action.detail));
                });
            }
        });
    });
}

/// The compact card on a game's own page. Draws from page state only; the
/// caller decides (with a pure check) whether the game is one to show it for.
pub(super) fn show_game_card(ui: &mut egui::Ui, state: &State, path: &Path) -> bool {
    let mut open = false;
    card(ui, |ui| {
        ui.strong("Game recovery");
        match state.report_for(path) {
            Some(report) => {
                let (headline, explanation, tone) = verdict_words(report.verdict);
                ui.horizontal(|ui| status_badge(ui, headline, tone));
                ui.label(explanation);
            }
            None if state.is_working_on(path) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking at the game file…");
                });
            }
            None => {
                ui.label("EmuWiz can look at this game file and say what would make it playable.");
            }
        }
        if action_button(ui, "Open game recovery", ActionStyle::Secondary, true).clicked() {
            open = true;
        }
    });
    open
}
