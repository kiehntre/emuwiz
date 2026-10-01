//! Plain-language catalogue health and the reviewed source rebind, for the
//! Sources page.
//!
//! This module projects the backend's own answers; it never decides anything.
//! Source-level state is `archivefs_core::catalogue_health::SourceHealth`,
//! computed by `Database::source_health` and carried inside the cached library
//! snapshot, so it always belongs to the same database generation as the rest
//! of the page. Row-level counts come from the backend's presence preview and
//! are tagged with the generation they were computed for. Anything the backend
//! has not said yet is shown as *unknown*, never as healthy.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use archivefs_core::SourceFolderView;
use archivefs_core::catalogue_health::{
    CatalogueHealthCounts, RebindReason, SourceHealth, SourceHealthState, SourceRebindReview,
    SourceRootBinding,
};
use eframe::egui;

use crate::ui::components::{self as widgets, StatusTone};

/// One configured source and what the backend says about it. `health` is `None`
/// until the snapshot carries an answer for that source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceHealthRow {
    pub(crate) path: PathBuf,
    pub(crate) health: Option<SourceHealth>,
}

pub(crate) fn project_rows(
    views: &[SourceFolderView],
    health: &[SourceHealth],
) -> Vec<SourceHealthRow> {
    views
        .iter()
        .map(|view| SourceHealthRow {
            path: view.path.clone(),
            health: view
                .id
                .and_then(|id| {
                    health
                        .iter()
                        .find(|h| h.source_id == id && h.path == view.path)
                })
                .cloned(),
        })
        .collect()
}

/// How a source reads to a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wording {
    pub(crate) headline: &'static str,
    pub(crate) explanation: String,
    pub(crate) tone: StatusTone,
    pub(crate) needs_review: bool,
}

pub(crate) fn wording(health: Option<&SourceHealth>) -> Wording {
    let Some(health) = health else {
        return Wording {
            headline: "Not checked yet",
            explanation: "EmuWiz has not read this folder's catalogue health yet. Nothing is \
                          assumed to be fine."
                .into(),
            tone: StatusTone::Pending,
            needs_review: false,
        };
    };
    let (headline, explanation, tone) = match (health.state, health.rebind) {
        (SourceHealthState::Healthy, _) => (
            "Up to date",
            "The last scan covered everything in this folder, so EmuWiz can tell when a game \
             goes missing."
                .to_string(),
            StatusTone::Success,
        ),
        (SourceHealthState::NeedsScan, _) => (
            "Needs a scan",
            "No scan has covered this folder since it was last confirmed. EmuWiz will not mark \
             any game missing until one has."
                .to_string(),
            StatusTone::Warning,
        ),
        (SourceHealthState::PartialScan, _) => (
            "Partial scan",
            "The last scan did not cover everything, so EmuWiz cannot tell which games are \
             really gone. Scan again to complete it."
                .to_string(),
            StatusTone::Warning,
        ),
        (SourceHealthState::CoverageIncomplete, _) => (
            "Coverage incomplete",
            format!(
                "{} Missing games cannot be reported for this folder until a full scan \
                 succeeds.",
                health
                    .detail
                    .clone()
                    .unwrap_or_else(|| "The last scan could not be fully trusted.".into())
            ),
            StatusTone::Warning,
        ),
        (SourceHealthState::SourceUnavailable, _) => (
            "Folder unavailable",
            "EmuWiz cannot reach this folder right now. Is the drive connected? Nothing is \
             marked missing while it is away."
                .to_string(),
            StatusTone::Blocked,
        ),
        (SourceHealthState::RebindRequired, Some(RebindReason::BackingChanged)) => (
            "Review needed: different storage",
            "EmuWiz can see this folder, but it may not be the same storage source you \
             reviewed before. Until you review it, it cannot be scanned and no game can be \
             marked missing."
                .to_string(),
            StatusTone::Blocked,
        ),
        (SourceHealthState::RebindRequired, _) => (
            "Review needed",
            "This folder was scanned by an older version of EmuWiz that did not record which \
             storage it was on. Review it once so EmuWiz can tell if the drive is ever \
             swapped. Until then it cannot be scanned and no game can be marked missing."
                .to_string(),
            StatusTone::Blocked,
        ),
        (SourceHealthState::NotGameScanned, _) => (
            "Not scanned for games",
            "This folder's role keeps it out of game scanning.".to_string(),
            StatusTone::Info,
        ),
    };
    Wording {
        headline,
        explanation,
        tone,
        needs_review: health.state == SourceHealthState::RebindRequired,
    }
}

/// The one-line answer for the whole catalogue. "All clear" is possible only
/// when the backend has answered for every source and every one is healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Overall {
    pub(crate) sources: usize,
    pub(crate) unknown: usize,
    pub(crate) review_needed: usize,
    pub(crate) needs_scan: usize,
    pub(crate) incomplete: usize,
    pub(crate) unavailable: usize,
    pub(crate) healthy: usize,
}

