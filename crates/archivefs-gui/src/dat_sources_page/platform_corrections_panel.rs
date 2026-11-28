//! Platform Corrections section of the DAT Sources page: the three-group
//! preview, applying the selected Ready corrections, and Undo (one game or
//! the batch). All work is in core (`Database::preview_platform_corrections`,
//! `apply_platform_corrections`, `undo_platform_corrections`); this module
//! only holds the person's selection and runs the writes off the UI thread.
//! No game file is moved, renamed or deleted.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use archivefs_core::{
    Database, PlatformCorrectionGroup, PlatformCorrectionPreview, PlatformCorrectionReceiptSummary,
    PlatformCorrectionsApplied, PlatformCorrectionsUndone, WhenSomeChanged,
};
use eframe::egui;

use crate::ui::components as widgets;

/// What the person asked the section to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CorrectionsAction {
    Toggle(i64),
    SelectAllReady,
    ClearSelection,
    /// Show the confirmation for the selected corrections.
    Review,
    Cancel,
    Apply {
        proceed_with_rest: bool,
    },
    Undo {
        receipt: PathBuf,
        game: Option<i64>,
        proceed_with_rest: bool,
    },
    Refresh,
    /// After a refusal because some games changed: go ahead with the others.
    ProceedWithRest,
}

enum WorkerResult {
    Applied(Result<PlatformCorrectionsApplied, String>),
    Undone(Result<PlatformCorrectionsUndone, String>),
    Receipts(Result<Vec<PlatformCorrectionReceiptSummary>, String>),
}

#[derive(Default)]
pub(crate) struct PlatformCorrectionsState {
    preview: Option<Box<PlatformCorrectionPreview>>,
    selected: BTreeSet<i64>,
    confirming: bool,
    worker: Option<Receiver<WorkerResult>>,
    status: Option<(widgets::StatusTone, String)>,
    /// The raw error behind `status`, which may name local paths: shown
    /// only under the collapsed technical details.
    status_detail: Option<String>,
    refused: Vec<(i64, String)>,
    receipts: Vec<PlatformCorrectionReceiptSummary>,
    /// Whether the last Apply/Undo was refused because some games changed,
    /// so "go ahead with the rest" can be offered.
    offer_rest: Option<OfferRest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OfferRest {
    Apply,
    Undo { receipt: PathBuf, game: Option<i64> },
}

/// One row of the preview, as shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CorrectionRowView {
    pub(crate) archive_id: i64,
    pub(crate) path: String,
    pub(crate) game_name: Option<String>,
    pub(crate) current: String,
    pub(crate) current_source: String,
    pub(crate) proposed: String,
    pub(crate) evidence: String,
    pub(crate) reason: String,
    pub(crate) group: PlatformCorrectionGroup,
    pub(crate) selected: bool,
    pub(crate) loses_discovery_import_undo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceiptView {
    pub(crate) path: PathBuf,
    pub(crate) created_at: String,
    pub(crate) catalogue: String,
    pub(crate) undoable: Vec<(i64, String, String, String)>,
    pub(crate) undone: usize,
    pub(crate) changed: Vec<(i64, String)>,
    /// The receipt cannot be read or trusted: shown, never hidden.
    pub(crate) problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlatformCorrectionsView {
    pub(crate) catalogue: Option<String>,
    pub(crate) rows: Vec<CorrectionRowView>,
    pub(crate) selected: usize,
    pub(crate) selected_losing_discovery_undo: usize,
    pub(crate) confirming: bool,
    pub(crate) busy: bool,
    pub(crate) status: Option<(widgets::StatusTone, String)>,
    pub(crate) status_detail: Option<String>,
    pub(crate) refused: Vec<(i64, String)>,
    pub(crate) offer_rest: bool,
    pub(crate) receipts: Vec<ReceiptView>,
}

fn system(platform: &str) -> String {
    if platform.is_empty() {
        "several systems".to_string()
    } else {
        archivefs_core::platform::display_name_for(platform).to_string()
    }
}

impl PlatformCorrectionsState {
    /// A new audit's preview replaces the old one; Ready rows are preselected.
    pub(crate) fn set_preview(
        &mut self,
        preview: Option<Box<PlatformCorrectionPreview>>,
        database: Option<&Path>,
    ) {
        self.selected = preview
            .as_ref()
            .map(|p| p.preselected().into_iter().collect())
            .unwrap_or_default();
        self.preview = preview;
        self.confirming = false;
        self.refused.clear();
        self.offer_rest = None;
        self.status = None;
        self.status_detail = None;
        self.refresh(database);
    }

    /// A new check started: its old preview is no longer offered. An Apply
    /// or Undo already running keeps going (its result and Undo are kept);
    /// the receipts stay listed.
    pub(crate) fn drop_preview(&mut self) {
        if self.is_busy() {
            return;
        }
        self.preview = None;
        self.selected.clear();
        self.confirming = false;
        self.offer_rest = None;
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.worker.is_some()
    }

    fn refresh(&mut self, database: Option<&Path>) {
        if self.worker.is_some() {
            return;
        }
        let Some(database) = database.map(Path::to_path_buf) else {
            return;
        };
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            // Listing is read-only: it never creates or migrates a library.
            let result = Database::open_read_only(&database)
                .and_then(|db| db.platform_correction_receipts())
                .map_err(|error| error.to_string());
            let _ = sender.send(WorkerResult::Receipts(result));
        });
        self.worker = Some(receiver);
    }

