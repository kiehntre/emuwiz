//! Mount-All / Unmount-All batch coordination.
//!
//! Relocated verbatim from `main.rs` (GUI cleanup bundle): the batch-engine
//! data types, the two pure coordinators (`run_mount_all_coordinator` /
//! `run_unmount_all_coordinator`) and the four progress/result renderers.
//! This is a move, not a rewrite - every body is byte-for-byte the original,
//! only item and field visibility was widened to `pub(crate)` so the
//! `ArchiveFsApp` glue in `main.rs` and the existing tests reach them the
//! same way they did when these lived in the crate root.
//!
//! The shared `>N items` typed-count confirmation gate
//! (`bulk_action_*` / `show_bulk_action_typed_count_gate`) is *not* here: it
//! is generic bulk-action infrastructure used by several unrelated dialogs
//! and stays in `main.rs`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use archivefs_core::{ArchiveRecord, MountState};
use eframe::egui;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MountAllItem {
    pub(crate) archive_path: PathBuf,
    pub(crate) mount_path: PathBuf,
    pub(crate) display_name: String,
}

pub(crate) fn mount_all_available(pending_count: usize, busy: bool) -> bool {
    pending_count > 0 && !busy
}

pub(crate) fn pending_mount_items(records: &[ArchiveRecord]) -> Vec<MountAllItem> {
    records
        .iter()
        .filter(|record| record.mount_state == MountState::Pending)
        .map(|record| MountAllItem {
            archive_path: record.mount_plan.archive.path.clone(),
            mount_path: record.mount_plan.mount_path.clone(),
            display_name: record.identity.display_name.clone(),
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BatchMountAttempt {
    Mounted(PathBuf),
    AlreadyMounted(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MountAllFailure {
    pub(crate) archive_path: PathBuf,
    pub(crate) message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MountAllSkipped {
    pub(crate) archive_path: PathBuf,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MountAllResult {
    pub(crate) total: usize,
    pub(crate) successful: usize,
    pub(crate) failures: Vec<MountAllFailure>,
    pub(crate) skipped: Vec<MountAllSkipped>,
    pub(crate) unattempted: usize,
    pub(crate) stopped: bool,
    pub(crate) setup_failure: Option<String>,
}

impl MountAllResult {
    pub(crate) fn setup_failed(total: usize, error: impl Into<String>) -> Self {
        Self {
            total,
            unattempted: total,
            setup_failure: Some(error.into()),
            ..Self::default()
        }
    }

    pub(crate) fn attempted(&self) -> usize {
        self.successful + self.failures.len()
    }

    pub(crate) fn failed(&self) -> usize {
        self.failures.len()
    }

    pub(crate) fn skipped(&self) -> usize {
        self.skipped.len()
    }

    pub(crate) fn completion_message(&self) -> String {
        if self.setup_failure.is_some() {
            "Mount All could not start.".to_string()
        } else if self.stopped {
            format!(
                "Mount All stopped after the current archive. {} archives were not attempted.",
                self.unattempted
            )
        } else if self.failed() > 0 {
            format!(
                "Mount All completed with {} failure{}.",
                self.failed(),
                if self.failed() == 1 { "" } else { "s" }
            )
        } else {
            "Mount All completed successfully.".to_string()
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum MountAllEvent {
    ArchiveStarted {
        index: usize,
        total: usize,
        item: MountAllItem,
    },
    ArchiveCompleted(MountAllItem),
    ArchiveFailed {
        item: MountAllItem,
        message: String,
    },
    ArchiveSkipped {
        item: MountAllItem,
        reason: String,
    },
    Finished(MountAllResult),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MountAllProgress {
    pub(crate) current_index: usize,
    pub(crate) total: usize,
    pub(crate) current_archive: Option<String>,
    pub(crate) successful: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
    pub(crate) stop_requested: bool,
}

pub(crate) struct RunningMountAll {
    pub(crate) receiver: Receiver<MountAllEvent>,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) progress: MountAllProgress,
}

#[derive(Clone)]
pub(crate) struct MountAllConfirmation;

pub(crate) fn run_mount_all_coordinator<E, V, M, P>(
    items: Vec<MountAllItem>,
    stop: &AtomicBool,
    mut archive_exists: E,
    mut validate: V,
    mut mount: M,
    mut publish: P,
) -> MountAllResult
where
    E: FnMut(&Path) -> bool,
    V: FnMut(&Path) -> Result<(), String>,
    M: FnMut(&Path) -> Result<BatchMountAttempt, String>,
    P: FnMut(MountAllEvent),
{
    let total = items.len();
    let mut result = MountAllResult {
        total,
        ..MountAllResult::default()
    };
    for (offset, mut item) in items.into_iter().enumerate() {
        if stop.load(Ordering::Acquire) {
            result.stopped = true;
            result.unattempted = total - offset;
            break;
        }

        if !archive_exists(&item.archive_path) {
            let reason = "archive disappeared before execution".to_string();
            result.skipped.push(MountAllSkipped {
                archive_path: item.archive_path.clone(),
                reason: reason.clone(),
            });
            publish(MountAllEvent::ArchiveSkipped { item, reason });
            continue;
        }

        if let Err(reason) = validate(&item.archive_path) {
            result.skipped.push(MountAllSkipped {
                archive_path: item.archive_path.clone(),
                reason: reason.clone(),
            });
            publish(MountAllEvent::ArchiveSkipped { item, reason });
            continue;
        }

        publish(MountAllEvent::ArchiveStarted {
            index: offset + 1,
            total,
            item: item.clone(),
        });
        match mount(&item.archive_path) {
            Ok(BatchMountAttempt::Mounted(actual_mount_path)) => {
                item.mount_path = actual_mount_path;
                result.successful += 1;
                publish(MountAllEvent::ArchiveCompleted(item));
            }
            Ok(BatchMountAttempt::AlreadyMounted(actual_mount_path)) => {
                item.mount_path = actual_mount_path;
                let reason = "archive is already mounted".to_string();
                result.skipped.push(MountAllSkipped {
                    archive_path: item.archive_path.clone(),
                    reason: reason.clone(),
                });
                publish(MountAllEvent::ArchiveSkipped { item, reason });
            }
            Err(message) => {
                result.failures.push(MountAllFailure {
                    archive_path: item.archive_path.clone(),
                    message: message.clone(),
                });
                publish(MountAllEvent::ArchiveFailed { item, message });
            }
        }
    }

    publish(MountAllEvent::Finished(result.clone()));
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnmountAllItem {
    pub(crate) archive_path: PathBuf,
    pub(crate) mount_path: PathBuf,
    pub(crate) display_name: String,
}

pub(crate) fn pending_unmount_items(records: &[ArchiveRecord]) -> Vec<UnmountAllItem> {
    records
        .iter()
        .filter(|record| record.mount_state == MountState::Mounted)
        .map(|record| UnmountAllItem {
            archive_path: record.mount_plan.archive.path.clone(),
            mount_path: record.mount_plan.mount_path.clone(),
            display_name: record.identity.display_name.clone(),
        })
        .collect()
}

/// Builds the `MountAll`/`UnmountAll` batch-engine's own item type for
/// exactly `paths` (already filtered to genuinely eligible archives by
/// `show_bulk_row_context_menu`) - the Library row context menu's "Mount
/// selected"/"Unmount selected" reuse `AppOperationRequest::MountAll`/
/// `UnmountAll` (and therefore `start_mount_all`/`start_unmount_all`)
/// unchanged, just fed a selection-scoped item list instead of every
/// pending/mounted archive.
pub(crate) fn mount_all_items_for_paths(
    records: &[ArchiveRecord],
    paths: &[PathBuf],
) -> Vec<MountAllItem> {
    records
        .iter()
        .filter(|record| record.is_mount_input() && paths.contains(&record.mount_plan.archive.path))
        .map(|record| MountAllItem {
            archive_path: record.mount_plan.archive.path.clone(),
            mount_path: record.mount_plan.mount_path.clone(),
            display_name: record.identity.display_name.clone(),
        })
        .collect()
}

pub(crate) fn unmount_all_items_for_paths(
    records: &[ArchiveRecord],
    paths: &[PathBuf],
) -> Vec<UnmountAllItem> {
    records
        .iter()
        .filter(|record| paths.contains(&record.mount_plan.archive.path))
        .map(|record| UnmountAllItem {
            archive_path: record.mount_plan.archive.path.clone(),
            mount_path: record.mount_plan.mount_path.clone(),
            display_name: record.identity.display_name.clone(),
        })
        .collect()
}

/// Exactly which selected archives are currently mounted, as full
/// `UnmountAllItem`s ready for `start_unmount_all` - the single
/// computation the "Unmount selected" confirmation dialog's displayed
/// count and its Confirm click both use (see the dialog in
/// `show_loaded_data`). A pure function of its two arguments, so calling
/// it again after `records`/`selected_archives` change always reflects
/// the current state - this is what "revalidate before starting; do not
/// unmount stale, missing or no-longer-mounted targets" means in
/// practice: there is no captured/cached list to go stale in the first
/// place.
pub(crate) fn mounted_selected_unmount_items(
    records: &[ArchiveRecord],
    selected_archives: &HashSet<PathBuf>,
) -> Vec<UnmountAllItem> {
    let mounted_selected_paths: Vec<PathBuf> = records
        .iter()
        .filter(|record| {
            selected_archives.contains(&record.mount_plan.archive.path)
                && record.mount_state == MountState::Mounted
        })
        .map(|record| record.mount_plan.archive.path.clone())
        .collect();
    unmount_all_items_for_paths(records, &mounted_selected_paths)
}

pub(crate) fn set_lazy_unmount_offer(
    offers: &mut HashSet<PathBuf>,
    archive_path: &Path,
    recovery_needed: bool,
) {
    if recovery_needed {
        offers.insert(archive_path.to_path_buf());
    } else {
        offers.remove(archive_path);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnmountAllFailure {
    pub(crate) archive_path: PathBuf,
    pub(crate) message: String,
    pub(crate) offer_lazy_unmount: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnmountAllSkip {
    pub(crate) archive_path: PathBuf,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnmountAllCleanupFailure {
    pub(crate) mount_path: PathBuf,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct UnmountAllResult {
    pub(crate) total: usize,
    pub(crate) successful: usize,
    pub(crate) failures: Vec<UnmountAllFailure>,
    pub(crate) skipped: Vec<UnmountAllSkip>,
    pub(crate) unattempted: usize,
    pub(crate) cleanup_successes: usize,
    pub(crate) cleanup_failures: Vec<UnmountAllCleanupFailure>,
    pub(crate) stopped: bool,
    pub(crate) setup_failure: Option<String>,
}

impl UnmountAllResult {
    pub(crate) fn setup_failed(total: usize, error: impl Into<String>) -> Self {
        Self {
            total,
            unattempted: total,
            setup_failure: Some(error.into()),
            ..Self::default()
        }
    }

    pub(crate) fn attempted(&self) -> usize {
        self.successful + self.failures.len()
    }

    pub(crate) fn completion_message(&self) -> String {
        if self.setup_failure.is_some() {
            "Unmount All could not start.".to_string()
        } else if self.stopped {
            format!(
                "Unmount All stopped after the current archive. {} archives were not attempted.",
                self.unattempted
            )
        } else if !self.failures.is_empty() {
            format!(
                "Unmount All completed with {} failure{}.",
                self.failures.len(),
                if self.failures.len() == 1 { "" } else { "s" }
            )
        } else if !self.cleanup_failures.is_empty() {
            format!(
                "Unmount All completed, but cleanup failed for {} mount{}.",
                self.cleanup_failures.len(),
                if self.cleanup_failures.len() == 1 {
                    ""
                } else {
                    "s"
                }
            )
        } else {
            "Unmount All completed successfully.".to_string()
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum UnmountAllEvent {
    ArchiveStarted {
        index: usize,
        total: usize,
        item: UnmountAllItem,
    },
    ArchiveCompleted(UnmountAllItem),
    ArchiveFailed {
        item: UnmountAllItem,
        message: String,
        offer_lazy_unmount: bool,
    },
    ArchiveSkipped {
        item: UnmountAllItem,
        reason: String,
    },
    CleanupStarted(PathBuf),
    CleanupCompleted(PathBuf),
    CleanupFailed {
        mount_path: PathBuf,
        message: String,
    },
    Finished(UnmountAllResult),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct UnmountAllProgress {
    pub(crate) current_index: usize,
    pub(crate) total: usize,
    pub(crate) current_archive: Option<String>,
    pub(crate) successful: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
    pub(crate) cleanup_successes: usize,
    pub(crate) cleanup_failures: usize,
    pub(crate) stop_requested: bool,
}

pub(crate) struct RunningUnmountAll {
    pub(crate) receiver: Receiver<UnmountAllEvent>,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) progress: UnmountAllProgress,
}

#[derive(Clone)]
pub(crate) struct UnmountAllConfirmation;
#[derive(Clone)]
pub(crate) struct UnmountSelectedConfirmation;

#[derive(Debug)]
pub(crate) enum BatchUnmountAttempt {
    Unmounted,
    NotMounted,
}

#[derive(Debug)]
pub(crate) struct BatchUnmountError {
    pub(crate) message: String,
    pub(crate) offer_lazy_unmount: bool,
}

pub(crate) fn run_unmount_all_coordinator<U, C, P>(
    items: Vec<UnmountAllItem>,
    stop: &AtomicBool,
    mut unmount: U,
    mut cleanup: C,
    mut publish: P,
) -> UnmountAllResult
where
    U: FnMut(&UnmountAllItem) -> Result<BatchUnmountAttempt, BatchUnmountError>,
    C: FnMut(&UnmountAllItem, &mut dyn FnMut(UnmountAllEvent)) -> Option<Result<(), String>>,
    P: FnMut(UnmountAllEvent),
{
    let total = items.len();
    let mut result = UnmountAllResult {
        total,
        ..Default::default()
    };
    for (offset, item) in items.into_iter().enumerate() {
        if stop.load(Ordering::Acquire) {
            result.stopped = true;
            result.unattempted = total - offset;
            break;
        }
        publish(UnmountAllEvent::ArchiveStarted {
            index: offset + 1,
            total,
            item: item.clone(),
        });
        match unmount(&item) {
            Ok(BatchUnmountAttempt::Unmounted) => {
                result.successful += 1;
                publish(UnmountAllEvent::ArchiveCompleted(item.clone()));
                match cleanup(&item, &mut publish) {
                    Some(Ok(())) => {
                        result.cleanup_successes += 1;
                        publish(UnmountAllEvent::CleanupCompleted(item.mount_path));
                    }
                    Some(Err(message)) => {
                        result.cleanup_failures.push(UnmountAllCleanupFailure {
                            mount_path: item.mount_path.clone(),
                            message: message.clone(),
                        });
                        publish(UnmountAllEvent::CleanupFailed {
                            mount_path: item.mount_path,
                            message,
                        });
                    }
                    None => {}
                }
            }
            Ok(BatchUnmountAttempt::NotMounted) => {
                let reason = "archive is no longer mounted".to_string();
                result.skipped.push(UnmountAllSkip {
                    archive_path: item.archive_path.clone(),
                    reason: reason.clone(),
                });
                publish(UnmountAllEvent::ArchiveSkipped { item, reason });
            }
            Err(error) => {
                result.failures.push(UnmountAllFailure {
                    archive_path: item.archive_path.clone(),
                    message: error.message.clone(),
                    offer_lazy_unmount: error.offer_lazy_unmount,
                });
                publish(UnmountAllEvent::ArchiveFailed {
                    item,
                    message: error.message,
                    offer_lazy_unmount: error.offer_lazy_unmount,
                });
            }
        }
    }
    publish(UnmountAllEvent::Finished(result.clone()));
    result
}

pub(crate) fn show_mount_all_progress(ui: &mut egui::Ui, progress: &MountAllProgress) -> bool {
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            ui.strong(format!(
                "Mounting {} of {}",
                progress.current_index, progress.total
            ));
            if let Some(archive) = &progress.current_archive {
                ui.label(archive);
            } else {
                ui.label("Preparing Mount All...");
            }
            ui.horizontal(|ui| {
                ui.label(format!("Successful: {}", progress.successful));
                ui.label(format!("Failed: {}", progress.failed));
                ui.label(format!("Skipped: {}", progress.skipped));
            });
            let fraction = if progress.total == 0 {
                0.0
            } else {
                progress.current_index as f32 / progress.total as f32
            };
            ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage());
            ui.add_enabled(
                !progress.stop_requested,
                egui::Button::new(if progress.stop_requested {
                    "Stop requested"
                } else {
                    "Stop After Current Archive"
                }),
            )
            .clicked()
        })
        .inner
}

pub(crate) fn show_mount_all_result(ui: &mut egui::Ui, result: &MountAllResult) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong(result.completion_message());
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Attempted: {}", result.attempted()));
            ui.label(format!("Successful: {}", result.successful));
            ui.label(format!("Failed: {}", result.failed()));
            ui.label(format!("Skipped: {}", result.skipped()));
            if result.unattempted > 0 {
                ui.label(format!("Not attempted: {}", result.unattempted));
            }
        });
        if let Some(error) = &result.setup_failure {
            ui.colored_label(ui.visuals().error_fg_color, format!("Setup error: {error}"));
        }
        if !result.failures.is_empty() {
            egui::CollapsingHeader::new("Failed archives")
                .default_open(false)
                .show(ui, |ui| {
                    for failure in &result.failures {
                        let text =
                            format!("{} — {}", failure.archive_path.display(), failure.message);
                        ui.add(egui::Label::new(&text).truncate())
                            .on_hover_text(text);
                    }
                });
        }
    });
}

pub(crate) fn show_unmount_all_progress(ui: &mut egui::Ui, progress: &UnmountAllProgress) -> bool {
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            ui.strong(format!(
                "Unmounting {} of {}",
                progress.current_index, progress.total
            ));
            ui.label(
                progress
                    .current_archive
                    .as_deref()
                    .unwrap_or("Preparing Unmount All..."),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Successful: {}", progress.successful));
                ui.label(format!("Failed: {}", progress.failed));
                ui.label(format!("Skipped: {}", progress.skipped));
                ui.label(format!(
                    "Cleanup successful: {}",
                    progress.cleanup_successes
                ));
                ui.label(format!("Cleanup failed: {}", progress.cleanup_failures));
            });
            let fraction = if progress.total == 0 {
                0.0
            } else {
                progress.current_index as f32 / progress.total as f32
            };
            ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage());
            ui.add_enabled(
                !progress.stop_requested,
                egui::Button::new(if progress.stop_requested {
                    "Stop requested"
                } else {
                    "Stop After Current Archive"
                }),
            )
            .clicked()
        })
        .inner
}

pub(crate) fn show_unmount_all_result(ui: &mut egui::Ui, result: &UnmountAllResult) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong(result.completion_message());
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Attempted: {}", result.attempted()));
            ui.label(format!("Successful: {}", result.successful));
            ui.label(format!("Failed: {}", result.failures.len()));
            ui.label(format!("Skipped: {}", result.skipped.len()));
            ui.label(format!("Not attempted: {}", result.unattempted));
            ui.label(format!("Cleanup successful: {}", result.cleanup_successes));
            ui.label(format!("Cleanup failed: {}", result.cleanup_failures.len()));
        });
        if let Some(error) = &result.setup_failure {
            ui.colored_label(ui.visuals().error_fg_color, format!("Setup error: {error}"));
        }
        if !result.failures.is_empty() {
            egui::CollapsingHeader::new("Failed archives")
                .default_open(false)
                .show(ui, |ui| {
                    for failure in &result.failures {
                        let text = format!(
                            "{} — {}{}",
                            failure.archive_path.display(),
                            failure.message,
                            if failure.offer_lazy_unmount {
                                " — individual Lazy Unmount recovery available"
                            } else {
                                ""
                            }
                        );
                        ui.add(egui::Label::new(&text).truncate())
                            .on_hover_text(text);
                    }
                });
        }
        if !result.cleanup_failures.is_empty() {
            egui::CollapsingHeader::new("Cleanup failures")
                .default_open(false)
                .show(ui, |ui| {
                    for failure in &result.cleanup_failures {
                        let text =
                            format!("{} — {}", failure.mount_path.display(), failure.message);
                        ui.add(egui::Label::new(&text).truncate())
                            .on_hover_text(text);
                    }
                });
        }
    });
}
