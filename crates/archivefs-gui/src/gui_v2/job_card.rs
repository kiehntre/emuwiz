//! The one place a running or finished job is described to a person.
//!
//! The wording is built by pure functions (so every state is unit-testable) and
//! drawn by one small egui helper that the Activity page, the status bar and
//! the pages that start a job all share. State lives in `Activity`, never in a
//! page widget, so progress survives leaving and returning to a page.
use super::activity::{Job, Phase};
use eframe::egui;
use std::time::Duration;

/// 38421 -> "38,421".
pub(super) fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "42s", "2m 14s", "1h 05m".
pub(super) fn duration(value: Duration) -> String {
    let seconds = value.as_secs();
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// "about 4m", rounded up so it never promises less time than it takes.
fn about(value: Duration) -> String {
    let seconds = value.as_secs();
    match seconds {
        0..=59 => format!("about {}s", seconds.div_ceil(5) * 5),
        60..=3599 => format!("about {}m", seconds.div_ceil(60)),
        _ => format!(
            "about {}h {:02}m",
            seconds / 3600,
            (seconds % 3600).div_ceil(60) % 60
        ),
    }
}

/// What a card says, as plain text lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct JobText {
    pub title: String,
    pub status: &'static str,
    /// The current phase, when it adds something to the title.
    pub phase: Option<String>,
    /// "38,421 / 104,525 files · 36%" or "12,831 files so far".
    pub counts: Option<String>,
    /// "2m 14s elapsed · about 4m remaining".
    pub timing: Option<String>,
    /// A short note from the worker, never a path dump.
    pub note: Option<String>,
    /// Why Cancel behaves as it does.
    pub cancel_hint: Option<&'static str>,
}

pub(super) fn describe(job: &Job) -> JobText {
    let progress = job
        .progress
        .as_ref()
        .filter(|_| job.phase == Phase::Running);
    let counts = progress.map(|progress| match (progress.total, progress.percent()) {
        (Some(total), Some(percent)) => format!(
            "{} / {} {} · {percent}%",
            count(progress.completed),
            count(total),
            progress.unit
        ),
        _ => format!("{} {} so far", count(progress.completed), progress.unit),
    });
    let timing = match job.phase {
        Phase::Running => {
            let mut text = format!("{} elapsed", duration(job.elapsed()));
            if let Some(remaining) = job.eta() {
                text.push_str(&format!(" · {} remaining", about(remaining)));
            } else if progress.is_some_and(|p| p.total.is_some())
                && job.elapsed() >= Duration::from_secs(3)
            {
                text.push_str(" · Estimating…");
            }
            Some(text)
        }
        Phase::Complete | Phase::Failed | Phase::Cancelled => {
            Some(format!("{} total", duration(job.elapsed())))
        }
        _ => None,
    };
    JobText {
        title: job.title.clone(),
        status: job.status_label(),
        phase: progress
            .map(|progress| progress.phase.clone())
            .filter(|phase| *phase != job.title),
        counts,
        timing,
        note: progress.and_then(|progress| progress.message.clone()),
        cancel_hint: (job.active() && job.cancel.is_some()).then(|| job.cancel_policy.hint()),
    }
}

/// What the person asked for on a card.
#[derive(Default)]
pub(super) struct JobCardResponse {
    pub cancel: bool,
}