    pub(crate) fn handle(&mut self, action: CorrectionsAction, database: Option<&Path>) {
        if self.worker.is_some() {
            return;
        }
        match action {
            CorrectionsAction::Toggle(id) => {
                let ready = self.preview.as_ref().is_some_and(|p| {
                    p.group(PlatformCorrectionGroup::Ready)
                        .any(|c| c.archive_id == id)
                });
                if ready && !self.selected.remove(&id) {
                    self.selected.insert(id);
                }
                self.confirming = false;
            }
            CorrectionsAction::SelectAllReady => {
                if let Some(preview) = &self.preview {
                    self.selected = preview.preselected().into_iter().collect();
                }
                self.confirming = false;
            }
            CorrectionsAction::ClearSelection => {
                self.selected.clear();
                self.confirming = false;
            }
            CorrectionsAction::Review => self.confirming = !self.selected.is_empty(),
            CorrectionsAction::Cancel => {
                self.confirming = false;
                self.offer_rest = None;
            }
            CorrectionsAction::Refresh => self.refresh(database),
            CorrectionsAction::ProceedWithRest => {
                if let Some(rest) = self.rest_action() {
                    self.handle(rest, database);
                }
            }
            CorrectionsAction::Apply { proceed_with_rest } => {
                let (Some(preview), Some(database)) = (self.preview.clone(), database) else {
                    return;
                };
                let selected: Vec<i64> = self.selected.iter().copied().collect();
                let database = database.to_path_buf();
                let when = if proceed_with_rest {
                    WhenSomeChanged::ProceedWithTheRest
                } else {
                    WhenSomeChanged::RefuseAll
                };
                let (sender, receiver) = channel();
                std::thread::spawn(move || {
                    let result = Database::open_or_create(&database)
                        .and_then(|mut db| db.apply_platform_corrections(&preview, &selected, when))
                        .map_err(|error| error.to_string());
                    let _ = sender.send(WorkerResult::Applied(result));
                });
                self.worker = Some(receiver);
                self.confirming = false;
                self.offer_rest = None;
                self.status = Some((widgets::StatusTone::Info, "Applying corrections…".into()));
            }
            CorrectionsAction::Undo {
                receipt,
                game,
                proceed_with_rest,
            } => {
                let Some(database) = database.map(Path::to_path_buf) else {
                    return;
                };
                let when = if proceed_with_rest {
                    WhenSomeChanged::ProceedWithTheRest
                } else {
                    WhenSomeChanged::RefuseAll
                };
                // A single game's Undo has no "other games" to go ahead with.
                self.offer_rest = game.is_none().then(|| OfferRest::Undo {
                    receipt: receipt.clone(),
                    game,
                });
                let (sender, receiver) = channel();
                std::thread::spawn(move || {
                    let only = game.map(|g| vec![g]);
                    let result = Database::open_or_create(&database)
                        .and_then(|mut db| {
                            db.undo_platform_corrections(&receipt, only.as_deref(), when)
                        })
                        .map_err(|error| error.to_string());
                    let _ = sender.send(WorkerResult::Undone(result));
                });
                self.worker = Some(receiver);
                self.status = Some((widgets::StatusTone::Info, "Undoing…".into()));
            }
        }
    }

