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
    /// The automatic content check for a renamed or moved file finished.
    Relinked(PathBuf, Option<(bool, String)>, Box<RecoveryReport>),
    BulkPlanned(
        Result<Box<recovery::BulkPlan>, String>,
        Option<recovery::Queue>,
        Option<String>,
    ),
    BulkRan(Result<recovery::Queue, String>),
    Storage(Result<StorageView, String>),
}

/// What the storage card shows.
#[derive(Debug, Clone)]
pub(super) struct StorageView {
    current: PathBuf,
    chosen: bool,
    available: Option<u64>,
    /// The result of checking the folder typed into the box, if any.
    checked: Option<recovery::storage::StorageCheck>,
    message: Option<(bool, String)>,
}

/// The "prepare several games" part of the page.
#[derive(Default)]
struct Bulk {
    plan: Option<Result<recovery::BulkPlan, String>>,
    selected: std::collections::BTreeSet<PathBuf>,
    confirm: bool,
    queue: Option<recovery::Queue>,
    message: Option<(bool, String)>,
    /// How many games were found and how many are already prepared.
    note: Option<String>,
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
        // A game prepared before the folder for copies was changed is still
        // served from the earlier folder; report on that copy.
        let store = match (&self.store, PreparedMediaStore::serving(&target.path)) {
            (None, Some((serving, _))) => serving,
            _ => self.store()?,
        };
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
    /// The file an automatic relink was last tried for (once per file).
    relink_tried: Option<PathBuf>,
    bulk: Bulk,
    storage: Option<StorageView>,
    storage_folder: String,
    storage_confirm: bool,
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
            relink_tried: None,
            bulk: Bulk::default(),
            storage: None,
            storage_folder: String::new(),
            storage_confirm: false,
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
            Outcome::Relinked(path, message, report) => {
                crate::emulator_setup::prepared_media::forget();
                if current(&path, &self.target) {
                    self.report = Some(*report);
                    if message.is_some() {
                        self.message = message;
                    }
                }
            }
            Outcome::BulkPlanned(plan, queue, note) => {
                self.bulk.note = note;
                self.bulk.selected = plan
                    .as_ref()
                    .map(|plan| {
                        plan.actionable()
                            .map(|item| item.candidate.path.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                self.bulk.plan = Some(plan.map(|plan| *plan));
                self.bulk.queue = queue;
                self.bulk.confirm = false;
            }
            Outcome::BulkRan(result) => {
                crate::emulator_setup::prepared_media::forget();
                // The single-game report may be about one of these games.
                self.report = None;
                self.relink_tried = None;
                self.bulk.plan = None;
                self.bulk.message = Some(match &result {
                    Ok(queue) => {
                        let counts = queue.counts();
                        let of = |key: &str| counts.get(key).copied().unwrap_or(0);
                        (
                            of("failed") == 0 && of("pending") == 0,
                            format!(
                                "{} prepared, {} already had a matching copy, {} could not be prepared, {} still to do.{} Your game files were only read.",
                                of("prepared"),
                                of("reused"),
                                of("failed"),
                                of("pending"),
                                queue
                                    .stopped
                                    .as_deref()
                                    .map(|why| format!(" Stopped: {why}"))
                                    .unwrap_or_default()
                            ),
                        )
                    }
                    Err(detail) => (false, format!("The batch did not run: {detail}.")),
                });
                if let Ok(queue) = result {
                    self.bulk.queue = Some(queue);
                }
            }
            Outcome::Storage(view) => match view {
                Ok(view) => {
                    crate::emulator_setup::prepared_media::forget();
                    self.storage = Some(view);
                }
                Err(detail) => {
                    self.storage = Some(StorageView {
                        current: PathBuf::new(),
                        chosen: false,
                        available: None,
                        checked: None,
                        message: Some((false, detail)),
                    });
                }
            },
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
            if let Ok(outcome) = &result {
                // A file renamed or moved since it was prepared gets its
                // remembered launch choice back with its copy.
                let remembered = archivefs_core::launch::remembered_launch_system::default_remembered_launch_systems_path().ok();
                recovery::moves::adopt_remembered_choice(
                    remembered.as_deref(),
                    outcome,
                    &target.path,
                );
            }
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

    /// Starts the background work a finished diagnosis calls for: a file that
    /// matches an existing copy by names, sizes and checksums but is not
    /// linked to it (renamed or moved outside EmuWiz) is compared with that
    /// copy in full, once, and linked if every file matches. Nothing is
    /// extracted and the game file is only read.
    pub(super) fn maintain(&mut self, ctx: &egui::Context) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let wanted = self.job.is_none()
            && self
                .report
                .as_ref()
                .is_some_and(|report| report.relinkable && report.subject.path == target.path)
            && self.relink_tried.as_ref() != Some(&target.path);
        if !wanted {
            return;
        }
        self.relink_tried = Some(target.path.clone());
        let environment = self.environment.clone();
        let cancel = self.cancel.clone();
        self.start(ctx, "Comparing this file with a copy prepared earlier…", move || {
            let remembered = archivefs_core::launch::remembered_launch_system::default_remembered_launch_systems_path().ok();
            let result = environment.store().and_then(|store| {
                recovery::moves::rediscover(
                    &store,
                    remembered.as_deref(),
                    &target.path,
                    &TrustedRoots::none(),
                    &DiscSetLimits::default(),
                    &cancel,
                    &|_| {},
                )
                .map_err(|error| error.to_string())
            });
            let message = match result {
                Ok(recovery::RelinkOutcome::Linked {
                    moved_from: Some(old),
                    ..
                }) => Some((
                    true,
                    format!(
                        "This is the file that was prepared earlier as {}. EmuWiz read it in full and every disc file matches the prepared copy, so that copy is used again. Nothing was extracted.",
                        old.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                )),
                Ok(recovery::RelinkOutcome::Linked { .. }) => Some((
                    true,
                    "This file holds exactly the disc of a copy prepared earlier (every file compared), so that copy is used. Nothing was extracted.".to_string(),
                )),
                Ok(recovery::RelinkOutcome::NoMatchingCopy) => None,
                Err(detail) => Some((
                    false,
                    format!("The copy prepared earlier is not used for this file: {detail}. Your game file was only read."),
                )),
            };
            let report = environment
                .diagnose(&target, &AtomicBool::new(false))
                .unwrap_or_else(|detail| unavailable_report(&target, &detail));
            Outcome::Relinked(target.path, message, Box::new(report))
        });
    }

    fn plan_bulk(&mut self, ctx: &egui::Context, candidates: Vec<recovery::BulkCandidate>) {
        let environment = self.environment.clone();
        let cancel = self.cancel.clone();
        self.bulk.message = None;
        self.start(ctx, "Looking at the games…", move || {
            let result = environment.store().and_then(|store| {
                let queue = recovery::bulk::load_queue(&store);
                // Games already prepared need no review, and one batch is
                // bounded: a larger library is worked through in turns.
                let found = candidates.len();
                let mut todo: Vec<_> = candidates
                    .into_iter()
                    .filter(|candidate| {
                        PreparedMediaStore::serving(&candidate.path).is_none()
                            && store.prepared_for_source(&candidate.path).is_none()
                    })
                    .collect();
                let ready = found - todo.len();
                let later = todo.len().saturating_sub(recovery::bulk::MAX_BULK_ITEMS);
                todo.truncate(recovery::bulk::MAX_BULK_ITEMS);
                let note = format!(
                    "{found} game(s) of this kind are in your library; {ready} already have a prepared copy.{}",
                    if later > 0 {
                        format!(" {later} more will be listed after these are done.")
                    } else {
                        String::new()
                    }
                );
                recovery::bulk::plan_bulk(
                    &store,
                    &todo,
                    &TrustedRoots::none(),
                    &DiscSetLimits::default(),
                    &cancel,
                )
                .map(|plan| (plan, queue, note))
            });
            match result {
                Ok((plan, queue, note)) => {
                    Outcome::BulkPlanned(Ok(Box::new(plan)), queue, Some(note))
                }
                Err(detail) => Outcome::BulkPlanned(Err(detail), None, None),
            }
        });
    }

    /// Runs the saved batch; with `start`, first saves the selection of the
    /// reviewed plan as the batch.
    fn run_bulk(&mut self, ctx: &egui::Context, start: bool) {
        let environment = self.environment.clone();
        let cancel = self.cancel.clone();
        let progress = self.progress.clone();
        let repaint = ctx.clone();
        let plan = self.bulk.plan.clone().and_then(Result::ok);
        let selected = self.bulk.selected.clone();
        self.bulk.message = None;
        self.start(
            ctx,
            "Preparing the selected games, one at a time…",
            move || {
                let result = environment.store().and_then(|store| {
                    if start {
                        let plan = plan.ok_or("the list was not reviewed")?;
                        recovery::bulk::start_queue(&store, &plan, &|path| selected.contains(path))
                            .map_err(|error| error.to_string())?;
                    }
                    let titles = std::cell::RefCell::new(String::new());
                    let remembered = archivefs_core::launch::remembered_launch_system::default_remembered_launch_systems_path().ok();
                    recovery::bulk::run_queue(
                        &store,
                        remembered.as_deref(),
                        &TrustedRoots::none(),
                        &DiscSetLimits::default(),
                        &cancel,
                        &|step| {
                            let text = match step {
                                recovery::bulk::BulkProgress::Starting {
                                    index,
                                    count,
                                    title,
                                } => {
                                    *titles.borrow_mut() = title.to_string();
                                    format!("Game {} of {count}: {title}", index + 1)
                                }
                                recovery::bulk::BulkProgress::Step { index, count, step } => {
                                    let part = match step {
                                        PrepareProgress::Extracting { index, count, .. } => {
                                            format!("reading file {} of {count}", index + 1)
                                        }
                                        PrepareProgress::Verifying { index, count, .. } => {
                                            format!("checking file {} of {count}", index + 1)
                                        }
                                        PrepareProgress::Publishing => "finishing".to_string(),
                                    };
                                    format!(
                                        "Game {} of {count}: {} ({part})",
                                        index + 1,
                                        titles.borrow()
                                    )
                                }
                            };
                            if let Ok(mut shown) = progress.lock() {
                                *shown = text;
                            }
                            repaint.request_repaint();
                        },
                    )
                    .map_err(|error| error.to_string())
                });
                Outcome::BulkRan(result)
            },
        );
    }

    fn clear_bulk(&mut self) {
        if self.job.is_some() {
            return;
        }
        if let Ok(store) = self.environment.store() {
            // One small file in EmuWiz's own folder.
            let _ = recovery::bulk::clear_queue(&store);
        }
        self.bulk = Bulk::default();
    }

    /// Reads the storage setting; with `check`, examines that folder; with
    /// `apply`, saves it (`None` clears the choice) after it passes.
    fn storage_job(
        &mut self,
        ctx: &egui::Context,
        check: Option<PathBuf>,
        apply: Option<Option<PathBuf>>,
    ) {
        self.start(ctx, "Checking the folder…", move || {
            // Without the list of game folders the "not inside a game folder"
            // check cannot be made, so nothing is checked or changed.
            let library = recovery::storage::library_folders();
            let mut message = None;
            let mut checked = None;
            match &library {
                Err(detail) => {
                    if check.is_some() || apply.as_ref().is_some_and(Option::is_some) {
                        message = Some((
                            false,
                            format!("The folder was not checked or changed: {detail}."),
                        ));
                    } else if let Some(None) = &apply {
                        // Going back to the default needs no library check.
                        message = Some(match recovery::storage::set_folder(None, &[]) {
                            Ok(_) => (
                                true,
                                "Prepared copies will be kept in EmuWiz's own data folder again.".to_string(),
                            ),
                            Err(detail) => (false, format!("The folder was not changed: {detail}")),
                        });
                    }
                }
                Ok(library) => {
                    if let Some(choice) = &apply {
                        message = Some(
                            match recovery::storage::set_folder(choice.as_deref(), library) {
                                Ok(Some(_)) => (
                                    true,
                                    "New prepared copies will be kept in this folder. Games already prepared keep working from the folder their copies are in.".to_string(),
                                ),
                                Ok(None) => (
                                    true,
                                    "New prepared copies will be kept in EmuWiz's own data folder again. Games already prepared keep working from the folder their copies are in.".to_string(),
                                ),
                                Err(detail) => (false, format!("The folder was not changed: {detail}")),
                            },
                        );
                    }
                    checked = check.map(|folder| recovery::storage::check_folder(&folder, library, false));
                }
            }
            Outcome::Storage(match recovery::storage::describe_current() {
                Ok((store, chosen)) => Ok(StorageView {
                    available: store.available_bytes().ok(),
                    current: store.root().to_path_buf(),
                    chosen,
                    checked,
                    message,
                }),
                // The saved folder can no longer be used: say so, and keep
                // "use EmuWiz's own data folder" available.
                Err(detail) => Ok(StorageView {
                    available: None,
                    current: PathBuf::new(),
                    chosen: true,
                    checked,
                    message: message.or(Some((false, format!("The folder chosen for prepared copies cannot be used: {detail}.")))),
                }),
            })
        });
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
        relinkable: false,
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

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    eligible: &dyn Fn() -> Vec<recovery::BulkCandidate>,
) -> Option<Request> {
    state.poll();
    state.maintain(ui.ctx());
    let mut request = None;
    section_header(ui, "Game recovery", Some(INTRO));
    show_bulk(ui, state, eligible);
    show_storage(ui, state);
    let Some(target) = state.target.clone() else {
        banner(
            ui,
            "One game",
            "To work on a single game, open it and choose “Open game recovery” on its page.",
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

fn busy_line(ui: &mut egui::Ui, state: &State) {
    if let Some(busy) = state.busy {
        ui.horizontal_wrapped(|ui| {
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
        });
    }
}

/// Several games in one reviewed batch: find, review, one confirmation, then
/// one game at a time on the worker. Nothing is prepared that was not ticked.
fn show_bulk(
    ui: &mut egui::Ui,
    state: &mut State,
    eligible: &dyn Fn() -> Vec<recovery::BulkCandidate>,
) {
    let idle = state.job.is_none();
    let mut find = false;
    let mut start = false;
    let mut resume = false;
    let mut clear = false;
    egui::CollapsingHeader::new("Prepare several games")
        .id_salt("game-recovery-bulk")
        .show(ui, |ui| {
            ui.label(
                "EmuWiz lists the games it can prepare, you choose which, and it prepares them one at a time. Nothing starts until you confirm, and you can cancel at any point: finished games stay prepared and the rest can be continued later.",
            );
            if action_button(ui, "Find games EmuWiz can prepare", ActionStyle::Secondary, idle)
                .clicked()
            {
                find = true;
            }
            if let Some((ok, text)) = &state.bulk.message {
                banner(
                    ui,
                    if *ok { "Batch finished" } else { "Batch not finished" },
                    text,
                    if *ok { StatusTone::Success } else { StatusTone::Warning },
                );
            }
            if let Some(queue) = &state.bulk.queue {
                let failed: Vec<_> = queue
                    .items
                    .iter()
                    .filter_map(|item| match &item.state {
                        recovery::QueueState::Failed(why) => Some((&item.candidate.title, why)),
                        _ => None,
                    })
                    .collect();
                if !failed.is_empty() {
                    ui.collapsing(format!("Games that could not be prepared ({})", failed.len()), |ui| {
                        for (title, why) in failed {
                            ui.label(format!("• {title}: {why}"));
                        }
                    });
                }
                if queue.pending() > 0 {
                    ui.label(format!(
                        "An earlier batch still has {} game(s) to do.",
                        queue.pending()
                    ));
                    ui.horizontal_wrapped(|ui| {
                        if action_button(ui, "Continue the unfinished batch", ActionStyle::Primary, idle)
                            .clicked()
                        {
                            resume = true;
                        }
                        if action_button(ui, "Forget the unfinished batch", ActionStyle::Secondary, idle)
                            .clicked()
                        {
                            clear = true;
                        }
                    });
                }
            }
            match &state.bulk.plan {
                None => {}
                Some(Err(detail)) => {
                    banner(ui, "The games could not be listed", detail, StatusTone::Blocked);
                }
                Some(Ok(plan)) => {
                    let selected = &mut state.bulk.selected;
                    if let Some(note) = &state.bulk.note {
                        ui.label(note);
                    }
                    let refused = plan.count(|item| matches!(item, recovery::BulkItemPlan::Refused(_)));
                    ui.label(format!(
                        "{} game(s) can be prepared; {refused} cannot be prepared.",
                        plan.actionable().count()
                    ));
                    egui::ScrollArea::vertical()
                        .id_salt("game-recovery-bulk-list")
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for item in plan.actionable() {
                                let path = &item.candidate.path;
                                let mut ticked = selected.contains(path);
                                let what = match &item.plan {
                                    recovery::BulkItemPlan::Extract { files, bytes } => {
                                        format!("{files} new files, {}", human_bytes(*bytes))
                                    }
                                    _ => "matches an existing copy; nothing to extract".to_string(),
                                };
                                if ui
                                    .checkbox(&mut ticked, format!("{} — {what}", item.candidate.title))
                                    .changed()
                                {
                                    if ticked {
                                        selected.insert(path.clone());
                                    } else {
                                        selected.remove(path);
                                    }
                                }
                            }
                        });
                    if refused > 0 {
                        ui.collapsing(format!("Cannot be prepared ({refused})"), |ui| {
                            for item in &plan.items {
                                if let recovery::BulkItemPlan::Refused(why) = &item.plan {
                                    ui.label(format!("• {}: {why}", item.candidate.title));
                                }
                            }
                        });
                    }
                    let chosen = |path: &Path| selected.contains(path);
                    let needed = plan.required_for(&chosen);
                    let fits = plan.enough_space_for(&chosen) || needed == 0;
                    ui.label(format!(
                        "Selected: {} game(s). Space needed (with a safety margin): {}. Free in {}: {}.",
                        selected.len(),
                        human_bytes(needed),
                        plan.store.display(),
                        plan.available_bytes
                            .map_or("unknown".to_string(), human_bytes)
                    ));
                    if !fits {
                        banner(
                            ui,
                            "Not enough free space for this selection",
                            "Choose fewer games, free some space, or choose another folder for prepared copies below. Nothing has been started.",
                            StatusTone::Blocked,
                        );
                    }
                    ui.checkbox(
                        &mut state.bulk.confirm,
                        "Yes, prepare the selected games as new files. My game files stay as they are.",
                    );
                    if action_button(
                        ui,
                        "Prepare the selected games",
                        ActionStyle::Primary,
                        idle && state.bulk.confirm && fits && !selected.is_empty(),
                    )
                    .clicked()
                    {
                        start = true;
                    }
                }
            }
            busy_line(ui, state);
            if !idle && action_button(ui, "Cancel the batch", ActionStyle::Secondary, true).clicked() {
                state.cancel.store(true, Ordering::Relaxed);
            }
        });
    if find {
        state.plan_bulk(ui.ctx(), eligible());
    }
    if start {
        state.bulk.confirm = false;
        state.run_bulk(ui.ctx(), true);
    }
    if resume {
        state.run_bulk(ui.ctx(), false);
    }
    if clear {
        state.clear_bulk();
    }
}

/// Where prepared copies are kept, and choosing another folder for them.
fn show_storage(ui: &mut egui::Ui, state: &mut State) {
    let idle = state.job.is_none();
    let mut job: Option<(Option<PathBuf>, Option<Option<PathBuf>>)> = None;
    let opened = egui::CollapsingHeader::new("Where prepared copies are kept")
        .id_salt("game-recovery-storage")
        .show(ui, |ui| {
            ui.label(
                "A prepared copy is as large as the disc it came from (a CD game is up to about 700 MiB). You can keep the copies on another disk.",
            );
            match &state.storage {
                None => {
                    ui.label("Looking…");
                }
                Some(view) => {
                    if let Some((ok, text)) = &view.message {
                        banner(
                            ui,
                            if *ok { "Done" } else { "Not done" },
                            text,
                            if *ok { StatusTone::Success } else { StatusTone::Blocked },
                        );
                    }
                    ui.label(format!(
                        "Now: {}{}. Free there: {}.",
                        view.current.display(),
                        if view.chosen { " (your choice)" } else { " (EmuWiz's own data folder)" },
                        view.available.map_or("unknown".to_string(), human_bytes)
                    ));
                    if let Some(check) = &view.checked {
                        if check.usable() {
                            banner(
                                ui,
                                "This folder can be used",
                                &format!(
                                    "Free there: {}. {}",
                                    check.available_bytes.map_or("unknown".to_string(), human_bytes),
                                    check.notes.join(" ")
                                ),
                                StatusTone::Success,
                            );
                        } else {
                            banner(
                                ui,
                                "This folder cannot be used",
                                &check.problems.join(" "),
                                StatusTone::Blocked,
                            );
                        }
                    }
                }
            }
            ui.horizontal(|ui| {
                ui.label("Another folder:");
                ui.text_edit_singleline(&mut state.storage_folder);
            });
            let typed = PathBuf::from(state.storage_folder.trim());
            let checked_ok = state.storage.as_ref().is_some_and(|view| {
                view.checked
                    .as_ref()
                    .is_some_and(|check| check.usable() && check.folder == typed)
            });
            ui.checkbox(
                &mut state.storage_confirm,
                "Yes, keep new prepared copies in this folder. Copies already prepared elsewhere are not moved.",
            );
            ui.horizontal_wrapped(|ui| {
                if action_button(
                    ui,
                    "Check this folder",
                    ActionStyle::Secondary,
                    idle && !state.storage_folder.trim().is_empty(),
                )
                .clicked()
                {
                    job = Some((Some(typed.clone()), None));
                }
                if action_button(
                    ui,
                    "Use this folder",
                    ActionStyle::Primary,
                    idle && checked_ok && state.storage_confirm,
                )
                .clicked()
                {
                    job = Some((None, Some(Some(typed.clone()))));
                }
                if action_button(
                    ui,
                    "Use EmuWiz's own data folder",
                    ActionStyle::Secondary,
                    idle && state.storage.as_ref().is_some_and(|view| view.chosen),
                )
                .clicked()
                {
                    job = Some((None, Some(None)));
                }
            });
        });
    // The first time the section is opened, read the setting once.
    if opened.fully_open() && state.storage.is_none() && idle && job.is_none() {
        job = Some((None, None));
    }
    if let Some((check, apply)) = job {
        state.storage_confirm = false;
        state.storage_job(ui.ctx(), check, apply);
    }
}
