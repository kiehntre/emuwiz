//! The second part of "Library files check": what became of library entries
//! whose file is not where the library last saw it, and which links in the
//! library folders work, are broken, or point at storage that is not there.
//!
//! The first part of the page pairs files with entries by stored checksum.
//! EmuWiz does not store a checksum for a library file, so that part can only
//! say "possibly moved". This part uses the evidence that does exist (EmuWiz's
//! own rename history, the file system's record of each file, and matching
//! name, size and date) and says how strong each piece is.
//!
//! Everything shown comes from `archivefs_core::collection_intelligence`. This
//! module starts that check on a worker thread and puts its answer into plain
//! words. It never changes a library entry, a "file not found" mark, a link
//! or a game file, and it offers no button that could.

use crate::ui::components::{
    ActionStyle, StatusTone, action_button, banner, card, detail_row, status_badge, tab_row,
    technical_details,
};
use archivefs_core::collection_intelligence::{
    EvidenceClass, EvidenceTier, LinkFinding, LinkGrade, Progress, ReconcileRequest,
    ReconciliationReport, RecordFinding, RecordSet, reconcile,
};
use eframe::egui;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

const INTRO: &str = "When a game file is moved or renamed outside EmuWiz, the library keeps the old entry and marks it \"file not found\". This works out, from evidence, where those files went, and checks the links in your library folders. It only looks: nothing is changed.";
const ROW_HEIGHT: f32 = 26.0;

enum Message {
    Progress(Progress),
    Finished(Box<Result<ReconciliationReport, String>>),
}

struct Job {
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Message>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Overview,
    Review,
    Links,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Show {
    /// Weak, candidate and unresolved: the entries a person should look at.
    #[default]
    NeedsReview,
    All,
    Only(EvidenceClass),
}

/// Where the check reads from. Live uses the real library; tests name a copy.
#[derive(Clone, Default)]
enum Source {
    #[default]
    Live,
    #[cfg(test)]
    Fixed { database: PathBuf },
}

#[derive(Default)]
pub(super) struct State {
    source: Source,
    job: Option<Job>,
    progress: Option<Progress>,
    report: Option<Arc<ReconciliationReport>>,
    error: Option<String>,
    tab: Tab,
    show: Show,
    set: Option<RecordSet>,
    query: String,
    filtered: Vec<usize>,
    filter_key: Option<(Show, Option<RecordSet>, String)>,
    selected: Option<usize>,
    selected_link: Option<usize>,
    compare_contents: bool,
}

fn tone(class: EvidenceClass) -> StatusTone {
    match class {
        EvidenceClass::Confirmed | EvidenceClass::Present => StatusTone::Success,
        EvidenceClass::Strong => StatusTone::Active,
        EvidenceClass::Metadata => StatusTone::Info,
        EvidenceClass::Weak | EvidenceClass::Candidate => StatusTone::Warning,
        EvidenceClass::Unresolved => StatusTone::Pending,
    }
}

/// The page's words for each level. "Metadata match" stays its own level: it
/// is not a confirmed move and must not read like one.
fn level(class: EvidenceClass) -> &'static str {
    match class {
        EvidenceClass::Confirmed => "Confirmed move",
        EvidenceClass::Strong => "Strong match",
        EvidenceClass::Metadata => "Metadata match",
        EvidenceClass::Weak => "Weak match",
        EvidenceClass::Present => "Stale missing mark",
        EvidenceClass::Candidate => "Possible match",
        EvidenceClass::Unresolved => "Unresolved",
    }
}

