//! Native Blue "Mounts" (boards `D11-LG-Mounts`, `-MT*`): choose archives,
//! mount the ready queue, review active mounts and unmount them.
//!
//! Execution reuses the legacy engine unchanged: `run_mount_all_worker` and
//! `run_unmount_all_worker` (the same coordinators and sessions the Library
//! uses) and `perform_archive_action` for one archive. Live mount state comes
//! from the read-only snapshot loader. Archive files are never changed and
//! there is no Undo; every start is a confirmed request:
//! - the Mount queue and Unmount All ask for the exact count when more than
//!   25 archives are affected (`bulk_confirmation`, the shared gate);
//! - one archive is unmounted only after its own confirmation;
//! - Lazy Unmount is offered only after a normal unmount failed and asked
//!   for recovery, and runs only after a second, final confirmation.
//!
//! The page's decisions are plain methods returning a [`Job`], so they are
//! tested without mounting anything; only [`MountsState::spawn`] runs one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::{ArchiveRecord, MountState};
use eframe::egui::{self, RichText};

use crate::bulk_confirmation::{bulk_action_confirm_enabled, bulk_action_requires_typed_count};
use crate::mount_batch::{
    MountAllEvent, MountAllItem, MountAllProgress, MountAllResult, UnmountAllEvent, UnmountAllItem,
    UnmountAllProgress, UnmountAllResult, mount_all_items_for_paths, mount_row_matches,
    pending_unmount_items, prune_mount_queue, queued_pending_paths,
};
use crate::mount_operation_controller::{
    ArchiveAction, CleanupOutcome, LAZY_UNMOUNT_WARNING, NORMAL_UNMOUNT_FAILURE_SUMMARY,
    OperationResult,
};
use crate::ui::components::{self as widgets, ActionStyle, StatusTone};
use crate::ui::theme::{self, blue};

use super::simple_shell::palette;

pub(super) const NO_UNDO_NOTE: &str =
    "Mounting never changes archive files. Mounts and unmounts have no Undo.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum MountsTab {
    #[default]
    Choose,
    Active,
}

/// The one confirmation open at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Confirm {
    MountQueue {
        typed: String,
    },
    UnmountAll {
        typed: String,
    },
    Unmount(PathBuf),
    /// Step 1 of 2: a normal unmount failed and asked for recovery.
    LazyOffer {
        path: PathBuf,
        message: String,
    },
    /// Step 2 of 2: the final confirmation before Lazy Unmount runs.
    LazyFinal(PathBuf),
}