impl Overall {
    pub(crate) fn from_rows(rows: &[SourceHealthRow]) -> Self {
        let mut overall = Overall {
            sources: rows.len(),
            ..Default::default()
        };
        for row in rows {
            match row.health.as_ref().map(|h| h.state) {
                None => overall.unknown += 1,
                Some(SourceHealthState::Healthy) => overall.healthy += 1,
                Some(SourceHealthState::NotGameScanned) => {}
                Some(SourceHealthState::RebindRequired) => overall.review_needed += 1,
                Some(SourceHealthState::NeedsScan) => overall.needs_scan += 1,
                Some(SourceHealthState::PartialScan | SourceHealthState::CoverageIncomplete) => {
                    overall.incomplete += 1
                }
                Some(SourceHealthState::SourceUnavailable) => overall.unavailable += 1,
            }
        }
        overall
    }

    pub(crate) fn all_clear(&self) -> bool {
        self.healthy > 0
            && self.unknown == 0
            && self.review_needed == 0
            && self.needs_scan == 0
            && self.incomplete == 0
            && self.unavailable == 0
    }

    pub(crate) fn headline(&self) -> (&'static str, StatusTone) {
        if self.sources == 0 {
            ("No game folders yet", StatusTone::Info)
        } else if self.unknown > 0 {
            ("Checking", StatusTone::Pending)
        } else if self.review_needed > 0 {
            ("Review needed", StatusTone::Blocked)
        } else if self.unavailable > 0 {
            ("Some folders unavailable", StatusTone::Blocked)
        } else if self.all_clear() {
            ("Up to date", StatusTone::Success)
        } else {
            ("Needs a scan", StatusTone::Warning)
        }
    }
}

/// One plain sentence for what stops the catalogue being trusted, or `None` when
/// nothing the backend has answered is blocking. Unknown sources are reported
/// separately by the caller; they are not a reason in themselves.
pub(crate) fn attention_summary(overall: &Overall) -> Option<String> {
    let mut parts = Vec::new();
    let mut add = |count: usize, one: &str, many: &str| match count {
        0 => {}
        1 => parts.push(format!("1 folder {one}")),
        n => parts.push(format!("{n} folders {many}")),
    };
    add(
        overall.review_needed,
        "needs review before scanning",
        "need review before scanning",
    );
    add(
        overall.unavailable,
        "cannot be reached right now",
        "cannot be reached right now",
    );
    add(
        overall.incomplete,
        "had an incomplete scan",
        "had an incomplete scan",
    );
    add(overall.needs_scan, "needs a scan", "need a scan");
    (!parts.is_empty()).then(|| format!("{}.", parts.join("; ")))
}

// --- Row-level counts: on demand, tagged with the generation they describe. --

/// What a finished row-level check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowCounts {
    pub(crate) counts: CatalogueHealthCounts,
    pub(crate) notes: Vec<String>,
}

pub(crate) type RowCheckResult = Result<RowCounts, String>;

struct RunningRowCheck {
    request: u64,
    generation: u64,
    receiver: Receiver<RowCheckResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SettledRowCheck {
    Counts {
        generation: u64,
        counts: Box<RowCounts>,
    },
    Failed(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RowCheckView<'a> {
    NotRun,
    Running,
    /// Computed for an earlier database generation; the numbers are not shown.
    Stale,
    Current(&'a RowCounts),
    Failed(&'a str),
}

/// Request/generation bookkeeping for the on-demand row check. At most one
/// check is in flight; starting another abandons the previous receiver, and a
/// result is accepted only for the request that is currently in flight, so an
/// older result can never overwrite a newer one.
#[derive(Default)]
pub(crate) struct RowCheckState {
    next_request: u64,
    running: Option<RunningRowCheck>,
    settled: Option<SettledRowCheck>,
}

impl RowCheckState {
    /// Registers a check for database `generation`; returns its request id.
    pub(crate) fn start(&mut self, generation: u64, receiver: Receiver<RowCheckResult>) -> u64 {
        self.next_request += 1;
        self.settled = None;
        self.running = Some(RunningRowCheck {
            request: self.next_request,
            generation,
            receiver,
        });
        self.next_request
    }

    /// Takes the in-flight result if it has arrived. Returns whether state changed.
    pub(crate) fn poll(&mut self) -> bool {
        let arrived = self.running.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|r| (running.request, r))
        });
        let Some((request, result)) = arrived else {
            return false;
        };
        self.settle(request, result)
    }

    /// Accepts `result` only if `request` is the one in flight.
    pub(crate) fn settle(&mut self, request: u64, result: RowCheckResult) -> bool {
        let Some(running) = self.running.as_ref().filter(|r| r.request == request) else {
            return false;
        };
        let generation = running.generation;
        self.running = None;
        self.settled = Some(match result {
            Ok(counts) => SettledRowCheck::Counts {
                generation,
                counts: Box::new(counts),
            },
            Err(message) => SettledRowCheck::Failed(message),
        });
        true
    }

    pub(crate) fn view(&self, current_generation: u64) -> RowCheckView<'_> {
        if self.running.is_some() {
            return RowCheckView::Running;
        }
        match &self.settled {
            None => RowCheckView::NotRun,
            Some(SettledRowCheck::Failed(message)) => RowCheckView::Failed(message),
            Some(SettledRowCheck::Counts { generation, counts }) => {
                if *generation == current_generation {
                    RowCheckView::Current(counts)
                } else {
                    RowCheckView::Stale
                }
            }
        }
    }
}

