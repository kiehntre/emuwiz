//! Blue "Scan history" on the Database status page.
//!
//! Read-only: recent `scan_runs` rows exactly as recorded (via
//! `Database::recent_scan_runs` on a read-only open), each run's per-source
//! coverage, and the Blue app's own scan as Activity already tracks it (the
//! shared `job_card`, so Cancel behaves exactly as everywhere else). Nothing
//! here starts, queues or cancels a scan.
//!
//! Never overstated: a run without a finish record is "No finish record", not
//! complete; an interrupted run's `finished_at` is when a later scan noticed
//! it; counters a run never wrote show as "not recorded", never as zero.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use archivefs_core::catalogue_health::{ScanCoverageState, SourceScanCoverage};
use archivefs_core::{
    Database, DiscoveryRunStatus, MAX_SCAN_HISTORY_RUNS, RecordedScanCounts, ScanRunCoverage,
    ScanRunRecord,
};
use eframe::egui::{self, RichText};

use super::activity::{Activity, Job};
use super::simple_shell::palette;
use crate::ui::components::{self as widgets, ActionStyle, StatusTone};
use crate::ui::theme::blue;

/// The Activity title of the Blue app's own folder scan (`App::load`).
pub(super) const SCAN_JOB_TITLE: &str = "Scanning your game folders";
/// Shown above the list: why a stopped scan may leave no entry.
pub(super) const UNSAVED_NOTE: &str = "A scan saves everything at once when it finishes. A scan that stopped before saving (closed, killed or out of memory) leaves no entry here, and the library database stays as it was before that scan.";
/// Runs shown before "Show older runs".
const SHOWN_FIRST: usize = 10;

type Loaded = Result<Vec<ScanRunRecord>, String>;

#[derive(Default)]
pub(super) struct ScanHistoryState {
    /// `None` keeps the page inert (tests, previews).
    database: Option<PathBuf>,
    loading: Option<Receiver<Loaded>>,
    runs: Option<Vec<ScanRunRecord>>,
    error: Option<String>,
    /// A scan was running last frame; reload once it has ended.
    saw_scan: bool,
    show_all: bool,
}

impl ScanHistoryState {
    pub(super) fn live() -> Self {
        Self {
            database: archivefs_core::default_database_path().ok(),
            ..Self::default()
        }
    }

    pub(super) fn loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Starts a read-only load. Never while a scan writes the catalogue: its
    /// lock would only make this wait, and the result would be stale.
    fn reload(&mut self, context: &egui::Context, scanning: bool) -> bool {
        if scanning || self.loading.is_some() {
            return false;
        }
        let Some(database) = self.database.clone() else {
            return false;
        };
        let (sender, receiver) = mpsc::channel();
        self.loading = Some(receiver);
        let context = context.clone();
        std::thread::spawn(move || {
            let _ = sender.send(load(&database));
            context.request_repaint();
        });
        true
    }

    fn poll(&mut self, context: &egui::Context, scanning: bool) {
        if let Some(receiver) = &self.loading {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    Err("The scan history reader stopped. Reload to try again.".into())
                }
            };
            self.loading = None;
            match result {
                Ok(runs) => {
                    self.runs = Some(runs);
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
        let ended = self.saw_scan && !scanning;
        self.saw_scan = scanning;
        if ended || (self.runs.is_none() && self.error.is_none()) {
            self.reload(context, scanning);
        }
    }

    #[cfg(test)]
    pub(super) fn with_runs(runs: Vec<ScanRunRecord>) -> Self {
        Self {
            runs: Some(runs),
            ..Self::default()
        }
    }
}

/// Reads the history from `database` without creating, migrating or
/// writing anything.
pub(super) fn load(database: &std::path::Path) -> Loaded {
    if !database.is_file() {
        return Err("There is no library database yet, so no scan has been recorded.".into());
    }
    Database::open_read_only(database)
        .and_then(|database| database.recent_scan_runs(MAX_SCAN_HISTORY_RUNS))
        .map_err(|error| error.to_string())
}

/// A run's state as its record supports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RunState {
    Completed,
    CompletedWithErrors,
    Partial,
    Failed,
    Interrupted,
    /// Still `running` in the record: no finish was ever written.
    NoFinishRecord,
    Unknown(String),
}