/// What a confirmation started. Only [`MountsState::spawn`] executes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Job {
    MountQueue(Vec<MountAllItem>),
    UnmountAll {
        items: Vec<UnmountAllItem>,
        cleanup: bool,
    },
    Single {
        action: ArchiveAction,
        path: PathBuf,
        cleanup: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LazyChoice {
    Cancel,
    TryNormalAgain,
    LazyUnmount,
}

enum Running {
    MountAll {
        receiver: Receiver<MountAllEvent>,
        stop: Arc<AtomicBool>,
        progress: MountAllProgress,
    },
    UnmountAll {
        receiver: Receiver<UnmountAllEvent>,
        stop: Arc<AtomicBool>,
        progress: UnmountAllProgress,
    },
    Single {
        action: ArchiveAction,
        path: PathBuf,
        receiver: Receiver<OperationResult>,
    },
}

#[derive(Clone, Debug)]
pub(super) enum Finished {
    MountAll(MountAllResult),
    UnmountAll(UnmountAllResult),
}

/// The live records and the configured mount folder.
type LoadResult = Result<(Vec<ArchiveRecord>, PathBuf), String>;

/// The engine entry points. Live by default; tests never spawn.
#[derive(Clone, Copy)]
pub(super) struct Workers {
    pub load: fn() -> LoadResult,
    pub mount_all: fn(Vec<MountAllItem>, &AtomicBool, &mut dyn FnMut(MountAllEvent)),
    pub unmount_all: fn(Vec<UnmountAllItem>, bool, &AtomicBool, &mut dyn FnMut(UnmountAllEvent)),
    pub single: fn(ArchiveAction, PathBuf, bool) -> OperationResult,
}

impl Workers {
    pub(super) fn live() -> Self {
        Self {
            load: || {
                archivefs_core::load_read_only_snapshot_default()
                    .map(|snapshot| (snapshot.records, snapshot.mount_root))
                    .map_err(|error| error.to_string())
            },
            mount_all: |items, stop, publish| {
                crate::mount_operations::run_mount_all_worker(items, stop, publish)
            },
            unmount_all: |items, cleanup, stop, publish| {
                crate::mount_operations::run_unmount_all_worker(items, cleanup, stop, publish)
            },
            single: |action, path, cleanup| {
                // Cleanup-started notices are not shown separately here.
                let (progress, _ignored) = mpsc::channel();
                crate::mount_operation_controller::perform_archive_action(
                    action, &path, cleanup, progress,
                )
            },
        }
    }
}

pub(super) struct MountsState {
    workers: Option<Workers>,
    pub(super) records: Option<Result<Vec<ArchiveRecord>, String>>,
    pub(super) mount_root: Option<PathBuf>,
    loading: Option<Receiver<LoadResult>>,
    pub(super) tab: MountsTab,
    pub(super) queue: Vec<PathBuf>,
    pub(super) search: String,
    pub(super) platform: Option<String>,
    pub(super) confirm: Option<Confirm>,
    pub(super) cleanup_after_unmount: bool,
    running: Option<Running>,
    pub(super) finished: Option<Finished>,
    /// (succeeded, message) of the last one-archive operation.
    pub(super) feedback: Option<(bool, String)>,
    /// Archives whose normal unmount failed and asked for recovery.
    pub(super) lazy_offers: HashSet<PathBuf>,
    /// This session's mount and unmount outcomes, newest first.
    pub(super) recent: Vec<String>,
}

impl Default for MountsState {
    /// Inert: never loads or runs anything (tests, previews).
    fn default() -> Self {
        Self {
            workers: None,
            records: None,
            mount_root: None,
            loading: None,
            tab: MountsTab::default(),
            queue: Vec::new(),
            search: String::new(),
            platform: None,
            confirm: None,
            cleanup_after_unmount: false,
            running: None,
            finished: None,
            feedback: None,
            lazy_offers: HashSet::new(),
            recent: Vec::new(),
        }
    }
}

impl MountsState {
    pub(super) fn live() -> Self {
        Self {
            workers: Some(Workers::live()),
            ..Self::default()
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.running.is_some()
    }

    fn records(&self) -> &[ArchiveRecord] {
        match &self.records {
            Some(Ok(records)) => records,
            _ => &[],
        }
    }

    /// The archives the Mount queue would attempt now: queued, still
    /// present and still ready to mount. Recomputed every time, so a
    /// confirmation never acts on a stale list.
    pub(super) fn queue_items(&self) -> Vec<MountAllItem> {
        let paths = queued_pending_paths(&self.queue, self.records());
        mount_all_items_for_paths(self.records(), &paths)
    }

    pub(super) fn mounted_items(&self) -> Vec<UnmountAllItem> {
        pending_unmount_items(self.records())
    }

    fn is_mounted(&self, path: &Path) -> bool {
        self.records().iter().any(|record| {
            record.mount_plan.archive.path == path && record.mount_state == MountState::Mounted
        })
    }

    pub(super) fn open_mount_queue(&mut self) {
        if !self.busy() && !self.queue_items().is_empty() {
            self.confirm = Some(Confirm::MountQueue {
                typed: String::new(),
            });
        }
    }

    pub(super) fn open_unmount_all(&mut self) {
        if !self.busy() && !self.mounted_items().is_empty() {
            self.confirm = Some(Confirm::UnmountAll {
                typed: String::new(),
            });
        }
    }

    pub(super) fn request_unmount(&mut self, path: PathBuf) {
        if !self.busy() && self.is_mounted(&path) {
            self.confirm = Some(Confirm::Unmount(path));
        }
    }

    /// Whether the open bulk confirmation may start (count, busy and, above
    /// 25, the exact typed count).
    pub(super) fn bulk_confirm_enabled(&self) -> bool {
        match &self.confirm {
            Some(Confirm::MountQueue { typed }) => {
                let count = self.queue_items().len();
                bulk_action_confirm_enabled(count, typed, !self.busy() && count > 0)
            }
            Some(Confirm::UnmountAll { typed }) => {
                let count = self.mounted_items().len();
                bulk_action_confirm_enabled(count, typed, !self.busy() && count > 0)
            }
            _ => false,
        }
    }

    /// Answers the open bulk confirmation. Starting requires the gate.
    pub(super) fn answer_bulk(&mut self, start: bool) -> Option<Job> {
        if !start {
            self.confirm = None;
            return None;
        }
        if !self.bulk_confirm_enabled() {
            return None;
        }
        let job = match self.confirm.take()? {
            Confirm::MountQueue { .. } => Job::MountQueue(self.queue_items()),
            Confirm::UnmountAll { .. } => Job::UnmountAll {
                items: self.mounted_items(),
                cleanup: self.cleanup_after_unmount,
            },
            other => {
                self.confirm = Some(other);
                return None;
            }
        };
        Some(job)
    }

    /// Answers a one-archive unmount confirmation.
    pub(super) fn answer_unmount(&mut self, start: bool) -> Option<Job> {
        let Some(Confirm::Unmount(path)) = self.confirm.clone() else {
            return None;
        };
        self.confirm = None;
        (start && !self.busy() && self.is_mounted(&path)).then(|| Job::Single {
            action: ArchiveAction::Unmount,
            path,
            cleanup: self.cleanup_after_unmount,
        })
    }

    /// Opens step 1 for an archive whose normal unmount asked for recovery.
    pub(super) fn offer_lazy(&mut self, path: PathBuf, message: String) {
        if self.lazy_offers.contains(&path) && !self.busy() {
            self.confirm = Some(Confirm::LazyOffer { path, message });
        }
    }

    /// Step 1 of Lazy Unmount. Choosing it only opens the final step.
    pub(super) fn answer_lazy_offer(&mut self, choice: LazyChoice) -> Option<Job> {
        let Some(Confirm::LazyOffer { path, .. }) = self.confirm.clone() else {
            return None;
        };
        self.confirm = None;
        match choice {
            LazyChoice::Cancel => None,
            LazyChoice::TryNormalAgain => (!self.busy()).then(|| Job::Single {
                action: ArchiveAction::Unmount,
                path,
                cleanup: self.cleanup_after_unmount,
            }),
            LazyChoice::LazyUnmount => {
                self.confirm = Some(Confirm::LazyFinal(path));
                None
            }
        }
    }

    /// Step 2 of Lazy Unmount: the only way it runs, and only while the
    /// recovery offer from a failed normal unmount still stands.
    pub(super) fn answer_lazy_final(&mut self, start: bool) -> Option<Job> {
        let Some(Confirm::LazyFinal(path)) = self.confirm.clone() else {
            return None;
        };
        self.confirm = None;
        (start && !self.busy() && self.lazy_offers.contains(&path)).then(|| Job::Single {
            action: ArchiveAction::LazyUnmount,
            path,
            cleanup: self.cleanup_after_unmount,
        })
    }

    pub(super) fn request_stop(&mut self) {
        match &mut self.running {
            Some(Running::MountAll { stop, progress, .. }) => {
                progress.stop_requested = true;
                stop.store(true, Ordering::Release);
            }
            Some(Running::UnmountAll { stop, progress, .. }) => {
                progress.stop_requested = true;
                stop.store(true, Ordering::Release);
            }
            _ => {}
        }
    }

    pub(super) fn refresh(&mut self, context: &egui::Context) {
        let Some(workers) = self.workers else {
            return;
        };
        if self.loading.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.loading = Some(receiver);
        let context = context.clone();
        std::thread::spawn(move || {
            let _ = sender.send((workers.load)());
            context.request_repaint();
        });
    }

    /// Runs a confirmed job on a worker thread. Refuses while busy.
    pub(super) fn spawn(&mut self, job: Job, context: &egui::Context) -> bool {
        let Some(workers) = self.workers else {
            return false;
        };
        if self.busy() {
            return false;
        }
        self.finished = None;
        self.feedback = None;
        let repaint = context.clone();
        match job {
            Job::MountQueue(items) => {
                let total = items.len();
                let (sender, receiver) = mpsc::channel();
                let stop = Arc::new(AtomicBool::new(false));
                let worker_stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    (workers.mount_all)(items, &worker_stop, &mut |event| {
                        let _ = sender.send(event);
                        repaint.request_repaint();
                    });
                });
                self.running = Some(Running::MountAll {
                    receiver,
                    stop,
                    progress: MountAllProgress {
                        total,
                        ..Default::default()
                    },
                });
            }
            Job::UnmountAll { items, cleanup } => {
                let total = items.len();
                let (sender, receiver) = mpsc::channel();
                let stop = Arc::new(AtomicBool::new(false));
                let worker_stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    (workers.unmount_all)(items, cleanup, &worker_stop, &mut |event| {
                        let _ = sender.send(event);
                        repaint.request_repaint();
                    });
                });
                self.running = Some(Running::UnmountAll {
                    receiver,
                    stop,
                    progress: UnmountAllProgress {
                        total,
                        ..Default::default()
                    },
                });
            }
            Job::Single {
                action,
                path,
                cleanup,
            } => {
                let (sender, receiver) = mpsc::channel();
                let worker_path = path.clone();
                std::thread::spawn(move || {
                    let _ = sender.send((workers.single)(action, worker_path, cleanup));
                    repaint.request_repaint();
                });
                self.running = Some(Running::Single {
                    action,
                    path,
                    receiver,
                });
            }
        }
        true
    }

    /// Takes finished loads and worker events. Refreshes after any change.
    pub(super) fn poll(&mut self, context: &egui::Context) {
        if self.records.is_none() && self.loading.is_none() {
            self.refresh(context);
        }
        if let Some(receiver) = &self.loading {
            match receiver.try_recv() {
                Ok(result) => {
                    self.records = Some(result.map(|(records, root)| {
                        self.mount_root = Some(root);
                        records
                    }));
                    self.loading = None;
                    let records = match &self.records {
                        Some(Ok(records)) => records.clone(),
                        _ => Vec::new(),
                    };
                    prune_mount_queue(&mut self.queue, &records);
                }
                Err(TryRecvError::Disconnected) => {
                    self.records = Some(Err("the mount state could not be read".into()));
                    self.loading = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut changed = false;
        match &mut self.running {
            Some(Running::MountAll {
                receiver, progress, ..
            }) => {
                let mut done = None;
                loop {
                    match receiver.try_recv() {
                        Ok(MountAllEvent::ArchiveStarted { index, total, item }) => {
                            progress.current_index = index;
                            progress.total = total;
                            progress.current_archive = Some(item.display_name);
                        }
                        Ok(MountAllEvent::ArchiveCompleted(_)) => progress.successful += 1,
                        Ok(MountAllEvent::ArchiveFailed { .. }) => progress.failed += 1,
                        Ok(MountAllEvent::ArchiveSkipped { item, .. }) => {
                            progress.skipped += 1;
                            progress.current_index =
                                progress.successful + progress.failed + progress.skipped;
                            progress.current_archive = Some(item.display_name);
                        }
                        Ok(MountAllEvent::Finished(result)) => done = Some(result),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            done.get_or_insert_with(|| {
                                MountAllResult::setup_failed(
                                    progress.total,
                                    "the Mount All worker stopped unexpectedly",
                                )
                            });
                            break;
                        }
                    }
                }
                if let Some(result) = done {
                    self.recent.insert(0, result.completion_message());
                    self.finished = Some(Finished::MountAll(result));
                    self.running = None;
                    changed = true;
                }
            }
            Some(Running::UnmountAll {
                receiver, progress, ..
            }) => {
                let mut done = None;
                loop {
                    match receiver.try_recv() {
                        Ok(UnmountAllEvent::ArchiveStarted { index, total, item }) => {
                            progress.current_index = index;
                            progress.total = total;
                            progress.current_archive = Some(item.display_name);
                        }
                        Ok(UnmountAllEvent::ArchiveCompleted(item)) => {
                            progress.successful += 1;
                            self.lazy_offers.remove(&item.archive_path);
                        }
                        Ok(UnmountAllEvent::ArchiveFailed {
                            item,
                            offer_lazy_unmount,
                            ..
                        }) => {
                            progress.failed += 1;
                            if offer_lazy_unmount {
                                self.lazy_offers.insert(item.archive_path);
                            }
                        }
                        Ok(UnmountAllEvent::ArchiveSkipped { item, .. }) => {
                            progress.skipped += 1;
                            self.lazy_offers.remove(&item.archive_path);
                        }
                        Ok(UnmountAllEvent::CleanupCompleted(_)) => progress.cleanup_successes += 1,
                        Ok(UnmountAllEvent::CleanupFailed { .. }) => progress.cleanup_failures += 1,
                        Ok(UnmountAllEvent::CleanupStarted(_)) => {}
                        Ok(UnmountAllEvent::Finished(result)) => done = Some(result),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            done.get_or_insert_with(|| {
                                UnmountAllResult::setup_failed(
                                    progress.total,
                                    "the Unmount All worker stopped unexpectedly",
                                )
                            });
                            break;
                        }
                    }
                }
                if let Some(result) = done {
                    self.recent.insert(0, result.completion_message());
                    self.finished = Some(Finished::UnmountAll(result));
                    self.running = None;
                    changed = true;
                }
            }
            Some(Running::Single {
                action,
                path,
                receiver,
            }) => {
                let result = match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => {
                        Some(Err(crate::mount_operation_controller::OperationFailure {
                            message: "the archive operation stopped unexpectedly".into(),
                            offer_lazy_unmount: false,
                        }))
                    }
                };
                if let Some(result) = result {
                    let (action, path) = (*action, path.clone());
                    self.running = None;
                    self.single_finished(action, path, result);
                    changed = true;
                }
            }
            None => {}
        }
        if changed {
            self.refresh(context);
        }
    }

    /// Records a one-archive outcome; a recoverable normal-unmount failure
    /// opens step 1 of Lazy Unmount.
    pub(super) fn single_finished(
        &mut self,
        action: ArchiveAction,
        path: PathBuf,
        result: OperationResult,
    ) {
        match result {
            Ok(success) => {
                let mut message = success.message;
                if let Some(cleanup) = &success.cleanup {
                    message.push_str("\n");
                    message.push_str(match cleanup {
                        CleanupOutcome::Completed { message, .. }
                        | CleanupOutcome::Failed { message, .. } => message,
                    });
                }
                if let Some(warning) = success.warning {
                    message.push_str("\n");
                    message.push_str(&warning);
                }
                self.lazy_offers.remove(&path);
                self.recent
                    .insert(0, message.lines().next().unwrap_or("").to_string());
                self.feedback = Some((true, message));
            }
            Err(failure) => {
                let recoverable = action == ArchiveAction::Unmount && failure.offer_lazy_unmount;
                self.recent
                    .insert(0, format!("{}: {}", path.display(), failure.message));
                if recoverable {
                    self.lazy_offers.insert(path.clone());
                    self.feedback = Some((false, NORMAL_UNMOUNT_FAILURE_SUMMARY.to_string()));
                    self.offer_lazy(path, failure.message);
                } else {
                    self.feedback = Some((false, failure.message));
                }
            }
        }
        self.recent.truncate(20);
    }
}