    /// Returns `(changed, library_changed)`.
    pub(crate) fn poll(&mut self, database: Option<&Path>) -> (bool, bool) {
        let Some(receiver) = &self.worker else {
            return (false, false);
        };
        let message = match receiver.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) => return (false, false),
            Err(TryRecvError::Disconnected) => {
                self.worker = None;
                self.status = Some((
                    widgets::StatusTone::Blocked,
                    "The correction stopped unexpectedly. Nothing is assumed; the list below shows what is in place.".into(),
                ));
                self.refresh(database);
                return (true, true);
            }
        };
        self.worker = None;
        self.status_detail = None;
        let mut library_changed = false;
        match message {
            WorkerResult::Receipts(Ok(receipts)) => self.receipts = receipts,
            WorkerResult::Receipts(Err(error)) => {
                self.status = Some((
                    widgets::StatusTone::Warning,
                    "Earlier corrections could not be read from the library database.".into(),
                ));
                self.status_detail = Some(error);
            }
            WorkerResult::Applied(Ok(applied)) => {
                self.refused = applied
                    .refused
                    .iter()
                    .map(|r| (r.archive_id, r.reason.clone()))
                    .collect();
                if applied.corrected.is_empty() {
                    self.status = Some((
                        widgets::StatusTone::Warning,
                        "Nothing was corrected: some selected games changed since the preview. Check the games again for a new preview, or correct only the games that did not change.".into(),
                    ));
                    self.offer_rest = (!self.refused.is_empty()).then_some(OfferRest::Apply);
                } else {
                    library_changed = true;
                    if let Some(preview) = &mut self.preview {
                        preview
                            .candidates
                            .retain(|c| !applied.corrected.contains(&c.archive_id));
                    }
                    for id in &applied.corrected {
                        self.selected.remove(id);
                    }
                    self.status = Some((
                        widgets::StatusTone::Success,
                        format!(
                            "{} game(s) corrected. Their results were saved and their earlier systems kept in history. No file was moved, renamed or deleted. Undo is below.",
                            applied.corrected.len()
                        ),
                    ));
                }
                self.refresh(database);
            }
            WorkerResult::Applied(Err(error)) => {
                // An error is not proof nothing was saved (a commit can land
                // and still report one): the library and receipts reload.
                library_changed = true;
                self.status = Some((
                    widgets::StatusTone::Blocked,
                    format!(
                        "{error}. Check the list of corrections you can undo below: if any was saved, its Undo is there."
                    ),
                ));
                self.refresh(database);
            }
            WorkerResult::Undone(Ok(undone)) => {
                self.refused = undone
                    .refused
                    .iter()
                    .map(|r| (r.archive_id, r.reason.clone()))
                    .collect();
                if undone.undone.is_empty() && undone.refused.is_empty() {
                    self.status = Some((
                        widgets::StatusTone::Info,
                        "Nothing left to undo: these corrections were already undone.".into(),
                    ));
                } else if undone.undone.is_empty()
                    && undone.refused.iter().all(|r| {
                        r.reason == "already undone"
                            || r.reason == "this correction was never applied"
                    })
                {
                    let never = undone
                        .refused
                        .iter()
                        .all(|r| r.reason == "this correction was never applied");
                    self.status = Some((
                        widgets::StatusTone::Info,
                        if never {
                            "Nothing to undo: this correction was never applied.".into()
                        } else {
                            "Nothing left to undo: this correction was already undone.".into()
                        },
                    ));
                } else if undone.undone.is_empty() {
                    self.status = Some((
                        widgets::StatusTone::Warning,
                        "Nothing was undone: some games changed after the correction, and Undo never overwrites a later change.".into(),
                    ));
                } else {
                    library_changed = true;
                    self.offer_rest = None;
                    self.status = Some((
                        widgets::StatusTone::Success,
                        format!(
                            "{} correction(s) undone: each game is back on its earlier system, with its earlier result.",
                            undone.undone.len()
                        ),
                    ));
                }
                if !library_changed && self.refused.is_empty() {
                    self.offer_rest = None;
                }
                self.refresh(database);
            }
            WorkerResult::Undone(Err(error)) => {
                library_changed = true;
                self.offer_rest = None;
                self.status = Some((widgets::StatusTone::Blocked, error));
                self.refresh(database);
            }
        }
        (true, library_changed)
    }

    pub(crate) fn view(&self) -> Option<PlatformCorrectionsView> {
        let rows: Vec<CorrectionRowView> = self
            .preview
            .iter()
            .flat_map(|p| p.candidates.iter())
            .map(|c| CorrectionRowView {
                archive_id: c.archive_id,
                path: c.path.clone(),
                game_name: c.game_name.clone(),
                current: system(&c.current.platform),
                current_source: c.current.source.clone(),
                proposed: system(&c.proposed_platform),
                evidence: if c.algorithm.is_empty() {
                    "no single exact match".to_string()
                } else {
                    format!("exact {} match", c.algorithm)
                },
                reason: c.reason.clone(),
                group: c.group,
                selected: self.selected.contains(&c.archive_id),
                loses_discovery_import_undo: c.loses_discovery_import_undo,
            })
            .collect();
        let receipts: Vec<ReceiptView> = self
            .receipts
            .iter()
            .filter(|r| !r.undoable.is_empty() || !r.changed.is_empty() || r.problem.is_some())
            .map(|r| ReceiptView {
                path: r.receipt_path.clone(),
                created_at: r.created_at.clone(),
                catalogue: r.catalogue.clone(),
                undoable: r
                    .undoable
                    .iter()
                    .map(|g| {
                        (
                            g.archive_id,
                            g.path.clone(),
                            g.previous_platform
                                .as_deref()
                                .map(system)
                                .unwrap_or_default(),
                            system(&g.corrected_platform),
                        )
                    })
                    .collect(),
                undone: r.undone.len(),
                changed: r
                    .changed
                    .iter()
                    .map(|c| (c.archive_id, c.reason.clone()))
                    .collect(),
                problem: r.problem.clone(),
            })
            .collect();
        if rows.is_empty() && receipts.is_empty() && self.status.is_none() {
            return None;
        }
        Some(PlatformCorrectionsView {
            catalogue: self
                .preview
                .as_ref()
                .map(|p| p.catalogue.display_name.clone()),
            selected: rows.iter().filter(|r| r.selected).count(),
            selected_losing_discovery_undo: rows
                .iter()
                .filter(|r| r.selected && r.loses_discovery_import_undo)
                .count(),
            rows,
            confirming: self.confirming,
            busy: self.worker.is_some(),
            status: self.status.clone(),
            status_detail: self.status_detail.clone(),
            refused: self.refused.clone(),
            offer_rest: self.offer_rest.is_some() && !self.refused.is_empty(),
            receipts,
        })
    }

    fn rest_action(&self) -> Option<CorrectionsAction> {
        match self.offer_rest.as_ref()? {
            OfferRest::Apply => Some(CorrectionsAction::Apply {
                proceed_with_rest: true,
            }),
            OfferRest::Undo { receipt, game } => Some(CorrectionsAction::Undo {
                receipt: receipt.clone(),
                game: *game,
                proceed_with_rest: true,
            }),
        }
    }
}