pub(super) fn classify(run: &ScanRunRecord) -> RunState {
    match run.state() {
        Some(DiscoveryRunStatus::Completed) => {
            let errors = run.counts.as_ref().is_some_and(|c| c.errors_count > 0);
            if errors || run.error_message.is_some() {
                RunState::CompletedWithErrors
            } else {
                RunState::Completed
            }
        }
        Some(DiscoveryRunStatus::Partial) => RunState::Partial,
        Some(DiscoveryRunStatus::Failed) => RunState::Failed,
        Some(DiscoveryRunStatus::Interrupted) => RunState::Interrupted,
        Some(DiscoveryRunStatus::Running) => RunState::NoFinishRecord,
        None => RunState::Unknown(run.status.clone()),
    }
}

impl RunState {
    pub(super) fn label(&self) -> String {
        match self {
            Self::Completed => "Completed".into(),
            Self::CompletedWithErrors => "Completed with errors".into(),
            Self::Partial => "Partial".into(),
            Self::Failed => "Failed".into(),
            Self::Interrupted => "Did not finish".into(),
            Self::NoFinishRecord => "No finish record".into(),
            Self::Unknown(status) => format!("Unknown state ({status})"),
        }
    }

    fn tone(&self) -> StatusTone {
        match self {
            Self::Completed => StatusTone::Success,
            Self::Failed => StatusTone::Blocked,
            Self::Unknown(_) => StatusTone::Pending,
            _ => StatusTone::Warning,
        }
    }

    /// What the state means, when the label alone is not enough.
    pub(super) fn explanation(&self) -> Option<&'static str> {
        match self {
            Self::CompletedWithErrors => {
                Some("The run finished, but at least one source could not be read.")
            }
            Self::Partial => Some(
                "Not every source was read completely, so games were not marked missing where reading was incomplete.",
            ),
            Self::Failed => Some("The run stopped with an error before it could finish."),
            Self::Interrupted => Some(
                "This run never recorded a finish. A later scan found it unfinished and marked it interrupted. It is not counted as complete.",
            ),
            Self::NoFinishRecord => Some(
                "This run never recorded a finish. It may still be running elsewhere or it stopped without finishing. It is not counted as complete.",
            ),
            Self::Unknown(_) => Some("This EmuWiz version does not know this recorded state."),
            Self::Completed => None,
        }
    }
}

/// Where a run came from, by its recorded `triggered_by`. All-sources scans
/// from Blue and from the command line record the same value.
pub(super) fn trigger_label(triggered_by: &str) -> String {
    match triggered_by {
        "cli-scan-all-sources" => "All enabled sources".into(),
        "cli-source-scan" => "One source".into(),
        "gui-scan-library" => "Library scan (database page)".into(),
        "cli-library-scan" => "Command-line library scan".into(),
        "gui-assign-source-platform" => "Source system change".into(),
        "catalogue-presence-reconciliation" => "Presence check (folders not read)".into(),
        other => format!("Recorded as \"{other}\""),
    }
}

/// `2026-11-13T01:13:27Z` -> `2026-11-13 01:13:27 UTC`; anything else as is.
pub(super) fn time_text(recorded: &str) -> String {
    match recorded.strip_suffix('Z') {
        Some(rest) if rest.len() == 19 && rest.as_bytes()[10] == b'T' => {
            format!("{} {} UTC", &rest[..10], &rest[11..])
        }
        _ => recorded.to_string(),
    }
}

/// The run's start/finish line.
pub(super) fn timing_text(run: &ScanRunRecord, state: &RunState) -> String {
    let started = format!("Started {}", time_text(&run.started_at));
    let duration = |seconds: Option<i64>| {
        seconds
            .map(|s| {
                format!(
                    " ({})",
                    super::job_card::duration(Duration::from_secs(s as u64))
                )
            })
            .unwrap_or_default()
    };
    match (state, &run.finished_at) {
        (RunState::Interrupted, Some(noticed)) => format!(
            "{started} · Found unfinished {} (not its real end)",
            time_text(noticed)
        ),
        (RunState::Interrupted, None) => format!("{started} · No finish recorded"),
        (RunState::NoFinishRecord, _) => format!("{started} · No finish recorded"),
        (RunState::Failed, Some(at)) => {
            format!(
                "{started} · Failed {}{}",
                time_text(at),
                duration(run.recorded_seconds)
            )
        }
        (_, Some(at)) => format!(
            "{started} · Finished {}{}",
            time_text(at),
            duration(run.recorded_seconds)
        ),
        (_, None) => format!("{started} · Finish time not recorded"),
    }
}