/// Draws a job. `compact` is the form used beside a page's own controls and in
/// the status bar; the Activity page passes `false` for the fuller card.
pub(super) fn show(ui: &mut egui::Ui, job: &Job, compact: bool) -> JobCardResponse {
    let text = describe(job);
    let mut response = JobCardResponse::default();
    ui.horizontal(|ui| {
        if job.phase == Phase::Running {
            ui.spinner();
        }
        ui.strong(&text.title);
        if !compact || job.cancelling() {
            ui.label(text.status);
        }
    });
    if let Some(phase) = &text.phase {
        ui.label(phase);
    }
    if job.cancelling() {
        ui.label("Waiting for a safe stopping point. Nothing is marked cancelled until work has stopped.");
    }
    if let Some(counts) = &text.counts {
        ui.label(counts);
    }
    if let Some(fraction) = job.fraction() {
        ui.add(egui::ProgressBar::new(fraction));
    }
    if let Some(note) = &text.note {
        ui.label(note);
    }
    if let Some(timing) = &text.timing {
        ui.label(timing);
    }
    if !job.active() && !job.summary.is_empty() {
        ui.label(&job.summary);
    }
    if job.can_cancel() {
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                response.cancel = true;
            }
            if !compact && let Some(hint) = text.cancel_hint {
                ui.label(hint);
            }
        });
    } else if job.active() && !job.cancelling() && !compact && job.cancel.is_none() {
        ui.label(job.cancel_policy.hint());
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_v2::{
        activity::{Activity, CancelPolicy, JobProgress, Settled},
        routes::Route,
    };

    fn running(policy: CancelPolicy) -> (Activity, u64) {
        let mut activity = Activity::default();
        let id = activity.queue_with("Scanning for duplicates", Route::Home, policy);
        activity.start(id);
        (activity, id)
    }

    #[test]
    fn counts_and_durations_read_naturally() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(38_421), "38,421");
        assert_eq!(count(104_525), "104,525");
        assert_eq!(count(1_000_000), "1,000,000");
        assert_eq!(duration(Duration::from_secs(42)), "42s");
        assert_eq!(duration(Duration::from_secs(134)), "2m 14s");
        assert_eq!(duration(Duration::from_secs(3900)), "1h 05m");
        assert_eq!(about(Duration::from_secs(61)), "about 2m");
    }

    #[test]
    fn known_totals_show_a_count_and_percentage() {
        let (mut activity, id) = running(CancelPolicy::SafeNow);
        activity.update_progress(
            id,
            JobProgress::new("Hashing candidate files", "files")
                .with_total(104_525)
                .at(38_421),
        );
        let text = describe(&activity.jobs[&id]);
        assert_eq!(text.counts.as_deref(), Some("38,421 / 104,525 files · 36%"));
        assert_eq!(text.phase.as_deref(), Some("Hashing candidate files"));
    }

    #[test]
    fn unknown_totals_show_a_count_and_never_a_percentage() {
        let (mut activity, id) = running(CancelPolicy::SafeNow);
        activity.update_progress(
            id,
            JobProgress::new("Reading archive metadata", "files").at(12_831),
        );
        let text = describe(&activity.jobs[&id]);
        assert_eq!(text.counts.as_deref(), Some("12,831 files so far"));
        assert!(!text.counts.unwrap().contains('%'));
        assert!(!text.timing.unwrap().contains("Estimating"));
    }

    #[test]
    fn the_estimate_is_omitted_until_it_is_defensible() {
        let (mut activity, id) = running(CancelPolicy::SafeNow);
        activity.update_progress(
            id,
            JobProgress::new("Scanning", "games").with_total(1000).at(1),
        );
        let early = describe(&activity.jobs[&id]).timing.unwrap();
        assert!(
            !early.contains("remaining") && !early.contains("0s remaining"),
            "{early}"
        );
        // With enough work and time the estimate appears, and is a rounded-up "about".
        let job = activity.jobs.get_mut(&id).unwrap();
        job.started = Some(std::time::Instant::now() - Duration::from_secs(60));
        job.progress = Some(
            JobProgress::new("Scanning", "games")
                .with_total(1000)
                .at(250),
        );
        let later = describe(&activity.jobs[&id]).timing.unwrap();
        assert!(
            later.contains("elapsed") && later.contains("about") && later.contains("remaining"),
            "{later}"
        );
        // A stalled start says "Estimating…" rather than inventing a figure.
        let job = activity.jobs.get_mut(&id).unwrap();
        job.progress = Some(JobProgress::new("Scanning", "games").with_total(1000).at(1));
        assert!(
            describe(&activity.jobs[&id])
                .timing
                .unwrap()
                .contains("Estimating…")
        );
    }

    #[test]
    fn cancelling_is_not_cancelled_until_the_worker_has_stopped() {
        let (mut activity, id) = running(CancelPolicy::AfterCurrentStep);
        assert!(activity.jobs[&id].can_cancel());
        activity.jobs[&id].request_cancel();
        let job = &activity.jobs[&id];
        assert!(job.cancelling());
        assert_eq!(job.status_label(), "Cancelling…");
        assert_ne!(job.phase, Phase::Cancelled, "not claimed early");
        assert!(!job.can_cancel(), "no second press while stopping");
        // the worker acknowledges by stopping
        activity.settle(id, "Stopped at a safe point.".into(), Settled::Stopped);
        assert_eq!(activity.jobs[&id].phase, Phase::Cancelled);
        assert_eq!(activity.jobs[&id].status_label(), "Stopped");
    }

    #[test]
    fn cancel_is_never_offered_for_an_operation_that_must_finish() {
        let (activity, id) = running(CancelPolicy::NotCancellable);
        activity.jobs[&id].request_cancel();
        assert!(!activity.jobs[&id].cancelling());
        assert!(!activity.jobs[&id].can_cancel());
        assert!(describe(&activity.jobs[&id]).cancel_hint.is_none());
        assert!(CancelPolicy::NotCancellable.hint().contains("finish"));
    }

    #[test]
    fn a_finished_job_cannot_be_revived_by_late_progress_or_a_late_start() {
        let (mut activity, id) = running(CancelPolicy::SafeNow);
        activity.settle(id, "Done".into(), Settled::Done);
        activity.update_progress(id, JobProgress::new("Late", "files").with_total(10).at(3));
        activity.start(id);
        assert_eq!(activity.jobs[&id].phase, Phase::Complete);
        assert!(activity.jobs[&id].progress.is_none());
        // and a late failure cannot overwrite a completed result
        activity.settle(id, "Boom".into(), Settled::Failed("raw".into()));
        assert_eq!(activity.jobs[&id].phase, Phase::Complete);
    }

    #[test]
    fn a_worker_that_finished_before_noticing_cancel_is_finished_not_stopped() {
        let (mut activity, id) = running(CancelPolicy::AfterCurrentStep);
        activity.jobs[&id].request_cancel();
        activity.settle(id, "All changes were applied.".into(), Settled::Done);
        assert_eq!(activity.jobs[&id].phase, Phase::Complete);
        // the same race through the legacy finish path with the worker's own marker
        let other = activity.queue_with("Other", Route::Home, CancelPolicy::SafeNow);
        activity.start(other);
        activity.finish(
            other,
            "Stopped".into(),
            Some(crate::gui_v2::activity::CANCELLED.into()),
        );
        assert_eq!(activity.jobs[&other].phase, Phase::Cancelled);
        assert!(activity.jobs[&other].technical.is_empty());
    }

    #[test]
    fn progress_inside_a_phase_never_goes_backwards_but_a_new_phase_starts_over() {
        let (mut activity, id) = running(CancelPolicy::SafeNow);
        let hashing = |done| {
            JobProgress::new("Hashing", "files")
                .with_total(100)
                .at(done)
        };
        activity.update_progress(id, hashing(60));
        activity.update_progress(id, hashing(40));
        assert_eq!(activity.jobs[&id].progress.as_ref().unwrap().completed, 60);
        activity.update_progress(
            id,
            JobProgress::new("Comparing", "files").with_total(5).at(1),
        );
        let progress = activity.jobs[&id].progress.as_ref().unwrap();
        assert_eq!(
            (progress.phase.as_str(), progress.completed),
            ("Comparing", 1)
        );
    }

    #[test]
    fn a_completed_job_shows_a_result_with_its_duration_instead_of_a_spinner() {
        let (mut activity, id) = running(CancelPolicy::SafeNow);
        activity.settle(
            id,
            "104,525 games checked · 312 duplicate groups found".into(),
            Settled::Done,
        );
        let job = &activity.jobs[&id];
        let text = describe(job);
        assert_eq!(text.status, "Finished");
        assert!(text.counts.is_none(), "no live counters on a finished job");
        assert!(text.timing.unwrap().ends_with("total"));
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                show(ui, job, false);
            });
        });
        let rendered = rendered_text(&output);
        assert!(
            rendered.contains("312 duplicate groups found"),
            "{rendered}"
        );
        assert!(!rendered.contains("Cancel"));
    }

    fn rendered_text(output: &egui::FullOutput) -> String {
        fn gather(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| gather(shape, out)),
                _ => {}
            }
        }
        let mut out = String::new();
        output
            .shapes
            .iter()
            .for_each(|clipped| gather(&clipped.shape, &mut out));
        out
    }

    fn card_text(job: &Job) -> String {
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                show(ui, job, false);
            });
        });
        rendered_text(&output)
    }

    #[test]
    fn the_card_renders_counts_percent_and_a_cancel_button_while_running() {
        let (mut activity, id) = running(CancelPolicy::SafeNow);
        activity.update_progress(
            id,
            JobProgress::new("Hashing candidate files", "files")
                .with_total(104_525)
                .at(38_421),
        );
        let rendered = card_text(&activity.jobs[&id]);
        assert!(
            rendered.contains("38,421 / 104,525 files · 36%"),
            "{rendered}"
        );
        assert!(rendered.contains("Hashing candidate files"));
        assert!(rendered.contains("Cancel"));
        // unknown totals: a count, no percent sign anywhere
        let (mut activity, id) = running(CancelPolicy::SafeNow);
        activity.update_progress(
            id,
            JobProgress::new("Reading archive metadata", "files").at(12_831),
        );
        let rendered = card_text(&activity.jobs[&id]);
        assert!(rendered.contains("12,831 files so far"), "{rendered}");
        assert!(!rendered.contains('%'), "{rendered}");
    }

    #[test]
    fn a_cancelling_card_does_not_claim_cancelled_and_offers_no_second_cancel() {
        let (activity, id) = running(CancelPolicy::AfterCurrentStep);
        activity.jobs[&id].request_cancel();
        let rendered = card_text(&activity.jobs[&id]);
        assert!(rendered.contains("Cancelling…"), "{rendered}");
        assert!(!rendered.contains("Stopped") && !rendered.contains("Cancelled"));
        assert!(!rendered.contains("\nCancel\n"), "{rendered}");
    }

    #[test]
    fn a_failed_job_card_shows_the_plain_reason_not_the_raw_error() {
        let (mut activity, id) = running(CancelPolicy::SafeNow);
        activity.settle(
            id,
            "Duplicate scan stopped. Some files could not be read. Try again.".into(),
            Settled::Failed("Os { code: 13, kind: PermissionDenied }".into()),
        );
        let rendered = card_text(&activity.jobs[&id]);
        assert!(rendered.contains("Duplicate scan stopped"), "{rendered}");
        assert!(!rendered.contains("Os { code"), "{rendered}");
        assert_eq!(
            activity.jobs[&id].technical,
            "Os { code: 13, kind: PermissionDenied }"
        );
    }
}