fn link_tone(grade: LinkGrade) -> StatusTone {
    match grade {
        LinkGrade::Resolves => StatusTone::Success,
        LinkGrade::TargetIdentified => StatusTone::Active,
        LinkGrade::PlausibleCandidate => StatusTone::Warning,
        LinkGrade::Unresolved => StatusTone::Pending,
    }
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

impl State {
    #[cfg(test)]
    pub(super) fn for_database(database: PathBuf) -> Self {
        Self {
            source: Source::Fixed { database },
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }

    #[cfg(test)]
    pub(super) fn with_report(report: ReconciliationReport) -> Self {
        Self {
            report: Some(Arc::new(report)),
            ..Self::default()
        }
    }

    fn request(&self) -> Result<ReconcileRequest, String> {
        let database = match &self.source {
            Source::Live => {
                archivefs_core::default_database_path().map_err(|error| error.to_string())?
            }
            #[cfg(test)]
            Source::Fixed { database } => database.clone(),
        };
        let mut request = ReconcileRequest::new(database);
        request.verify_content = self.compare_contents;
        Ok(request)
    }

    pub(super) fn start(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        self.error = None;
        self.progress = None;
        let request = match self.request() {
            Ok(request) => request,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let (sender, receiver) = mpsc::sync_channel(32);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let ctx = ctx.clone();
        self.job = Some(Job { cancel, receiver });
        crate::gui_v2::backend::spawn_worker(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                reconcile(&request, &worker_cancel, &mut |progress| {
                    let _ = sender.try_send(Message::Progress(progress));
                })
                .map_err(|error| error.to_string())
            }))
            .unwrap_or_else(|_| Err("The check stopped unexpectedly. Nothing was changed.".into()));
            let _ = sender.send(Message::Finished(Box::new(result)));
            ctx.request_repaint();
        });
    }

    fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn poll(&mut self) {
        loop {
            let Some(job) = &self.job else { return };
            match job.receiver.try_recv() {
                Ok(Message::Progress(progress)) => self.progress = Some(progress),
                Ok(Message::Finished(result)) => {
                    self.job = None;
                    self.progress = None;
                    match *result {
                        Ok(report) => {
                            self.report = Some(Arc::new(report));
                            self.filter_key = None;
                            self.selected = None;
                            self.selected_link = None;
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.job = None;
                    self.error =
                        Some("The check stopped unexpectedly. Nothing was changed.".into());
                }
            }
        }
    }

    fn refilter(&mut self, report: &ReconciliationReport) {
        let key = (self.show, self.set, self.query.trim().to_lowercase());
        if self.filter_key.as_ref() == Some(&key) {
            return;
        }
        let words: Vec<&str> = key.2.split_whitespace().collect();
        self.filtered = report
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                let class_ok = match self.show {
                    Show::All => true,
                    Show::NeedsReview => matches!(
                        record.class,
                        EvidenceClass::Weak | EvidenceClass::Candidate | EvidenceClass::Unresolved
                    ),
                    Show::Only(class) => record.class == class,
                };
                class_ok && self.set.is_none_or(|set| record.set == set) && {
                    words.is_empty() || {
                        let text = record.old_path.to_string_lossy().to_lowercase();
                        words.iter().all(|word| text.contains(word))
                    }
                }
            })
            .map(|(index, _)| index)
            .collect();
        self.filter_key = Some(key);
        self.selected = None;
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll();
    ui.label(INTRO);
    let idle = state.job.is_none();
    ui.horizontal_wrapped(|ui| {
        let label = if state.report.is_some() {
            "Check again"
        } else {
            "Check now"
        };
        if action_button(ui, label, ActionStyle::Primary, idle).clicked() {
            state.start(ui.ctx());
        }
        if !idle && action_button(ui, "Cancel", ActionStyle::Secondary, true).clicked() {
            state.cancel();
        }
        ui.add_enabled(
            idle,
            egui::Checkbox::new(
                &mut state.compare_contents,
                "Also compare file contents where a checksum was recorded",
            ),
        );
    });
    if !idle {
        ui.horizontal(|ui| {
            ui.spinner();
            let text = match state.progress {
                Some(Progress {
                    phase,
                    done,
                    total: Some(total),
                }) => {
                    format!("{phase}: {} of {}", thousands(done), thousands(total))
                }
                Some(Progress {
                    phase,
                    done,
                    total: None,
                }) if done > 0 => {
                    format!("{phase}: {} so far", thousands(done))
                }
                Some(Progress { phase, .. }) => phase.to_string(),
                None => "Reading the library".to_string(),
            };
            ui.label(text);
        });
        ui.ctx().request_repaint_after(Duration::from_millis(150));
    }
    if let Some(error) = &state.error {
        banner(ui, "Could not complete", error, StatusTone::Blocked);
    }
    let Some(report) = state.report.clone() else {
        if idle && state.error.is_none() {
            card(ui, |ui| {
                ui.label("Not checked yet. The check lists your library folders (file names, sizes and dates only) and reads the library without changing it.");
            });
        }
        return;
    };
    if let Some(tab) = tab_row(
        ui,
        &[
            (Tab::Overview, "Overview"),
            (Tab::Review, "Entries to review"),
            (Tab::Links, "Links"),
        ],
        state.tab,
    ) {
        state.tab = tab;
    }
    ui.add_space(6.0);
    match state.tab {
        Tab::Overview => overview(ui, &report),
        Tab::Review => review(ui, state, &report),
        Tab::Links => links(ui, state, &report),
    }
}