/// What a row says about its proposed system: only a Ready row "would
/// become" it; the others are suggestions or cannot name one system.
fn proposal_text(row: &CorrectionRowView) -> String {
    match row.group {
        PlatformCorrectionGroup::Ready => format!("would become {}", row.proposed),
        PlatformCorrectionGroup::NeedsReview => {
            format!(
                "the catalogue suggests {} (not changed automatically)",
                row.proposed
            )
        }
        PlatformCorrectionGroup::Ambiguous => {
            "the evidence does not name one system (not changed)".to_string()
        }
    }
}

pub(crate) const DISCOVERY_UNDO_WARNING: &str = "Correcting this game ends the Undo of the Discovery Import that added it. That Undo cannot be offered again, even after this correction is undone.";

fn group_title(group: PlatformCorrectionGroup) -> &'static str {
    match group {
        PlatformCorrectionGroup::Ready => "Ready to correct",
        PlatformCorrectionGroup::NeedsReview => "Needs your review (never changed automatically)",
        PlatformCorrectionGroup::Ambiguous => "Ambiguous (cannot be corrected)",
    }
}

/// Renders the section; returns what the person asked for.
pub(crate) fn show(ui: &mut egui::Ui, view: &PlatformCorrectionsView) -> Option<CorrectionsAction> {
    let mut action = None;
    ui.add_space(8.0);
    ui.heading("Platform Corrections");
    ui.label(
        "Games whose system was guessed by the scanner but match another system's catalogue exactly. Correcting a game changes only its library records: no file is moved, renamed or deleted, and every correction can be undone.",
    );
    if let Some(catalogue) = &view.catalogue {
        ui.label(format!("Evidence from: {catalogue}"));
    }
    if let Some((tone, text)) = &view.status {
        widgets::banner(ui, "Platform Corrections", text, *tone);
        if let Some(detail) = &view.status_detail {
            egui::CollapsingHeader::new("Technical details (may include local paths)")
                .id_salt("platform-corrections-status-detail")
                .show(ui, |ui| {
                    ui.label(detail);
                });
        }
    }
    if !view.refused.is_empty() {
        for (id, reason) in &view.refused {
            ui.label(format!("Game {id}: {reason}"));
        }
        if view.offer_rest && !view.busy && ui.button("Go ahead with the other games").clicked() {
            action = Some(CorrectionsAction::ProceedWithRest);
        }
    }
    for group in [
        PlatformCorrectionGroup::Ready,
        PlatformCorrectionGroup::NeedsReview,
        PlatformCorrectionGroup::Ambiguous,
    ] {
        let rows: Vec<&CorrectionRowView> = view.rows.iter().filter(|r| r.group == group).collect();
        if rows.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        ui.strong(format!("{} ({})", group_title(group), rows.len()));
        for row in rows {
            ui.horizontal_wrapped(|ui| {
                if group == PlatformCorrectionGroup::Ready {
                    let mut selected = row.selected;
                    if ui
                        .add_enabled(!view.busy, egui::Checkbox::without_text(&mut selected))
                        .changed()
                    {
                        action = Some(CorrectionsAction::Toggle(row.archive_id));
                    }
                }
                ui.label(format!(
                    "{} — now {} ({}), {} · {}{}",
                    row.path,
                    row.current,
                    row.current_source,
                    proposal_text(row),
                    row.evidence,
                    row.game_name
                        .as_deref()
                        .map(|g| format!(" · {g}"))
                        .unwrap_or_default()
                ));
            });
            ui.label(egui::RichText::new(&row.reason).small());
            if row.loses_discovery_import_undo {
                widgets::banner(
                    ui,
                    "Discovery Import Undo ends",
                    DISCOVERY_UNDO_WARNING,
                    widgets::StatusTone::Warning,
                );
            }
        }
    }
    let ready = view
        .rows
        .iter()
        .filter(|r| r.group == PlatformCorrectionGroup::Ready)
        .count();
    if ready > 0 {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!view.busy, egui::Button::new("Select all ready"))
                .clicked()
            {
                action = Some(CorrectionsAction::SelectAllReady);
            }
            if ui
                .add_enabled(!view.busy, egui::Button::new("Select none"))
                .clicked()
            {
                action = Some(CorrectionsAction::ClearSelection);
            }
            if !view.confirming
                && ui
                    .add_enabled(
                        !view.busy && view.selected > 0,
                        egui::Button::new(format!("Correct {} selected game(s)…", view.selected)),
                    )
                    .clicked()
            {
                action = Some(CorrectionsAction::Review);
            }
        });
    }
    if view.confirming {
        let mut text = format!(
            "Correct the system of {} game(s)? Each is re-checked against the catalogue and its file first. Only library records change; no file is moved, renamed or deleted. You can undo it afterwards.",
            view.selected
        );
        if view.selected_losing_discovery_undo > 0 {
            text.push_str(&format!(
                "\n\n{} of them were added by a Discovery Import: correcting them ends that import's Undo for good.",
                view.selected_losing_discovery_undo
            ));
        }
        widgets::banner(
            ui,
            "Confirm corrections",
            &text,
            widgets::StatusTone::Warning,
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!view.busy, egui::Button::new("Correct them"))
                .clicked()
            {
                action = Some(CorrectionsAction::Apply {
                    proceed_with_rest: false,
                });
            }
            if ui.button("Cancel").clicked() {
                action = Some(CorrectionsAction::Cancel);
            }
        });
    }
    if !view.receipts.is_empty() {
        ui.add_space(8.0);
        ui.strong("Corrections you can undo");
        for receipt in &view.receipts {
            if let Some(problem) = &receipt.problem {
                widgets::banner(
                    ui,
                    "A correction receipt cannot be used",
                    &format!("{}: {problem}", receipt.path.display()),
                    widgets::StatusTone::Blocked,
                );
                continue;
            }
            ui.label(format!(
                "{} · {} · {} in place, {} undone",
                receipt.created_at,
                receipt.catalogue,
                receipt.undoable.len(),
                receipt.undone
            ));
            for (id, path, previous, corrected) in &receipt.undoable {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{path}: {corrected} (was {previous})"));
                    if ui
                        .add_enabled(!view.busy, egui::Button::new("Undo"))
                        .clicked()
                    {
                        action = Some(CorrectionsAction::Undo {
                            receipt: receipt.path.clone(),
                            game: Some(*id),
                            proceed_with_rest: false,
                        });
                    }
                });
            }
            for (id, reason) in &receipt.changed {
                ui.label(format!("Game {id}: cannot be undone: {reason}"));
            }
            if receipt.undoable.len() > 1
                && ui
                    .add_enabled(!view.busy, egui::Button::new("Undo all of these"))
                    .clicked()
            {
                action = Some(CorrectionsAction::Undo {
                    receipt: receipt.path.clone(),
                    game: None,
                    proceed_with_rest: false,
                });
            }
        }
    }
    action
}