// --- The reviewed rebind dialog ------------------------------------------------

pub(crate) enum RebindStage {
    /// Reading the source's current storage. Nothing can be confirmed yet.
    Loading(Receiver<Result<SourceRebindReview, String>>),
    Review {
        review: Box<SourceRebindReview>,
        acknowledged: bool,
    },
    /// The review could not be made, or went out of date.
    Refused(String),
}

pub(crate) struct SourcesRebindDialogState {
    pub(crate) path: PathBuf,
    pub(crate) stage: RebindStage,
}

impl SourcesRebindDialogState {
    pub(crate) fn loading(
        path: PathBuf,
        receiver: Receiver<Result<SourceRebindReview, String>>,
    ) -> Self {
        Self {
            path,
            stage: RebindStage::Loading(receiver),
        }
    }

    pub(crate) fn poll(&mut self) -> bool {
        let RebindStage::Loading(receiver) = &self.stage else {
            return false;
        };
        let Ok(result) = receiver.try_recv() else {
            return false;
        };
        self.stage = match result {
            Ok(review) => RebindStage::Review {
                review: Box::new(review),
                acknowledged: false,
            },
            Err(message) => RebindStage::Refused(rebind_refusal_text(&message)),
        };
        true
    }

    /// What confirming would commit. Only a fully loaded review the person has
    /// explicitly acknowledged can be confirmed.
    pub(crate) fn confirmable(&self) -> Option<&SourceRebindReview> {
        match &self.stage {
            RebindStage::Review {
                review,
                acknowledged: true,
            } => Some(review),
            _ => None,
        }
    }
}

pub(crate) enum DialogAction {
    Confirm(Box<SourceRebindReview>),
    ReviewAgain,
    Close,
}

/// `major:minor` of a Linux `dev_t`, as `ls -l /dev` shows it.
/// What a person reads when a review or commit is refused. The backend's
/// "changed since it was reviewed" refusal is turned into plain language and
/// the internal "database error:" prefix never reaches the screen.
pub(crate) fn rebind_refusal_text(message: &str) -> String {
    if message.contains(archivefs_core::catalogue_health::REBIND_REVIEW_AGAIN) {
        return "The folder changed after you reviewed it, so nothing was recorded. Review it \
                again to continue."
            .to_string();
    }
    message
        .strip_prefix("database error: ")
        .unwrap_or(message)
        .to_string()
}

pub(crate) fn device_label(device: u64) -> String {
    let major = ((device >> 8) & 0xfff) | ((device >> 32) & !0xfff);
    let minor = (device & 0xff) | ((device >> 12) & !0xff);
    format!("{major}:{minor}")
}

fn storage_lines(binding: &SourceRootBinding) -> [(&'static str, String); 4] {
    [
        ("Filesystem", binding.filesystem_name()),
        ("Device", device_label(binding.device)),
        ("Filesystem ID", binding.filesystem_id_hex()),
        ("Folder number", binding.inode.to_string()),
    ]
}

pub(crate) fn rebind_reason_text(reason: RebindReason) -> &'static str {
    match reason {
        RebindReason::NeverBound => {
            "This folder was scanned before EmuWiz recorded which storage it was on, so EmuWiz \
             has nothing to compare it with."
        }
        RebindReason::BackingChanged => {
            "EmuWiz sees this folder, but it may not be the same storage source you reviewed \
             before. It could be a different disk, a restored copy, or a folder that was \
             replaced."
        }
    }
}