fn overview(ui: &mut egui::Ui, report: &ReconciliationReport) {
    let m = &report.measures;
    egui::ScrollArea::vertical().id_salt("collection_reconcile_overview").show(ui, |ui| {
        card(ui, |ui| {
            ui.strong("Library entries");
            ui.label("These are separate counts. Do not add them together.");
            detail_row(ui, "Entries in the library", &thousands(m.rows_in_library));
            detail_row(ui, "File present, as recorded", &thousands(m.rows_present_as_recorded));
            if m.rows_present_but_changed > 0 {
                detail_row(ui, "File present, changed since scanned", &thousands(m.rows_present_but_changed));
            }
            detail_row(ui, "Game folders present", &thousands(m.rows_present_folders));
            detail_row(ui, "Marked \"file not found\"", &thousands(m.rows_flagged_missing));
            detail_row(ui, "Not marked, but no file", &thousands(m.rows_absent_never_flagged));
            if m.rows_not_checked > 0 {
                detail_row(ui, "Not checked (see the notes)", &thousands(m.rows_not_checked));
            }
            if m.forgotten_records_not_checked > 0 {
                detail_row(
                    ui,
                    "Removed entries not judged",
                    &thousands(m.forgotten_records_not_checked),
                );
            }
            detail_row(ui, "Removed earlier by Forget Missing", &thousands(m.records_removed_by_forget_missing));
            detail_row(
                ui,
                "Different files behind the present entries",
                &format!("{} ({})", thousands(m.distinct_files_behind_present_rows), gib(m.bytes_of_distinct_present_files)),
            );
        });
        card(ui, |ui| {
            ui.strong(format!(
                "What became of {} entries without a file at their recorded place",
                thousands(m.records_examined)
            ));
            ui.label(format!(
                "{} are tied to a file that exists now ({} different files).",
                thousands(m.records_tied_to_a_file),
                thousands(m.distinct_files_reached)
            ));
            egui::Grid::new("collection_reconcile_classes").striped(true).show(ui, |ui| {
                ui.strong("Evidence");
                for set in RecordSet::ALL {
                    ui.strong(set.label());
                }
                ui.strong("Total");
                ui.end_row();
                for class in EvidenceClass::ALL {
                    let count = |set: RecordSet| -> u64 {
                        EvidenceTier::ALL
                            .iter()
                            .filter(|tier| tier.class() == class)
                            .filter_map(|tier| report.record_sets.get(&set)?.get(tier.code()))
                            .map(|count| count.records)
                            .sum()
                    };
                    let row = RecordSet::ALL.map(count);
                    status_badge(ui, level(class), tone(class));
                    for value in row {
                        ui.label(thousands(value));
                    }
                    ui.label(thousands(row.iter().sum()));
                    ui.end_row();
                }
            });
            ui.label("\"Metadata match\" rests on recorded facts such as name, size and date. It does not show that two files hold the same data.");
            technical_details(ui, "collection_reconcile_tiers", |ui| {
                for tier in EvidenceTier::ALL {
                    let total: u64 = RecordSet::ALL
                        .iter()
                        .filter_map(|set| report.record_sets.get(set)?.get(tier.code()))
                        .map(|count| count.records)
                        .sum();
                    if total > 0 {
                        ui.label(format!("{} {}: {}", tier.code(), thousands(total), tier.explanation()));
                    }
                }
                let i = &report.inventory;
                ui.label(format!(
                    "Folders listed: {} entries in {} folders.",
                    thousands(i.entries),
                    thousands(i.directories_listed)
                ));
            });
        });
        let l = &report.links;
        card(ui, |ui| {
            ui.strong("Links in the library folders");
            ui.label("A link is a shortcut to a file kept somewhere else. Links are not library entries and are counted on their own.");
            detail_row(ui, "Links", &thousands(l.total));
            detail_row(ui, "Working", &thousands(l.resolve));
            detail_row(ui, "Broken, target found", &thousands(l.target_identified));
            detail_row(ui, "Broken, possible match only", &thousands(l.plausible_candidate));
            detail_row(ui, "Broken, nothing found", &thousands(l.unresolved));
            detail_row(
                ui,
                "Of the broken ones, storage not there",
                &thousands(l.storage_unavailable),
            );
        });
        if report.content_check.requested {
            let c = &report.content_check;
            card(ui, |ui| {
                ui.strong("File contents compared");
                ui.label(format!(
                    "{} compared: {} match, {} differ, {} could not be compared.",
                    thousands(c.compared),
                    thousands(c.matched),
                    thousands(c.differed),
                    thousands(c.not_comparable)
                ));
            });
        }
        for note in &report.notes {
            ui.label(format!("Note: {note}"));
        }
    });
}