#[cfg(test)]
mod review_tests {
    use super::*;

    fn summary(problem: Option<&str>) -> PlatformCorrectionReceiptSummary {
        PlatformCorrectionReceiptSummary {
            receipt_path: PathBuf::from("/library.sqlite3.platform-corrections-x.json"),
            created_at: String::new(),
            catalogue: String::new(),
            undoable: Vec::new(),
            undone: Vec::new(),
            changed: Vec::new(),
            problem: problem.map(str::to_string),
        }
    }

    /// A receipt that cannot be used is shown with its problem, never hidden.
    #[test]
    fn an_unusable_receipt_is_shown() {
        let state = PlatformCorrectionsState {
            receipts: vec![summary(Some("its contents do not match its seal"))],
            ..Default::default()
        };
        let view = state.view().unwrap();
        assert_eq!(view.receipts.len(), 1);
        assert!(
            view.receipts[0]
                .problem
                .as_deref()
                .unwrap()
                .contains("seal")
        );
    }

    /// One game's Undo has no other games: "go ahead with the others" is
    /// never offered for it; Undo all still can be.
    #[test]
    fn a_single_game_undo_never_offers_the_others() {
        let mut state = PlatformCorrectionsState::default();
        let missing = tempfile::tempdir().unwrap();
        let db = missing.path().join("none.sqlite3");
        state.handle(
            CorrectionsAction::Undo {
                receipt: PathBuf::from("/r.json"),
                game: Some(1),
                proceed_with_rest: false,
            },
            Some(&db),
        );
        assert_eq!(state.offer_rest, None);
        state.worker = None;
        state.handle(
            CorrectionsAction::Undo {
                receipt: PathBuf::from("/r.json"),
                game: None,
                proceed_with_rest: false,
            },
            Some(&db),
        );
        assert!(matches!(
            state.offer_rest,
            Some(OfferRest::Undo { game: None, .. })
        ));
    }