/// What the page asks the app to do beyond its own state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PageRequest {
    Run(Job),
    OpenInLibrary(PathBuf),
    OpenSources,
}

/// Renders the page. Returns at most one request for the app.
pub(super) fn show(ui: &mut egui::Ui, state: &mut MountsState) -> Option<PageRequest> {
    let mut request = None;
    tabs(ui, state);
    ui.add_space(theme::SPACE_SM);
    if let Some((succeeded, message)) = state.feedback.clone() {
        widgets::banner(
            ui,
            if succeeded {
                "Completed"
            } else {
                "Not completed"
            },
            &message,
            if succeeded {
                StatusTone::Success
            } else {
                StatusTone::Warning
            },
        );
        ui.add_space(theme::SPACE_SM);
    }
    if running_view(ui, state) {
        return None;
    }
    if let Some(finished) = state.finished.clone() {
        if let Some(found) = result_view(ui, state, &finished) {
            request = Some(found);
        }
        ui.add_space(theme::SPACE_SM);
    }
    match &state.records {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the current mount state…");
            });
            return request;
        }
        Some(Err(error)) => {
            widgets::banner(
                ui,
                "Mount state unavailable",
                &format!("{error}. Nothing was mounted or unmounted."),
                StatusTone::Warning,
            );
            return request;
        }
        Some(Ok(_)) => {}
    }
    let found = match state.tab {
        MountsTab::Choose => choose_view(ui, state),
        MountsTab::Active => active_view(ui, state),
    };
    request = request.or(found);
    request.or_else(|| dialogs(ui.ctx(), state))
}