fn review(ui: &mut egui::Ui, state: &mut State, report: &ReconciliationReport) {
    ui.horizontal_wrapped(|ui| {
        let mut options = vec![(Show::NeedsReview, "Needs review"), (Show::All, "All")];
        options.extend(EvidenceClass::ALL.map(|class| (Show::Only(class), level(class))));
        if let Some(show) = tab_row(ui, &options, state.show) {
            state.show = show;
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("From:");
        let mut options: Vec<(Option<RecordSet>, &str)> = vec![(None, "Everywhere")];
        options.extend(RecordSet::ALL.map(|set| (Some(set), set.label())));
        if let Some(set) = tab_row(ui, &options, state.set) {
            state.set = set;
        }
        ui.label("Search:");
        ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .desired_width(220.0)
                .hint_text("part of a name or folder"),
        );
    });
    state.refilter(report);
    ui.label(format!(
        "{} entries",
        thousands(state.filtered.len() as u64)
    ));
    if state.filtered.is_empty() {
        card(ui, |ui| {
            ui.label("Nothing matches. Choose another level or clear the search.");
        });
        return;
    }
    let detail_height = if state.selected.is_some() { 230.0 } else { 0.0 };
    let list_height = (ui.available_height() - detail_height).max(160.0);
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("collection_reconcile_records")
        .max_height(list_height)
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, state.filtered.len(), |ui, rows| {
            for row in rows {
                let index = state.filtered[row];
                let record = &report.records[index];
                ui.push_id((record.set as u8, record.archive_id, row), |ui| {
                    ui.horizontal(|ui| {
                        status_badge(ui, level(record.class), tone(record.class));
                        let name = record
                            .old_path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let label = match &record.platform {
                            Some(platform) => format!("{name}   ({platform})"),
                            None => name,
                        };
                        if ui
                            .selectable_label(state.selected == Some(index), label)
                            .clicked()
                        {
                            clicked = Some(index);
                        }
                    });
                });
            }
        });
    if let Some(index) = clicked {
        state.selected = Some(index);
    }
    if let Some(record) = state.selected.and_then(|index| report.records.get(index)) {
        record_detail(ui, record);
    }
}

fn record_detail(ui: &mut egui::Ui, record: &RecordFinding) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            status_badge(ui, level(record.class), tone(record.class));
            ui.strong(record.tier.explanation());
        });
        detail_row(
            ui,
            "Where the library last saw it",
            &record.old_path.to_string_lossy(),
        );
        match &record.target {
            Some(target) if record.tier.tied_to_file() => {
                detail_row(ui, "The file now", &target.to_string_lossy());
            }
            Some(target) => detail_row(
                ui,
                "Possible match (not accepted)",
                &target.to_string_lossy(),
            ),
            None if record.tier.tied_to_file() => detail_row(
                ui,
                "The file now",
                "Several files match; none is singled out",
            ),
            None => detail_row(ui, "The file now", "Not found"),
        }
        detail_row(ui, "Entry came from", record.set.label());
        for line in &record.evidence {
            ui.label(format!("• {line}"));
        }
        technical_details(
            ui,
            ("collection_reconcile_record", record.archive_id),
            |ui| {
                detail_row(ui, "Evidence code", record.code);
                detail_row(ui, "Library entry number", &record.archive_id.to_string());
                if let Some(size) = record.size_bytes {
                    detail_row(ui, "Recorded size", &format!("{} bytes", thousands(size)));
                }
                if let Some(seen) = &record.last_seen_at {
                    detail_row(ui, "Last seen", seen);
                }
                if let Some(flagged) = &record.flagged_missing_at {
                    detail_row(ui, "Marked \"file not found\"", flagged);
                }
            },
        );
    });
}