pub(crate) fn show_rebind_dialog(
    context: &egui::Context,
    dialog: &mut SourcesRebindDialogState,
    busy: bool,
) -> Option<DialogAction> {
    let mut action = None;
    let mut open = true;
    let path = dialog.path.clone();
    egui::Window::new("Review and rebind source")
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .open(&mut open)
        .show(context, |ui| {
            ui.strong("Game folder");
            widgets::path_value(ui, "Path", &path);
            ui.add_space(6.0);
            match &mut dialog.stage {
                RebindStage::Loading(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading this folder's storage...");
                    });
                }
                RebindStage::Refused(message) => {
                    ui.colored_label(ui.visuals().warn_fg_color, message.as_str());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if widgets::action_button(
                            ui,
                            "Review again",
                            widgets::ActionStyle::Primary,
                            !busy,
                        )
                        .clicked()
                        {
                            action = Some(DialogAction::ReviewAgain);
                        }
                        if widgets::action_button(
                            ui,
                            "Close",
                            widgets::ActionStyle::Quiet,
                            true,
                        )
                        .clicked()
                        {
                            action = Some(DialogAction::Close);
                        }
                    });
                }
                RebindStage::Review {
                    review,
                    acknowledged,
                } => {
                    ui.label("Reachable now: yes");
                    ui.add_space(4.0);
                    ui.label(rebind_reason_text(review.reason));
                    ui.add_space(6.0);
                    egui::Grid::new("rebind_review_storage")
                        .num_columns(3)
                        .spacing([14.0, 4.0])
                        .show(ui, |ui| {
                            ui.strong("");
                            ui.strong("Reviewed before");
                            ui.strong("This folder now");
                            ui.end_row();
                            let now = storage_lines(&review.current);
                            let before = review.recorded.as_ref().map(storage_lines);
                            for (index, (label, value)) in now.iter().enumerate() {
                                ui.weak(*label);
                                match &before {
                                    Some(before) => ui.label(before[index].1.as_str()),
                                    None => ui.weak("not recorded"),
                                };
                                ui.label(value.as_str());
                                ui.end_row();
                            }
                        });
                    ui.add_space(4.0);
                    ui.label(format!(
                        "{} game entr{} from this folder in your library. Last successful scan: {}.",
                        review.archive_count,
                        if review.archive_count == 1 { "y" } else { "ies" },
                        review
                            .last_successful_scan_at
                            .as_deref()
                            .unwrap_or("never")
                    ));
                    ui.add_space(8.0);
                    ui.strong("What rebinding means");
                    ui.label("- EmuWiz records that this folder is the storage you mean.");
                    ui.label("- It does not mark any game missing, and it changes no game file.");
                    ui.label("- It does not change your library entries or what EmuWiz knows about each game.");
                    ui.label(
                        "- You must run a new complete scan afterwards. Only that scan can mark \
                         a game missing.",
                    );
                    ui.add_space(8.0);
                    ui.checkbox(
                        acknowledged,
                        "I have checked that this is the storage I mean",
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if widgets::action_button(
                            ui,
                            "Rebind this source",
                            widgets::ActionStyle::Primary,
                            *acknowledged && !busy,
                        )
                        .clicked()
                        {
                            action = Some(DialogAction::Confirm(review.clone()));
                        }
                        if widgets::action_button(
                            ui,
                            "Cancel",
                            widgets::ActionStyle::Quiet,
                            true,
                        )
                        .clicked()
                        {
                            action = Some(DialogAction::Close);
                        }
                    });
                }
            }
        });
    if !open {
        action = Some(DialogAction::Close);
    }
    action
}

// --- The Sources-page section -------------------------------------------------

pub(crate) enum HealthAction {
    Review(PathBuf),
    CheckRows,
}

fn count_line(ui: &mut egui::Ui, label: &str, value: usize, note: &str) {
    ui.weak(label);
    ui.label(value.to_string());
    ui.weak(note);
    ui.end_row();
}

/// A compact, one-line-per-folder notice for folders that need review, meant for
/// the top of a page whose layout has no room for the full health section. It
/// draws nothing at all when no folder needs review.
pub(crate) fn show_review_banner(
    ui: &mut egui::Ui,
    rows: &[SourceHealthRow],
    busy: bool,
) -> Option<HealthAction> {
    let mut action = None;
    for row in rows {
        let words = wording(row.health.as_ref());
        if !words.needs_review {
            continue;
        }
        ui.horizontal_wrapped(|ui| {
            widgets::status_strip(ui, &[(words.headline, words.tone)]);
            ui.label(row.path.display().to_string());
            if widgets::action_button(
                ui,
                "Review and rebind source",
                widgets::ActionStyle::Primary,
                !busy,
            )
            .clicked()
            {
                action = Some(HealthAction::Review(row.path.clone()));
            }
        });
    }
    action
}