fn tabs(ui: &mut egui::Ui, state: &mut MountsState) {
    let active = state.mounted_items().len();
    ui.horizontal_wrapped(|ui| {
        for (tab, label) in [
            (MountsTab::Choose, "Choose archives".to_string()),
            (MountsTab::Active, format!("Active mounts ({active})")),
        ] {
            if ui
                .add(egui::Button::selectable(state.tab == tab, label))
                .clicked()
            {
                state.tab = tab;
            }
        }
    });
}

fn running_view(ui: &mut egui::Ui, state: &mut MountsState) -> bool {
    let (title, current, total, note, stop_requested, can_stop) = match &state.running {
        None => return false,
        Some(Running::MountAll { progress, .. }) => (
            format!("Mounting {} of {}", progress.current_index, progress.total),
            progress.current_archive.clone(),
            progress.total,
            progress.current_index,
            progress.stop_requested,
            true,
        ),
        Some(Running::UnmountAll { progress, .. }) => (
            format!(
                "Unmounting {} of {}",
                progress.current_index, progress.total
            ),
            progress.current_archive.clone(),
            progress.total,
            progress.current_index,
            progress.stop_requested,
            true,
        ),
        Some(Running::Single { action, path, .. }) => (
            match action {
                ArchiveAction::Mount => "Mounting".to_string(),
                ArchiveAction::Unmount => "Unmounting".to_string(),
                ArchiveAction::LazyUnmount => "Lazy unmounting".to_string(),
                ArchiveAction::Remount => "Remounting".to_string(),
            },
            Some(path.display().to_string()),
            1,
            0,
            false,
            false,
        ),
    };
    let mut stop = false;
    super::game_detail_blue::hero_card(ui, |ui| {
        ui.label(theme::section_title(title));
        if let Some(current) = current {
            ui.label(RichText::new(current).color(blue::BODY_TEXT));
        }
        if total > 0 {
            // Counts archives, never a data percentage.
            ui.add(
                egui::ProgressBar::new((note as f32 / total as f32).clamp(0.0, 1.0))
                    .desired_width(ui.available_width().min(480.0)),
            );
        }
        ui.label(
            RichText::new(
                "Archives are handled one at a time. A failure is recorded and later archives are still attempted. Progress here counts archives, not data.",
            )
            .color(palette::SECONDARY_TEXT),
        );
        if can_stop
            && widgets::action_button(
                ui,
                if stop_requested {
                    "Stop requested"
                } else {
                    "Stop After Current Archive"
                },
                ActionStyle::Secondary,
                !stop_requested,
            )
            .clicked()
        {
            stop = true;
        }
    });
    if stop {
        state.request_stop();
    }
    true
}

fn result_view(
    ui: &mut egui::Ui,
    state: &mut MountsState,
    finished: &Finished,
) -> Option<PageRequest> {
    let mut request = None;
    let (message, rows, failures): (String, Vec<(&str, usize)>, Vec<(PathBuf, String, bool)>) =
        match finished {
            Finished::MountAll(result) => (
                result.completion_message(),
                vec![
                    ("Attempted", result.attempted()),
                    ("Successful", result.successful),
                    ("Failed", result.failed()),
                    ("Skipped", result.skipped()),
                    ("Not attempted", result.unattempted),
                ],
                result
                    .failures
                    .iter()
                    .map(|failure| (failure.archive_path.clone(), failure.message.clone(), false))
                    .collect(),
            ),
            Finished::UnmountAll(result) => (
                result.completion_message(),
                vec![
                    ("Attempted", result.attempted()),
                    ("Successful", result.successful),
                    ("Failed", result.failures.len()),
                    ("Skipped", result.skipped.len()),
                    ("Not attempted", result.unattempted),
                    ("Cleanup successful", result.cleanup_successes),
                    ("Cleanup failed", result.cleanup_failures.len()),
                ],
                result
                    .failures
                    .iter()
                    .map(|failure| {
                        (
                            failure.archive_path.clone(),
                            failure.message.clone(),
                            failure.offer_lazy_unmount,
                        )
                    })
                    .collect(),
            ),
        };
    let setup = match finished {
        Finished::MountAll(result) => result.setup_failure.clone(),
        Finished::UnmountAll(result) => result.setup_failure.clone(),
    };
    let clean = failures.is_empty() && setup.is_none();
    widgets::banner(
        ui,
        &message,
        &setup.map_or_else(
            || {
                if clean {
                    "Archive files were not changed.".to_string()
                } else {
                    "Failed archives keep their current state; nothing was force-closed."
                        .to_string()
                }
            },
            |error| format!("Setup error: {error}"),
        ),
        if clean {
            StatusTone::Success
        } else {
            StatusTone::Warning
        },
    );
    ui.add_space(6.0);
    for (label, value) in rows {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(170.0, 0.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_width(170.0);
                    ui.label(RichText::new(label).color(blue::DIM_TEXT));
                },
            );
            ui.label(value.to_string());
        });
    }
    if !failures.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new("FAILED ARCHIVES")
                .strong()
                .color(blue::DIM_TEXT),
        );
        for (path, message, lazy) in failures {
            row_frame(ui, |ui| {
                ui.label(
                    RichText::new(file_name(&path))
                        .strong()
                        .color(blue::PRIMARY_TEXT),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(if lazy {
                            format!("{message} — individual Lazy Unmount recovery available")
                        } else {
                            message.clone()
                        })
                        .color(palette::SECONDARY_TEXT),
                    )
                    .wrap(),
                );
                ui.horizontal_wrapped(|ui| {
                    let unmount_failure = matches!(finished, Finished::UnmountAll(_));
                    if unmount_failure
                        && widgets::action_button(
                            ui,
                            "Try again",
                            ActionStyle::Secondary,
                            !state.busy(),
                        )
                        .clicked()
                    {
                        state.request_unmount(path.clone());
                    }
                    if lazy
                        && state.lazy_offers.contains(&path)
                        && widgets::action_button(
                            ui,
                            "Lazy Unmount…",
                            ActionStyle::Secondary,
                            !state.busy(),
                        )
                        .clicked()
                    {
                        state.offer_lazy(path.clone(), message.clone());
                    }
                });
            });
        }
    }
    if ui.button("Dismiss result").clicked() {
        state.finished = None;
    }
    let _ = &mut request;
    request
}