fn links(ui: &mut egui::Ui, state: &mut State, report: &ReconciliationReport) {
    let l = &report.links;
    ui.label("A link is a shortcut to a file kept somewhere else. Links are not library entries, so they are counted on their own and never as games.");
    card(ui, |ui| {
        detail_row(ui, "Links", &thousands(l.total));
        detail_row(ui, "Working", &thousands(l.resolve));
        detail_row(
            ui,
            "Broken: the storage they point at is not there",
            &thousands(l.storage_unavailable),
        );
        detail_row(
            ui,
            "Broken: the file is gone from a folder that is there",
            &thousands(l.target_missing),
        );
        ui.label("\"Storage not there\" covers a drive or remote folder that is not attached and a folder that was moved. It does not mean the game is lost.");
    });
    ui.label("Links that broke for the same reason are grouped.");
    let groups: Vec<_> = l
        .groups
        .iter()
        .filter(|group| group.grade != LinkGrade::Resolves)
        .collect();
    egui::ScrollArea::vertical()
        .id_salt("collection_reconcile_link_groups")
        .max_height(180.0)
        .show(ui, |ui| {
            for group in &groups {
                ui.horizontal_wrapped(|ui| {
                    status_badge(ui, group.grade.label(), link_tone(group.grade));
                    ui.label(format!(
                        "{} in {} point at {}: {}",
                        thousands(group.links),
                        group.link_folder.display(),
                        group.target_root.display(),
                        group.cause.map_or("", |cause| cause.label())
                    ));
                });
            }
        });
    let broken: Vec<(usize, &LinkFinding)> = report
        .link_findings
        .iter()
        .enumerate()
        .filter(|(_, link)| link.grade != LinkGrade::Resolves)
        .collect();
    if broken.is_empty() {
        card(ui, |ui| {
            ui.label("No broken links.");
        });
        return;
    }
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("collection_reconcile_links")
        .max_height((ui.available_height() - 150.0).max(140.0))
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, broken.len(), |ui, rows| {
            for row in rows {
                let (index, link) = broken[row];
                ui.push_id(("link", index), |ui| {
                    ui.horizontal(|ui| {
                        status_badge(ui, link.grade.label(), link_tone(link.grade));
                        let name = link.link.to_string_lossy();
                        if ui
                            .selectable_label(state.selected_link == Some(index), name)
                            .clicked()
                        {
                            clicked = Some(index);
                        }
                    });
                });
            }
        });
    if let Some(index) = clicked {
        state.selected_link = Some(index);
    }
    if let Some(link) = state
        .selected_link
        .and_then(|index| report.link_findings.get(index))
    {
        card(ui, |ui| {
            detail_row(ui, "Link", &link.link.to_string_lossy());
            detail_row(ui, "Points at", &link.resolved_target.to_string_lossy());
            if let Some(cause) = link.cause {
                detail_row(ui, "Why it is broken", cause.label());
            }
            match (&link.found, link.grade) {
                (Some(found), LinkGrade::TargetIdentified) => detail_row(
                    ui,
                    "Its target was probably moved to",
                    &found.to_string_lossy(),
                ),
                (Some(found), _) => detail_row(
                    ui,
                    "Possible match (not accepted)",
                    &found.to_string_lossy(),
                ),
                (None, _) => detail_row(ui, "Its file", "Not found"),
            }
            for line in &link.evidence {
                ui.label(format!("• {line}"));
            }
        });
    }
}

#[cfg(test)]
#[path = "collection_reconcile_tests.rs"]
mod tests;