    /// Review 2 C2: an Undo with nothing left says so (not "changed"); an
    /// Apply or Undo that reports an error still reloads the library.
    #[test]
    fn undo_and_error_results_say_what_happened() {
        let mut state = PlatformCorrectionsState::default();
        let (sender, receiver) = channel();
        state.worker = Some(receiver);
        sender
            .send(WorkerResult::Undone(Ok(PlatformCorrectionsUndone {
                undone: Vec::new(),
                refused: Vec::new(),
            })))
            .unwrap();
        state.poll(None);
        let status = state.status.clone().unwrap().1;
        assert!(status.contains("Nothing left to undo"), "{status}");
        // N4: one game's Undo of a correction already undone says so too.
        let (sender, receiver) = channel();
        state.worker = Some(receiver);
        sender
            .send(WorkerResult::Undone(Ok(PlatformCorrectionsUndone {
                undone: Vec::new(),
                refused: vec![archivefs_core::PlatformCorrectionRefusal {
                    archive_id: 1,
                    reason: "already undone".into(),
                }],
            })))
            .unwrap();
        state.poll(None);
        let status = state.status.clone().unwrap().1;
        assert!(status.contains("already undone"), "{status}");
        for result in [
            WorkerResult::Applied(Err("commit failed".into())),
            WorkerResult::Undone(Err("commit failed".into())),
        ] {
            let (sender, receiver) = channel();
            state.worker = Some(receiver);
            sender.send(result).unwrap();
            let (_, library_changed) = state.poll(None);
            assert!(library_changed, "an error result reloads the library");
        }
    }