fn choose_view(ui: &mut egui::Ui, state: &mut MountsState) -> Option<PageRequest> {
    let records = state.records().to_vec();
    if records.is_empty() {
        let open = widgets::empty_state(
            ui,
            "No archives found",
            "Add and scan a source folder before creating a mount queue.",
            Some("Open Sources"),
        );
        return open.then_some(PageRequest::OpenSources);
    }
    let attempted = state.queue_items().len();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(if state.queue.len() == 1 {
                "1 archive queued.".to_string()
            } else {
                format!("{} archives queued.", state.queue.len())
            })
            .strong(),
        );
    });
    let visible: Vec<&ArchiveRecord> = records
        .iter()
        .filter(|record| {
            state.platform.as_deref().is_none_or(|wanted| {
                record.identity.platform.as_deref().unwrap_or("Unknown") == wanted
            }) && mount_row_matches(record, &state.search)
        })
        .collect();
    ui.horizontal_wrapped(|ui| {
        let any_unqueued = visible.iter().any(|record| {
            record.mount_state == MountState::Pending
                && !state.queue.contains(&record.mount_plan.archive.path)
        });
        if widgets::action_button(
            ui,
            "Queue all visible",
            ActionStyle::Secondary,
            any_unqueued,
        )
        .clicked()
        {
            for record in &visible {
                let path = &record.mount_plan.archive.path;
                if record.mount_state == MountState::Pending && !state.queue.contains(path) {
                    state.queue.push(path.clone());
                }
            }
        }
        if widgets::action_button(
            ui,
            "Clear queue",
            ActionStyle::Secondary,
            !state.queue.is_empty(),
        )
        .clicked()
        {
            state.queue.clear();
        }
        if widgets::action_button(
            ui,
            format!("Mount queue ({attempted})"),
            ActionStyle::Primary,
            !state.busy() && attempted > 0,
        )
        .on_disabled_hover_text("Queue at least one archive that is ready to mount.")
        .clicked()
        {
            state.open_mount_queue();
        }
    });
    ui.add_space(4.0);
    let counts = crate::cheats_mods::detected_platform_counts(
        records
            .iter()
            .map(|record| record.identity.platform.as_deref()),
    );
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(egui::Button::selectable(state.platform.is_none(), "All"))
            .clicked()
        {
            state.platform = None;
        }
        for (name, count) in &counts.named {
            if ui
                .add(egui::Button::selectable(
                    state.platform.as_deref() == Some(name.as_str()),
                    format!("{name} ({count})"),
                ))
                .clicked()
            {
                state.platform = Some(name.clone());
            }
        }
        if counts.unknown > 0
            && ui
                .add(egui::Button::selectable(
                    state.platform.as_deref() == Some("Unknown"),
                    format!("Unknown ({})", counts.unknown),
                ))
                .clicked()
        {
            state.platform = Some("Unknown".into());
        }
        ui.add(
            egui::TextEdit::singleline(&mut state.search)
                .hint_text("Search archives")
                .desired_width(220.0),
        );
    });
    ui.add_space(6.0);
    let wide = ui.available_width() >= 1100.0;
    let mut toggle = None;
    let mut list = |ui: &mut egui::Ui| {
        if visible.is_empty() {
            widgets::empty_state(
                ui,
                "No matching archives",
                "Change or clear the search to see available archives.",
                None,
            );
        }
        for record in &visible {
            let path = record.mount_plan.archive.path.clone();
            let queued = state.queue.contains(&path);
            ui.push_id(("v2_mount_row", &path), |ui| {
                row_frame(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        let button_width = 170.0_f32.min(ui.available_width() * 0.35);
                        let text_width = (ui.available_width() - button_width - 12.0).max(140.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(text_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new(&record.identity.display_name)
                                            .strong()
                                            .color(blue::PRIMARY_TEXT),
                                    );
                                    let tone = match record.mount_state {
                                        MountState::Pending => StatusTone::Success,
                                        MountState::Mounted => StatusTone::Info,
                                        MountState::MountPathExists => StatusTone::Warning,
                                        MountState::NotMountable => StatusTone::Pending,
                                    };
                                    widgets::status_badge(
                                        ui,
                                        crate::cheats_mods::mount_validation_label(
                                            record.mount_state,
                                        ),
                                        tone,
                                    );
                                });
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!(
                                            "Destination {}",
                                            record.mount_plan.mount_path.display()
                                        ))
                                        .monospace()
                                        .size(12.5)
                                        .color(palette::SECONDARY_TEXT),
                                    )
                                    .wrap(),
                                );
                            },
                        );
                        ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                            let ready = record.mount_state == MountState::Pending;
                            if queued {
                                if widgets::action_button(
                                    ui,
                                    "Remove",
                                    ActionStyle::Secondary,
                                    true,
                                )
                                .clicked()
                                {
                                    toggle = Some((path.clone(), false));
                                }
                            } else {
                                if widgets::action_button(
                                    ui,
                                    "Add to queue",
                                    ActionStyle::Secondary,
                                    ready,
                                )
                                .clicked()
                                {
                                    toggle = Some((path.clone(), true));
                                }
                                if !ready {
                                    ui.label(
                                        RichText::new("Not ready to mount.")
                                            .size(12.5)
                                            .color(palette::SECONDARY_TEXT),
                                    );
                                }
                            }
                        });
                    });
                });
            });
        }
    };
    if wide {
        let gap = 20.0;
        let right = (ui.available_width() * 0.3).clamp(340.0, 520.0);
        let left = ui.available_width() - right - gap;
        let queued = state.queue.len();
        let skipped = queued.saturating_sub(attempted);
        let mount_root = state.mount_root.clone();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                egui::vec2(left, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left);
                    list(ui);
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(right, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right);
                    queue_panels(ui, queued, attempted, skipped, mount_root.as_deref());
                },
            );
        });
    } else {
        list(ui);
    }
    if let Some((path, add)) = toggle {
        if add {
            if !state.queue.contains(&path) {
                state.queue.push(path);
            }
        } else {
            state.queue.retain(|queued| *queued != path);
        }
    }
    None
}