pub(crate) fn show_health_section(
    ui: &mut egui::Ui,
    rows: &[SourceHealthRow],
    row_view: &RowCheckView<'_>,
    busy: bool,
) -> Option<HealthAction> {
    let mut action = None;
    let overall = Overall::from_rows(rows);
    widgets::section_header(
        ui,
        "Catalogue health",
        Some("Whether EmuWiz can trust what it knows about each game folder."),
    );
    widgets::card(ui, |ui| {
        let (headline, tone) = overall.headline();
        widgets::status_strip(ui, &[(headline, tone)]);
        if overall.all_clear() {
            ui.label(
                egui::RichText::new(
                    "Every folder's last scan was complete and its storage is the one EmuWiz \
                     reviewed.",
                )
                .weak(),
            );
        }
        for row in rows {
            let words = wording(row.health.as_ref());
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);
            ui.strong(row.path.display().to_string());
            widgets::status_strip(ui, &[(words.headline, words.tone)]);
            ui.label(words.explanation.as_str());
            if words.needs_review
                && widgets::action_button(
                    ui,
                    "Review and rebind source",
                    widgets::ActionStyle::Primary,
                    !busy,
                )
                .clicked()
            {
                action = Some(HealthAction::Review(row.path.clone()));
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.strong("What is in the catalogue");
        match row_view {
            RowCheckView::NotRun => {
                ui.label(
                    "Check every catalogued game against its folder to see which are present, \
                     possibly moved, or missing. This only reads; nothing is changed.",
                );
            }
            RowCheckView::Running => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking the catalogue against your folders...");
                });
            }
            RowCheckView::Stale => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "That check is out of date: the library changed since it ran. Check again.",
                );
            }
            RowCheckView::Failed(message) => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("The check could not finish: {message}"),
                );
            }
            RowCheckView::Current(found) => {
                let counts = &found.counts;
                egui::Grid::new("catalogue_row_counts")
                    .num_columns(3)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        count_line(ui, "Games checked", counts.total, "");
                        count_line(
                            ui,
                            "Present",
                            counts.present_verified + counts.present_not_verified,
                            "found where EmuWiz expects them",
                        );
                        count_line(
                            ui,
                            "Possibly moved",
                            counts.possibly_moved,
                            "not found, but a matching file exists elsewhere",
                        );
                        count_line(
                            ui,
                            "Missing",
                            counts.missing,
                            "not found, and no match anywhere EmuWiz looked",
                        );
                        count_line(
                            ui,
                            "From a removed folder",
                            counts.orphaned_source,
                            "its folder is no longer in EmuWiz",
                        );
                        count_line(
                            ui,
                            "Could not be checked",
                            counts.not_checked,
                            "their folder was unreachable or not proven",
                        );
                    });
                for note in &found.notes {
                    ui.weak(note.as_str());
                }
                if counts.not_checked > 0 || overall.unknown > 0 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Some games could not be checked, so this is not an all-clear.",
                    );
                }
            }
        }
        ui.add_space(4.0);
        if widgets::action_button(
            ui,
            if matches!(row_view, RowCheckView::NotRun) {
                "Check catalogue"
            } else {
                "Check again"
            },
            widgets::ActionStyle::Secondary,
            !busy && !matches!(row_view, RowCheckView::Running),
        )
        .clicked()
        {
            action = Some(HealthAction::CheckRows);
        }
    });
    action
}