    /// Review 4 C1: only a Ready row says what the game "would become";
    /// review and ambiguous rows never read as corrected.
    #[test]
    fn rows_say_what_their_group_means() {
        let row = |group| CorrectionRowView {
            archive_id: 1,
            path: "/g/a.nes".into(),
            game_name: None,
            current: "SNES".into(),
            current_source: "folder_alias".into(),
            proposed: "NES".into(),
            evidence: "exact SHA-1 match".into(),
            reason: String::new(),
            group,
            selected: false,
            loses_discovery_import_undo: false,
        };
        assert_eq!(
            proposal_text(&row(PlatformCorrectionGroup::Ready)),
            "would become NES"
        );
        for group in [
            PlatformCorrectionGroup::NeedsReview,
            PlatformCorrectionGroup::Ambiguous,
        ] {
            let text = proposal_text(&row(group));
            assert!(
                !text.contains("corrected") && !text.contains("would become"),
                "{text}"
            );
            assert!(text.contains("not changed"), "{text}");
        }
    }

    /// A new check drops the old preview but keeps the receipts (and an
    /// Apply already running).
    #[test]
    fn a_new_check_drops_the_old_preview_only() {
        let mut state = PlatformCorrectionsState {
            receipts: vec![summary(Some("x"))],
            selected: [1].into_iter().collect(),
            confirming: true,
            ..Default::default()
        };
        state.drop_preview();
        assert!(state.selected.is_empty());
        assert!(!state.confirming);
        assert_eq!(state.receipts.len(), 1);
        let (_sender, receiver) = channel();
        state.worker = Some(receiver);
        state.selected.insert(2);
        state.drop_preview();
        assert_eq!(state.selected.len(), 1, "a running Apply keeps its state");
    }
}