fn queue_panels(
    ui: &mut egui::Ui,
    queued: usize,
    attempted: usize,
    skipped: usize,
    mount_root: Option<&Path>,
) {
    super::game_detail_blue::side_card(ui, "MOUNT QUEUE", |ui| {
        ui.label(
            RichText::new(if queued == 1 {
                "1 archive queued".to_string()
            } else {
                format!("{queued} archives queued")
            })
            .strong()
            .color(blue::PRIMARY_TEXT),
        );
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Only archives that are ready are attempted. Already-mounted archives and existing destinations are skipped. Archive files are not changed.",
                )
                .color(blue::BODY_TEXT),
            )
            .wrap(),
        );
        ui.add_space(6.0);
        for (label, value) in [
            (
                "Mount folder",
                mount_root.map_or_else(
                    || "Not known yet".to_string(),
                    |root| root.display().to_string(),
                ),
            ),
            ("Ready", format!("{attempted} of {queued}")),
            ("Skipped", skipped.to_string()),
        ] {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(120.0, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(120.0);
                        ui.label(RichText::new(label).color(blue::DIM_TEXT));
                    },
                );
                ui.add(egui::Label::new(RichText::new(value).color(blue::BODY_TEXT)).wrap());
            });
        }
    });
    super::game_detail_blue::side_card(ui, "LARGE QUEUES", |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "A queue of more than 25 archives asks you to type the exact count first. Unmount All does the same.",
                )
                .color(blue::BODY_TEXT),
            )
            .wrap(),
        );
        ui.label(RichText::new(NO_UNDO_NOTE).color(palette::SECONDARY_TEXT));
    });
}

fn active_view(ui: &mut egui::Ui, state: &mut MountsState) -> Option<PageRequest> {
    let mut request = None;
    let mounted = state.mounted_items();
    // A confirmation for an archive that is no longer mounted is dropped.
    if let Some(Confirm::Unmount(path)) = &state.confirm
        && !mounted.iter().any(|item| &item.archive_path == path)
    {
        state.confirm = None;
    }
    ui.horizontal_wrapped(|ui| {
        widgets::status_badge(
            ui,
            format!("{} active", mounted.len()),
            if mounted.is_empty() {
                StatusTone::Pending
            } else {
                StatusTone::Info
            },
        );
        if widgets::action_button(ui, "Refresh", ActionStyle::Secondary, !state.busy()).clicked() {
            state.records = None;
        }
        ui.add_enabled_ui(!state.busy(), |ui| {
            ui.checkbox(
                &mut state.cleanup_after_unmount,
                "Clean empty mount directories after unmount",
            );
        });
        if widgets::action_button(
            ui,
            format!("Unmount All ({})", mounted.len()),
            ActionStyle::Secondary,
            !state.busy() && !mounted.is_empty(),
        )
        .clicked()
        {
            state.open_unmount_all();
        }
    });
    ui.add_space(6.0);
    if mounted.is_empty() {
        widgets::empty_state(
            ui,
            "Nothing is mounted",
            "Mounted archives appear here with their destinations and unmount controls.",
            None,
        );
    }
    let mut unmount_job = None;
    for item in &mounted {
        ui.push_id(("v2_active_mount", &item.archive_path), |ui| {
            row_frame(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(&item.display_name)
                            .strong()
                            .color(blue::PRIMARY_TEXT),
                    );
                    widgets::status_badge(ui, "Mounted", StatusTone::Success);
                    if widgets::action_button(ui, "Open in Library", ActionStyle::Secondary, true)
                        .clicked()
                    {
                        request = Some(PageRequest::OpenInLibrary(item.archive_path.clone()));
                    }
                    if widgets::action_button(ui, "Unmount", ActionStyle::Secondary, !state.busy())
                        .clicked()
                    {
                        state.request_unmount(item.archive_path.clone());
                    }
                    if state.lazy_offers.contains(&item.archive_path)
                        && widgets::action_button(ui, "Lazy Unmount…", ActionStyle::Secondary, !state.busy())
                            .clicked()
                    {
                        state.offer_lazy(
                            item.archive_path.clone(),
                            "Normal unmount failed for this archive.".into(),
                        );
                    }
                });
                for (label, path) in [("Destination", &item.mount_path), ("Archive", &item.archive_path)] {
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(110.0, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(110.0);
                                ui.label(RichText::new(label).color(blue::DIM_TEXT));
                            },
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(path.display().to_string())
                                    .monospace()
                                    .size(12.5)
                                    .color(blue::BODY_TEXT),
                            )
                            .wrap(),
                        );
                    });
                }
                // The inline confirmation, as on the board.
                if state.confirm == Some(Confirm::Unmount(item.archive_path.clone())) {
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(blue::SELECTED_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, blue::SELECTED_OUTLINE))
                        .corner_radius(10)
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                widgets::status_badge(ui, "Confirmation", StatusTone::Info);
                                ui.label(
                                    RichText::new(format!("Unmount {}?", item.display_name))
                                        .strong(),
                                );
                            });
                            ui.label(
                                "Close applications using this mount before unmounting. The archive file is not changed.",
                            );
                            ui.horizontal(|ui| {
                                let cancel = ui.button("Cancel");
                                focus_once(ui, cancel.id, &cancel);
                                if cancel.clicked() {
                                    let _ = state.answer_unmount(false);
                                }
                                if widgets::action_button(ui, "Unmount now", ActionStyle::Primary, !state.busy())
                                    .clicked()
                                {
                                    unmount_job = state.answer_unmount(true);
                                }
                            });
                        });
                }
            });
        });
    }
    if let Some(job) = unmount_job {
        request = Some(PageRequest::Run(job));
    }
    ui.add_space(theme::SPACE_SM);
    ui.label(
        RichText::new("RECENT ACTIVITY")
            .strong()
            .color(blue::DIM_TEXT),
    );
    if state.recent.is_empty() {
        ui.label(
            RichText::new("No mount or unmount activity has been recorded in this session.")
                .color(palette::SECONDARY_TEXT),
        );
    }
    for line in state.recent.iter().take(8) {
        ui.label(RichText::new(line).color(blue::BODY_TEXT));
    }
    ui.label(RichText::new(NO_UNDO_NOTE).color(palette::SECONDARY_TEXT));
    request
}

/// Moves keyboard focus to `response` the first frame it is shown.
fn focus_once(ui: &egui::Ui, id: egui::Id, response: &egui::Response) {
    let key = id.with("v2_mounts_focused");
    if !ui
        .ctx()
        .data(|data| data.get_temp::<bool>(key).unwrap_or(false))
    {
        response.request_focus();
        ui.ctx().data_mut(|data| data.insert_temp(key, true));
    }
}