impl crate::ArchiveFsApp {
    /// Starts reading one source's storage for review. Read-only: nothing is
    /// recorded until the person confirms the finished review.
    pub(crate) fn start_rebind_review(&mut self, context: egui::Context, path: PathBuf) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let target = path.clone();
        std::thread::spawn(move || {
            let result =
                archivefs_core::review_source_rebind_default(&target).map_err(|e| e.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
        self.sources_ui.sources_rebind_dialog =
            Some(SourcesRebindDialogState::loading(path, receiver));
    }

    /// Starts the read-only row-level check for the current database generation.
    pub(crate) fn start_row_check(&mut self, context: egui::Context) {
        let roots: Vec<PathBuf> = self
            .database_state
            .snapshot()
            .map(|s| s.source_views.iter().map(|v| v.path.clone()).collect())
            .unwrap_or_default();
        let (sender, receiver) = std::sync::mpsc::channel();
        self.sources_ui
            .catalogue_row_check
            .start(self.database_generation.0, receiver);
        std::thread::spawn(move || {
            let _ = sender.send(run_row_check(&roots));
            context.request_repaint();
        });
    }

    /// Carries out a click on the health section.
    pub(crate) fn apply_health_action(
        &mut self,
        context: &egui::Context,
        action: Option<HealthAction>,
    ) {
        match action {
            Some(HealthAction::Review(path)) => self.start_rebind_review(context.clone(), path),
            Some(HealthAction::CheckRows) => self.start_row_check(context.clone()),
            None => {}
        }
    }

    /// Draws the rebind dialog when one is open and carries out what the person
    /// chose. Shared by every Sources page so there is one implementation.
    pub(crate) fn show_rebind_dialog_and_apply(&mut self, context: &egui::Context) {
        let busy = !self.source_action_available() || self.database_state.is_loading();
        let action = self
            .sources_ui
            .sources_rebind_dialog
            .as_mut()
            .and_then(|dialog| show_rebind_dialog(context, dialog, busy));
        match action {
            Some(DialogAction::Confirm(review)) => {
                // Only the loaded, acknowledged review currently on screen can be
                // committed; anything else is dropped without effect.
                let on_screen = self
                    .sources_ui
                    .sources_rebind_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.confirmable());
                if on_screen == Some(&*review) {
                    self.start_source_action(
                        context.clone(),
                        crate::SourceAction::Rebind { review },
                    );
                }
            }
            Some(DialogAction::ReviewAgain) => {
                if let Some(path) = self
                    .sources_ui
                    .sources_rebind_dialog
                    .as_ref()
                    .map(|dialog| dialog.path.clone())
                {
                    self.start_rebind_review(context.clone(), path);
                }
            }
            Some(DialogAction::Close) => self.sources_ui.sources_rebind_dialog = None,
            None => {}
        }
    }

    pub(crate) fn poll_catalogue_health(&mut self, context: &egui::Context) {
        let mut changed = self.sources_ui.catalogue_row_check.poll();
        if let Some(dialog) = self.sources_ui.sources_rebind_dialog.as_mut() {
            changed |= dialog.poll();
        }
        if changed {
            context.request_repaint();
        }
    }
}

/// The backend's own presence preview, opened read-only. It never migrates or
/// writes the library database.
fn run_row_check(roots: &[PathBuf]) -> RowCheckResult {
    let path = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
    let database = archivefs_core::Database::open_catalogue_health_read_only(&path)
        .map_err(|e| e.to_string())?;
    let report = archivefs_core::catalogue_health::preview_catalogue_health(&database, roots)
        .map_err(|e| e.to_string())?;
    Ok(RowCounts {
        counts: report.counts.clone(),
        notes: report.diagnostics.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn health(state: SourceHealthState, rebind: Option<RebindReason>) -> SourceHealth {
        SourceHealth {
            source_id: 1,
            path: PathBuf::from("/games"),
            state,
            rebind,
            generation: 1,
            detail: None,
        }
    }

    fn row(state: Option<SourceHealthState>) -> SourceHealthRow {
        SourceHealthRow {
            path: PathBuf::from("/games"),
            health: state.map(|s| health(s, None)),
        }
    }

    #[test]
    fn a_source_the_backend_has_not_answered_for_is_unknown_never_healthy() {
        let words = wording(None);
        assert_eq!(words.headline, "Not checked yet");
        assert_ne!(words.tone, StatusTone::Success);
        let overall = Overall::from_rows(&[row(None)]);
        assert!(!overall.all_clear());
        assert_eq!(overall.headline().0, "Checking");
    }

    #[test]
    fn attention_summary_names_what_blocks_the_catalogue_and_is_silent_when_clear() {
        let row = |state| row(Some(state));
        let clear = Overall::from_rows(&[row(SourceHealthState::Healthy)]);
        assert_eq!(attention_summary(&clear), None);
        let blocked = Overall::from_rows(&[
            row(SourceHealthState::RebindRequired),
            row(SourceHealthState::RebindRequired),
            row(SourceHealthState::SourceUnavailable),
            row(SourceHealthState::PartialScan),
            row(SourceHealthState::NeedsScan),
        ]);
        let text = attention_summary(&blocked).unwrap();
        assert!(
            text.contains("2 folders need review before scanning"),
            "{text}"
        );
        assert!(text.contains("1 folder cannot be reached"), "{text}");
        assert!(text.contains("1 folder had an incomplete scan"), "{text}");
        assert!(text.contains("1 folder needs a scan"), "{text}");
    }

    #[test]
    fn empty_summary_is_not_healthy() {
        let overall = Overall::from_rows(&[]);
        assert!(!overall.all_clear());
        assert_eq!(overall.headline().0, "No game folders yet");
    }

    #[test]
    fn all_clear_requires_every_source_healthy() {
        let healthy = row(Some(SourceHealthState::Healthy));
        assert!(Overall::from_rows(&[healthy.clone()]).all_clear());
        for blocking in [
            Some(SourceHealthState::NeedsScan),
            Some(SourceHealthState::PartialScan),
            Some(SourceHealthState::CoverageIncomplete),
            Some(SourceHealthState::SourceUnavailable),
            Some(SourceHealthState::RebindRequired),
            None,
        ] {
            let overall = Overall::from_rows(&[healthy.clone(), row(blocking)]);
            assert!(!overall.all_clear(), "{blocking:?} must block all-clear");
        }
    }

    #[test]
    fn only_rebind_required_offers_review_and_the_wording_distinguishes_storage_swap() {
        for state in [
            SourceHealthState::Healthy,
            SourceHealthState::NeedsScan,
            SourceHealthState::PartialScan,
            SourceHealthState::CoverageIncomplete,
            SourceHealthState::SourceUnavailable,
            SourceHealthState::NotGameScanned,
        ] {
            assert!(
                !wording(Some(&health(state, None))).needs_review,
                "{state:?}"
            );
        }
        let never = wording(Some(&health(
            SourceHealthState::RebindRequired,
            Some(RebindReason::NeverBound),
        )));
        let changed = wording(Some(&health(
            SourceHealthState::RebindRequired,
            Some(RebindReason::BackingChanged),
        )));
        assert!(never.needs_review && changed.needs_review);
        assert!(
            changed
                .explanation
                .contains("may not be the same storage source")
        );
        assert_ne!(never.headline, changed.headline);
        assert!(never.explanation.contains("no game can be marked missing"));
    }

    #[test]
    fn rows_pair_health_by_source_id_and_path() {
        let view = |id, path: &str| SourceFolderView {
            path: PathBuf::from(path),
            role: Default::default(),
            enabled: true,
            created_at: None,
            id: Some(id),
            availability: archivefs_core::SourceAvailability::Available,
            last_scan_status: None,
            last_scan_error: None,
            last_scan_at: None,
            last_successful_scan_at: None,
            last_archive_count: None,
            assigned_platform: None,
            unknown_archive_count: 0,
        };
        let mut one = health(SourceHealthState::Healthy, None);
        one.source_id = 1;
        one.path = PathBuf::from("/games");
        // Health recorded for a source id that now belongs to a different path
        // is not accepted for the new path.
        let rows = project_rows(
            &[view(1, "/games"), view(2, "/other"), view(1, "/replaced")],
            &[one],
        );
        assert!(rows[0].health.is_some());
        assert!(rows[1].health.is_none());
        assert!(rows[2].health.is_none());
    }

    fn counts(missing: usize) -> RowCounts {
        RowCounts {
            counts: CatalogueHealthCounts {
                missing,
                total: missing,
                ..Default::default()
            },
            notes: Vec::new(),
        }
    }

    #[test]
    fn row_check_result_for_an_old_generation_is_stale_not_shown() {
        let mut state = RowCheckState::default();
        let (_tx, rx) = mpsc::channel();
        let request = state.start(7, rx);
        assert_eq!(state.view(7), RowCheckView::Running);
        assert!(state.settle(request, Ok(counts(3))));
        assert!(matches!(state.view(7), RowCheckView::Current(c) if c.counts.missing == 3));
        // The library moved to a new generation: the old numbers are withheld.
        assert_eq!(state.view(8), RowCheckView::Stale);
    }

    #[test]
    fn an_older_row_check_result_cannot_overwrite_a_newer_request() {
        let mut state = RowCheckState::default();
        let (_old_tx, old_rx) = mpsc::channel();
        let old = state.start(1, old_rx);
        let (_new_tx, new_rx) = mpsc::channel();
        let new = state.start(2, new_rx);
        assert_ne!(old, new);
        // The abandoned request's late result is refused.
        assert!(!state.settle(old, Ok(counts(999))));
        assert_eq!(state.view(2), RowCheckView::Running);
        assert!(state.settle(new, Ok(counts(1))));
        // And a duplicate or late delivery after settling changes nothing.
        assert!(!state.settle(old, Ok(counts(999))));
        assert!(!state.settle(new, Ok(counts(999))));
        assert!(matches!(state.view(2), RowCheckView::Current(c) if c.counts.missing == 1));
    }

    #[test]
    fn polling_delivers_the_in_flight_result_only() {
        let mut state = RowCheckState::default();
        let (old_tx, old_rx) = mpsc::channel();
        state.start(1, old_rx);
        let (new_tx, new_rx) = mpsc::channel();
        state.start(2, new_rx);
        // Starting a new check drops the old receiver: a late result from the
        // abandoned request has nowhere to land.
        assert!(old_tx.send(Ok(counts(999))).is_err());
        assert!(
            !state.poll(),
            "nothing has arrived for the in-flight request yet"
        );
        new_tx.send(Ok(counts(2))).unwrap();
        assert!(state.poll());
        assert!(matches!(state.view(2), RowCheckView::Current(c) if c.counts.missing == 2));
    }

    #[test]
    fn a_failed_row_check_is_reported_not_hidden_as_zero() {
        let mut state = RowCheckState::default();
        let (_tx, rx) = mpsc::channel();
        let request = state.start(1, rx);
        state.settle(request, Err("database busy".into()));
        assert_eq!(state.view(1), RowCheckView::Failed("database busy"));
    }

    fn review() -> SourceRebindReview {
        let binding = SourceRootBinding {
            device: 1,
            inode: 2,
            filesystem_type: 0xef53,
            filesystem_id: vec![1, 2, 3, 4],
        };
        SourceRebindReview {
            source_id: 1,
            path: PathBuf::from("/games"),
            generation: 0,
            recorded: None,
            current: binding,
            reason: RebindReason::NeverBound,
            archive_count: 3,
            last_successful_scan_at: None,
        }
    }

    #[test]
    fn rebind_needs_a_loaded_review_and_an_explicit_acknowledgement() {
        let (tx, rx) = mpsc::channel();
        let mut dialog = SourcesRebindDialogState::loading(PathBuf::from("/games"), rx);
        assert!(
            dialog.confirmable().is_none(),
            "nothing to confirm while loading"
        );
        tx.send(Ok(review())).unwrap();
        assert!(dialog.poll());
        assert!(
            dialog.confirmable().is_none(),
            "a loaded review is not a confirmation"
        );
        if let RebindStage::Review { acknowledged, .. } = &mut dialog.stage {
            *acknowledged = true;
        }
        assert_eq!(dialog.confirmable().map(|r| r.source_id), Some(1));
    }

    #[test]
    fn a_refused_review_cannot_be_confirmed() {
        let (tx, rx) = mpsc::channel();
        let mut dialog = SourcesRebindDialogState::loading(PathBuf::from("/games"), rx);
        tx.send(Err("the folder cannot be reached".into())).unwrap();
        assert!(dialog.poll());
        assert!(matches!(dialog.stage, RebindStage::Refused(_)));
        assert!(dialog.confirmable().is_none());
    }

    /// The whole path a person takes, against a private temp library: a
    /// migrated legacy source is blocked, the snapshot says so, the reviewed
    /// rebind (the exact core calls the Sources action makes) unblocks it
    /// without marking anything missing, and only a later scan reads healthy.
    #[test]
    fn snapshot_health_follows_the_reviewed_rebind_and_never_reads_healthy_early() {
        use archivefs_core::{Database, add_source_folder_at, scan_source_folder_at};

        let temp = tempfile::tempdir().unwrap();
        let (config, database, folder) = (
            temp.path().join("config.toml"),
            temp.path().join("library.sqlite3"),
            temp.path().join("games"),
        );
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("game.zip"), b"game").unwrap();
        Database::open_or_create(&database).unwrap();
        add_source_folder_at(&config, &database, &folder).unwrap();
        scan_source_folder_at(&config, &database, &folder, "setup").unwrap();
        // Make it look migrated from before storage continuity was recorded.
        let sql = rusqlite::Connection::open(&database).unwrap();
        sql.execute("DELETE FROM source_scan_bindings", []).unwrap();
        sql.execute(
            "UPDATE source_folders SET last_successful_scan_at='legacy'",
            [],
        )
        .unwrap();

        let rows_for = |label: &str| {
            let db = Database::open_or_create(&database).unwrap();
            let snapshot = crate::database_load::load_snapshot_from(&db, &database, &config)
                .unwrap_or_else(|_| panic!("snapshot load failed ({label})"));
            let rows = project_rows(&snapshot.source_views, &snapshot.source_health);
            assert_eq!(rows.len(), 1, "{label}");
            rows
        };

        let blocked = rows_for("legacy");
        let words = wording(blocked[0].health.as_ref());
        assert!(words.needs_review, "a migrated source must offer review");
        assert!(!Overall::from_rows(&blocked).all_clear());

        let review = archivefs_core::review_source_rebind_at(&database, &folder).unwrap();
        archivefs_core::rebind_source_after_review_at(&database, &review).unwrap();
        let after_rebind = rows_for("rebound");
        let health = after_rebind[0].health.as_ref().unwrap();
        assert_eq!(health.state, SourceHealthState::NeedsScan);
        assert!(
            !Overall::from_rows(&after_rebind).all_clear(),
            "rebinding alone must not read as healthy"
        );
        let missing: i64 = sql
            .query_row(
                "SELECT COUNT(*) FROM archives WHERE last_verified_missing_at IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(missing, 0, "rebinding marks nothing missing");

        scan_source_folder_at(&config, &database, &folder, "after-review").unwrap();
        let healthy = rows_for("scanned");
        assert!(Overall::from_rows(&healthy).all_clear());
    }

    #[test]
    fn refusals_are_plain_language_without_internal_prefixes() {
        let stale = format!(
            "database error: {}",
            archivefs_core::catalogue_health::REBIND_REVIEW_AGAIN
        );
        let shown = rebind_refusal_text(&stale);
        assert!(shown.contains("changed after you reviewed it"));
        assert!(shown.contains("nothing was recorded"));
        assert!(!shown.contains("database error"));
        assert_eq!(
            rebind_refusal_text(
                "database error: the source folder cannot be reached right now; reconnect it and try again"
            ),
            "the source folder cannot be reached right now; reconnect it and try again"
        );
    }

    #[test]
    fn device_numbers_read_as_major_minor() {
        assert_eq!(device_label(0x0811), "8:17");
        assert_eq!(device_label(0), "0:0");
    }

    #[test]
    fn filesystem_names_are_plain_and_unknown_types_show_their_number() {
        let mut binding = review().current;
        assert_eq!(binding.filesystem_name(), "ext2/3/4");
        binding.filesystem_type = 0x1234_5678;
        assert_eq!(binding.filesystem_name(), "type 0x12345678");
    }
}