fn number(value: i64) -> String {
    if value < 0 {
        value.to_string()
    } else {
        super::job_card::count(value as u64)
    }
}

/// The recorded counters, or why there are none.
pub(super) fn counts_text(counts: Option<&RecordedScanCounts>) -> String {
    let Some(c) = counts else {
        return "Counts: not recorded (this run did not finish).".into();
    };
    let optional = |value: Option<i64>| value.map_or_else(|| "not recorded".into(), number);
    let skipped = match (
        c.skipped_unsupported_extension,
        c.skipped_ambiguous_platform,
    ) {
        (Some(unsupported), Some(ambiguous)) => number(unsupported + ambiguous),
        _ => "not recorded".into(),
    };
    format!(
        "{} {} scanned · {} seen · {} added · {} updated · {} marked missing · {} unchanged · {} skipped · {} {}",
        number(c.source_folders_scanned),
        if c.source_folders_scanned == 1 {
            "source"
        } else {
            "sources"
        },
        number(c.archives_seen),
        number(c.archives_added),
        number(c.archives_updated),
        number(c.archives_missing),
        optional(c.archives_unchanged),
        skipped,
        number(c.errors_count),
        if c.errors_count == 1 {
            "error"
        } else {
            "errors"
        },
    )
}

pub(super) fn coverage_label(state: ScanCoverageState) -> &'static str {
    match state {
        ScanCoverageState::Complete => "Read completely",
        ScanCoverageState::Partial => "Partly read",
        ScanCoverageState::Unavailable => "Unavailable",
        ScanCoverageState::Failed => "Failed",
        ScanCoverageState::Skipped => "Skipped",
        ScanCoverageState::Removed => "No longer in settings",
        ScanCoverageState::NotAttempted => "Not attempted",
    }
}

/// One line per source the run recorded, plus one summary line for sources
/// that were no longer configured (they are never scanned).
pub(super) fn coverage_lines(coverage: &ScanRunCoverage) -> Vec<String> {
    let recorded = match coverage {
        ScanRunCoverage::NotRecorded => {
            return vec!["Sources: not recorded for this run.".into()];
        }
        ScanRunCoverage::Unreadable(error) => {
            return vec![format!("Sources: the record could not be read ({error}).")];
        }
        ScanRunCoverage::Recorded(recorded) => recorded,
    };
    let mut lines: Vec<String> = recorded
        .iter()
        .filter(|source| source.state != ScanCoverageState::Removed)
        .map(source_line)
        .collect();
    let removed = recorded
        .iter()
        .filter(|source| source.state == ScanCoverageState::Removed)
        .count();
    if removed > 0 {
        lines.push(format!(
            "{removed} {} no longer in settings (not scanned).",
            if removed == 1 { "source" } else { "sources" }
        ));
    }
    lines
}

fn source_line(source: &SourceScanCoverage) -> String {
    let mut line = format!(
        "{} — {}",
        source.root.display(),
        coverage_label(source.state)
    );
    if let Some(diagnostic) = &source.diagnostic {
        line.push_str(&format!(": {diagnostic}"));
    }
    let excluded = source.excluded_roots.len();
    if excluded > 0 {
        line.push_str(&format!(
            " · {excluded} {} inside it kept out of its missing-game check",
            if excluded == 1 { "folder" } else { "folders" }
        ));
    }
    line
}

/// The Blue app's scan this session: the running one, else the newest
/// finished one Activity still lists.
pub(super) fn session_scan_job(activity: &Activity, current: Option<u64>) -> Option<&Job> {
    current.and_then(|id| activity.jobs.get(&id)).or_else(|| {
        activity
            .jobs
            .values()
            .rev()
            .find(|job| job.title == SCAN_JOB_TITLE)
    })
}