fn dialogs(context: &egui::Context, state: &mut MountsState) -> Option<PageRequest> {
    let confirm = state.confirm.clone()?;
    let mut job = None;
    match confirm {
        Confirm::MountQueue { .. } | Confirm::UnmountAll { .. } => {
            let mount = matches!(confirm, Confirm::MountQueue { .. });
            let count = if mount {
                state.queue_items().len()
            } else {
                state.mounted_items().len()
            };
            let id = egui::Id::new(("v2_mounts_bulk", mount));
            let mut answer = None;
            let modal = egui::Modal::new(id).show(context, |ui| {
                ui.set_max_width(600.0);
                ui.heading(if mount {
                    format!("Mount {count} queued archive{}?", if count == 1 { "" } else { "s" })
                } else {
                    "Unmount All mounted archives?".to_string()
                });
                ui.add(
                    egui::Label::new(if mount {
                        "Only archives that are ready to mount are attempted. Already-mounted archives and existing destinations are skipped. Archive files are not changed.".to_string()
                    } else {
                        format!(
                            "{count} mounted archive{} will be unmounted one at a time. Close emulators, file managers, terminals, media players and other applications using mounted files; open files can stop a normal unmount. Cleanup after each successful unmount: {}.",
                            if count == 1 { "" } else { "s" },
                            if state.cleanup_after_unmount { "enabled" } else { "off" }
                        )
                    })
                    .wrap(),
                );
                if bulk_action_requires_typed_count(count) {
                    ui.label(format!(
                        "This affects more than 25 items. Type the exact count ({count}) to confirm."
                    ));
                    if let Some(Confirm::MountQueue { typed } | Confirm::UnmountAll { typed }) =
                        state.confirm.as_mut()
                    {
                        ui.add(
                            egui::TextEdit::singleline(typed)
                                .desired_width(160.0)
                                .hint_text(count.to_string()),
                        );
                    }
                }
                let enabled = state.bulk_confirm_enabled();
                ui.horizontal(|ui| {
                    let cancel = ui.button("Cancel");
                    focus_once(ui, id, &cancel);
                    if cancel.clicked() {
                        answer = Some(false);
                    }
                    if widgets::action_button(
                        ui,
                        if mount { "Mount now" } else { "Unmount All" },
                        ActionStyle::Primary,
                        enabled,
                    )
                    .clicked()
                    {
                        answer = Some(true);
                    }
                });
                if !enabled && bulk_action_requires_typed_count(count) {
                    ui.label(
                        RichText::new(format!("Type {count} to enable."))
                            .color(palette::SECONDARY_TEXT),
                    );
                }
            });
            if modal.should_close() && answer.is_none() {
                answer = Some(false);
            }
            if let Some(answer) = answer {
                job = state.answer_bulk(answer);
            }
        }
        Confirm::LazyOffer { message, .. } => {
            let id = egui::Id::new("v2_mounts_lazy_offer");
            let mut answer = None;
            let modal = egui::Modal::new(id).show(context, |ui| {
                ui.set_max_width(640.0);
                ui.heading("Use Lazy Unmount?");
                ui.add(egui::Label::new(format!("Normal unmount failed: {message}")).wrap());
                ui.add(egui::Label::new(LAZY_UNMOUNT_WARNING).wrap());
                ui.horizontal_wrapped(|ui| {
                    let cancel = ui.button("Cancel");
                    focus_once(ui, id, &cancel);
                    if cancel.clicked() {
                        answer = Some(LazyChoice::Cancel);
                    }
                    if ui.button("Try Normal Unmount Again").clicked() {
                        answer = Some(LazyChoice::TryNormalAgain);
                    }
                    if ui.button("Lazy Unmount…").clicked() {
                        answer = Some(LazyChoice::LazyUnmount);
                    }
                });
            });
            if modal.should_close() && answer.is_none() {
                answer = Some(LazyChoice::Cancel);
            }
            if let Some(answer) = answer {
                job = state.answer_lazy_offer(answer);
            }
        }
        Confirm::LazyFinal(_) => {
            let id = egui::Id::new("v2_mounts_lazy_final");
            let mut answer = None;
            let modal = egui::Modal::new(id).show(context, |ui| {
                ui.set_max_width(600.0);
                ui.heading("Confirm Lazy Unmount");
                ui.label("This is the final confirmation. Close applications using this mount before continuing. The archive file is not changed.");
                ui.horizontal(|ui| {
                    let cancel = ui.button("Cancel");
                    focus_once(ui, id, &cancel);
                    if cancel.clicked() {
                        answer = Some(false);
                    }
                    if widgets::action_button(ui, "Confirm Lazy Unmount", ActionStyle::Primary, true)
                        .clicked()
                    {
                        answer = Some(true);
                    }
                });
            });
            if modal.should_close() && answer.is_none() {
                answer = Some(false);
            }
            if let Some(answer) = answer {
                job = state.answer_lazy_final(answer);
            }
        }
        // Rendered inline on its row.
        Confirm::Unmount(_) => {}
    }
    job.map(PageRequest::Run)
}

fn row_frame(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(blue::CARD_SURFACE_TOP)
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    ui.add_space(6.0);
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::{Archive, ArchiveHealth, ArchiveMetadata, MountPlan};

    fn record(path: &str, state: MountState) -> ArchiveRecord {
        let archive = Archive::from_path(path).unwrap();
        let name = Path::new(path)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        ArchiveRecord::new(
            MountPlan::new(archive, PathBuf::from(format!("/mnt/emuwiz/{name}"))),
            state,
            ArchiveMetadata {
                title: None,
                platform: None,
                region: None,
                languages: None,
                version: None,
                disc: None,
                publisher: None,
                developer: None,
                release_year: None,
                genre: None,
                notes: None,
                source: None,
                synopsis: None,
                players: None,
                rating: None,
            },
            ArchiveHealth::Pending,
        )
    }

    fn with(records: Vec<ArchiveRecord>) -> MountsState {
        MountsState {
            records: Some(Ok(records)),
            ..MountsState::default()
        }
    }

    fn pending(count: usize) -> MountsState {
        let records: Vec<_> = (0..count)
            .map(|index| record(&format!("/roms/g{index:03}.zip"), MountState::Pending))
            .collect();
        let mut state = with(records);
        state.queue = state
            .records()
            .iter()
            .map(|record| record.mount_plan.archive.path.clone())
            .collect();
        state
    }

    #[test]
    fn a_queue_of_25_needs_no_typed_count_but_26_needs_the_exact_count() {
        let mut state = pending(25);
        state.open_mount_queue();
        assert!(state.bulk_confirm_enabled());
        match state.answer_bulk(true) {
            Some(Job::MountQueue(items)) => assert_eq!(items.len(), 25),
            other => panic!("{other:?}"),
        }

        let mut state = pending(26);
        state.open_mount_queue();
        assert!(!state.bulk_confirm_enabled());
        assert_eq!(state.answer_bulk(true), None, "no start without the count");
        for wrong in ["25", "2 6", "026", "26.", "-26"] {
            state.confirm = Some(Confirm::MountQueue {
                typed: wrong.into(),
            });
            assert_eq!(state.answer_bulk(true), None, "{wrong:?} must not start");
        }
        state.confirm = Some(Confirm::MountQueue {
            typed: " 26 ".into(),
        });
        match state.answer_bulk(true) {
            Some(Job::MountQueue(items)) => assert_eq!(items.len(), 26),
            other => panic!("{other:?}"),
        }
        assert_eq!(state.confirm, None);
    }

    #[test]
    fn unmount_all_uses_the_same_gate_and_the_cleanup_choice() {
        let records: Vec<_> = (0..26)
            .map(|index| record(&format!("/roms/m{index:03}.zip"), MountState::Mounted))
            .collect();
        let mut state = with(records);
        state.cleanup_after_unmount = true;
        state.open_unmount_all();
        assert_eq!(state.answer_bulk(true), None);
        state.confirm = Some(Confirm::UnmountAll { typed: "26".into() });
        match state.answer_bulk(true) {
            Some(Job::UnmountAll { items, cleanup }) => {
                assert_eq!(items.len(), 26);
                assert!(cleanup);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_queue_attempts_only_archives_still_ready_to_mount() {
        let mut state = with(vec![
            record("/roms/a.zip", MountState::Pending),
            record("/roms/b.zip", MountState::Mounted),
            record("/roms/c.zip", MountState::MountPathExists),
        ]);
        state.queue = vec![
            PathBuf::from("/roms/a.zip"),
            PathBuf::from("/roms/b.zip"),
            PathBuf::from("/roms/c.zip"),
            PathBuf::from("/roms/gone.zip"),
        ];
        let items = state.queue_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].archive_path, PathBuf::from("/roms/a.zip"));
        // Cancelling never starts anything.
        state.open_mount_queue();
        assert_eq!(state.answer_bulk(false), None);
        assert_eq!(state.confirm, None);
    }

    #[test]
    fn lazy_unmount_runs_only_after_a_recoverable_failure_and_two_confirmations() {
        let path = PathBuf::from("/roms/busy.zip");
        let mut state = with(vec![record("/roms/busy.zip", MountState::Mounted)]);
        // Without a failed normal unmount there is no offer at all.
        state.offer_lazy(path.clone(), "busy".into());
        assert_eq!(state.confirm, None);
        state.confirm = Some(Confirm::LazyFinal(path.clone()));
        assert_eq!(
            state.answer_lazy_final(true),
            None,
            "no offer, no lazy unmount"
        );

        // A normal unmount that fails and asks for recovery opens step 1.
        state.request_unmount(path.clone());
        assert_eq!(
            state.answer_unmount(true),
            Some(Job::Single {
                action: ArchiveAction::Unmount,
                path: path.clone(),
                cleanup: false,
            })
        );
        state.single_finished(
            ArchiveAction::Unmount,
            path.clone(),
            Err(crate::mount_operation_controller::OperationFailure {
                message: "target is busy".into(),
                offer_lazy_unmount: true,
            }),
        );
        assert!(matches!(state.confirm, Some(Confirm::LazyOffer { .. })));
        // Step 1: choosing Lazy Unmount only opens the final step.
        assert_eq!(state.answer_lazy_offer(LazyChoice::LazyUnmount), None);
        assert_eq!(state.confirm, Some(Confirm::LazyFinal(path.clone())));
        // Step 2: cancel runs nothing.
        assert_eq!(state.answer_lazy_final(false), None);
        // Again, through both steps.
        state.offer_lazy(path.clone(), "busy".into());
        let _ = state.answer_lazy_offer(LazyChoice::LazyUnmount);
        assert_eq!(
            state.answer_lazy_final(true),
            Some(Job::Single {
                action: ArchiveAction::LazyUnmount,
                path: path.clone(),
                cleanup: false,
            })
        );
        // Try Normal Unmount Again is a normal unmount, never lazy.
        state.offer_lazy(path.clone(), "busy".into());
        assert_eq!(
            state.answer_lazy_offer(LazyChoice::TryNormalAgain),
            Some(Job::Single {
                action: ArchiveAction::Unmount,
                path: path.clone(),
                cleanup: false,
            })
        );
    }

    #[test]
    fn a_failure_without_a_recovery_offer_never_offers_lazy_unmount() {
        let path = PathBuf::from("/roms/x.zip");
        let mut state = with(vec![record("/roms/x.zip", MountState::Mounted)]);
        state.single_finished(
            ArchiveAction::Unmount,
            path.clone(),
            Err(crate::mount_operation_controller::OperationFailure {
                message: "permission denied".into(),
                offer_lazy_unmount: false,
            }),
        );
        assert_eq!(state.confirm, None);
        assert!(!state.lazy_offers.contains(&path));
        assert_eq!(state.feedback, Some((false, "permission denied".into())));
    }

    #[test]
    fn unmounting_needs_a_mounted_archive_and_its_own_confirmation() {
        let mut state = with(vec![
            record("/roms/m.zip", MountState::Mounted),
            record("/roms/p.zip", MountState::Pending),
        ]);
        state.request_unmount(PathBuf::from("/roms/p.zip"));
        assert_eq!(state.confirm, None, "not mounted: nothing to confirm");
        state.request_unmount(PathBuf::from("/roms/m.zip"));
        assert_eq!(state.answer_unmount(false), None);
        assert_eq!(state.confirm, None);
    }

    #[test]
    fn an_inert_state_never_loads_or_runs() {
        let context = egui::Context::default();
        let mut state = pending(1);
        state.records = None;
        state.poll(&context);
        assert!(state.loading.is_none());
        assert!(!state.spawn(Job::MountQueue(Vec::new()), &context));
        assert!(!state.busy());
    }

    #[test]
    fn rendering_alone_starts_nothing() {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        let mut state = pending(30);
        for tab in [MountsTab::Choose, MountsTab::Active] {
            state.tab = tab;
            for width in [600.0, 1600.0] {
                let mut request = None;
                let mut used = 0.0_f32;
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 900.0),
                        )),
                        ..Default::default()
                    },
                    |context| {
                        egui::CentralPanel::default().show(context, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                request = show(ui, &mut state);
                                used = ui.min_rect().right();
                            });
                        });
                    },
                );
                assert_eq!(request, None);
                assert!(used <= width, "{tab:?} at {width} used {used}");
            }
        }
        assert!(!state.busy());
    }
}