/// Draws the history. `scan_job` is Blue's own scan (running or last this
/// session); `upgrading` is the Database status upgrade scan.
pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut ScanHistoryState,
    scan_job: Option<&Job>,
    upgrading: bool,
) {
    let scanning = upgrading || scan_job.is_some_and(Job::active);
    state.poll(ui.ctx(), scanning);
    ui.add_space(8.0);
    super::game_detail_blue::side_card(ui, "SCAN HISTORY", |ui| {
        current_scan(ui, scan_job, upgrading);
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                RichText::new(UNSAVED_NOTE)
                    .size(13.0)
                    .color(palette::SECONDARY_TEXT),
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let context = ui.ctx().clone();
            if widgets::action_button(
                ui,
                "Reload scan history",
                ActionStyle::Secondary,
                !state.loading() && !scanning,
            )
            .on_hover_text(
                "Reads the runs already saved in the library database. It does not scan.",
            )
            .on_disabled_hover_text(if scanning {
                "Reloads when the running scan has finished."
            } else {
                "Loading scan history…"
            })
            .clicked()
            {
                state.reload(&context, scanning);
            }
            if state.loading() {
                ui.spinner();
                ui.label(RichText::new("Loading scan history…").color(palette::SECONDARY_TEXT));
            }
        });
        if let Some(error) = &state.error {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("Scan history could not be read. {error}"))
                        .color(blue::WARN),
                )
                .wrap(),
            );
        }
        match &state.runs {
            None => {}
            Some(runs) if runs.is_empty() => {
                ui.label(
                    RichText::new("No scan has been recorded in this library database.")
                        .color(blue::BODY_TEXT),
                );
            }
            Some(runs) => {
                let shown = if state.show_all {
                    runs.len()
                } else {
                    runs.len().min(SHOWN_FIRST)
                };
                for run in &runs[..shown] {
                    ui.add_space(6.0);
                    run_card(ui, run);
                }
                if runs.len() > SHOWN_FIRST {
                    let label = if state.show_all {
                        "Show fewer runs".to_string()
                    } else {
                        format!("Show older runs ({} more)", runs.len() - SHOWN_FIRST)
                    };
                    if widgets::action_button(ui, &label, ActionStyle::Secondary, true).clicked() {
                        state.show_all = !state.show_all;
                    }
                }
            }
        }
    });
}

fn current_scan(ui: &mut egui::Ui, scan_job: Option<&Job>, upgrading: bool) {
    if upgrading {
        widgets::status_badge(ui, "In progress", StatusTone::Active);
        ui.label(
            RichText::new(
                "A database upgrade scan is running. It appears below once it has been saved.",
            )
            .color(blue::BODY_TEXT),
        );
        return;
    }
    let Some(job) = scan_job else {
        ui.label(
            RichText::new("No folder scan has run from Blue in this session.")
                .color(palette::SECONDARY_TEXT),
        );
        return;
    };
    ui.label(
        RichText::new(if job.active() {
            "Current scan"
        } else {
            "Last scan in this session"
        })
        .color(blue::DIM_TEXT),
    );
    // The shared card: same progress, timing and Cancel rules as Activity.
    if super::job_card::show(ui, job, false).cancel {
        job.request_cancel();
    }
    if job.active() && job.progress.is_none() {
        ui.label(
            RichText::new("This scan does not report how far it has got.")
                .color(palette::SECONDARY_TEXT),
        );
    }
    if job.active() {
        ui.label(
            RichText::new("It appears below once it has been saved.")
                .color(palette::SECONDARY_TEXT),
        );
    }
}

fn run_card(ui: &mut egui::Ui, run: &ScanRunRecord) {
    let state = classify(run);
    egui::Frame::new()
        .fill(blue::INSET)
        .stroke(egui::Stroke::new(1.0_f32, blue::BORDER_SUBTLE))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                widgets::status_badge(ui, state.label(), state.tone());
                ui.label(
                    RichText::new(format!(
                        "Run {} · {}",
                        run.scan_run_id,
                        trigger_label(&run.triggered_by)
                    ))
                    .strong()
                    .color(blue::PRIMARY_TEXT),
                )
                .on_hover_text(format!(
                    "scan_runs id {}, status \"{}\", triggered_by \"{}\"",
                    run.scan_run_id, run.status, run.triggered_by
                ));
            });
            let line = |ui: &mut egui::Ui, text: String, color| {
                ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
            };
            line(ui, timing_text(run, &state), blue::BODY_TEXT);
            if let Some(explanation) = state.explanation() {
                line(ui, explanation.to_string(), palette::SECONDARY_TEXT);
            }
            line(ui, counts_text(run.counts.as_ref()), blue::BODY_TEXT);
            for source in coverage_lines(&run.coverage) {
                line(ui, source, blue::BODY_TEXT);
            }
            if let Some(reason) = &run.error_message {
                line(ui, format!("Recorded reason: {reason}"), blue::WARN);
            }
        });
}

#[cfg(test)]
#[path = "scan_history_tests.rs"]
mod tests;
