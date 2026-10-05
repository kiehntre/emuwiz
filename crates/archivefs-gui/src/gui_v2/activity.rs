//! Shared, honest activity vocabulary. Progress is evidence, not animation.
use super::routes::Route;
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub(super) use archivefs_core::job_progress::JobProgress;

/// What a worker reports when it noticed the cancel flag and stopped. It is the
/// only way a job becomes "Stopped": a worker that finished its work reports
/// success even if the person pressed Cancel a moment too late.
pub(super) const CANCELLED: &str = "The operation was cancelled.";

/// How soon a running job can honour Cancel. Cooperative only: a worker is
/// never killed part-way through a database or filesystem change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CancelPolicy {
    /// Stops between units of work; nothing is half done.
    SafeNow,
    /// The current atomic step finishes first, then it stops at a safe point.
    AfterCurrentStep,
    /// Must run to the end to stay consistent.
    NotCancellable,
}

impl CancelPolicy {
    pub(super) fn hint(self) -> &'static str {
        match self {
            Self::SafeNow => "Stops right away; nothing is left half done.",
            Self::AfterCurrentStep => "Stops after the current step finishes, at a safe point.",
            Self::NotCancellable => "This has to finish to keep your files consistent.",
        }
    }
}

/// How a worker's run ended, as the worker itself reports it.
pub(super) enum Settled {
    Done,
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Queued,
    Running,
    Complete,
    Failed,
    Cancelled,
    Superseded,
}

impl Phase {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Queued => "Waiting to start",
            Self::Running => "In progress",
            Self::Complete => "Finished",
            Self::Failed => "Needs attention",
            Self::Cancelled => "Stopped",
            Self::Superseded => "Superseded",
        }
    }
}

pub(super) struct Job {
    pub title: String,
    pub phase: Phase,
    pub progress: Option<JobProgress>,
    pub item: Option<String>,
    pub cancel_policy: CancelPolicy,
    pub summary: String,
    pub technical: String,
    pub result: Route,
    pub cancel: Option<Arc<AtomicBool>>,
    pub started: Option<Instant>,
    pub finished: Option<Instant>,
}

impl Job {
    pub fn active(&self) -> bool {
        matches!(self.phase, Phase::Queued | Phase::Running)
    }
    pub fn elapsed(&self) -> Duration {
        self.started.map_or(Duration::ZERO, |start| {
            self.finished
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(start)
        })
    }
    pub fn fraction(&self) -> Option<f32> {
        self.progress.as_ref().and_then(JobProgress::fraction)
    }
    /// A defensible estimate of the time left; see `job_progress::eta`.
    pub fn eta(&self) -> Option<Duration> {
        if self.phase != Phase::Running {
            return None;
        }
        self.progress
            .as_ref()
            .and_then(|progress| archivefs_core::job_progress::eta(self.elapsed(), progress))
    }
    /// Cancel was asked for and the worker has not yet acknowledged it by
    /// finishing. Never "Stopped" until the worker has actually stopped.
    pub fn cancelling(&self) -> bool {
        self.active()
            && self
                .cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }
    pub fn can_cancel(&self) -> bool {
        self.active()
            && !self.cancelling()
            && self.cancel.is_some()
            && self.cancel_policy != CancelPolicy::NotCancellable
    }
    /// The status in words: "Cancelling…" while a stop is pending.
    pub fn status_label(&self) -> &'static str {
        if self.cancelling() {
            "Cancelling…"
        } else {
            self.phase.label()
        }
    }
    pub fn request_cancel(&self) {
        if self.active()
            && self.cancel_policy != CancelPolicy::NotCancellable
            && let Some(cancel) = &self.cancel
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The jobs Activity lists, newest first. With `attention_only`, just the ones
/// that need attention; nothing is removed from the history either way.
pub(super) fn visible_jobs(activity: &Activity, attention_only: bool) -> Vec<(&u64, &Job)> {
    activity
        .jobs
        .iter()
        .rev()
        .filter(|(_, job)| !attention_only || job.phase == Phase::Failed)
        .collect()
}

#[derive(Default)]
pub(super) struct Activity {
    pub jobs: BTreeMap<u64, Job>,
    next: u64,
}

impl Activity {
    pub fn queue(&mut self, title: &str, result: Route, cancellable: bool) -> u64 {
        // The weakest honest claim for a job that never said how it stops.
        self.queue_with(
            title,
            result,
            if cancellable {
                CancelPolicy::AfterCurrentStep
            } else {
                CancelPolicy::NotCancellable
            },
        )
    }
    pub fn queue_with(&mut self, title: &str, result: Route, policy: CancelPolicy) -> u64 {
        let cancellable = policy != CancelPolicy::NotCancellable;
        self.next += 1;
        // A session history is bounded; never evict running operations.
        if self.jobs.len() >= 100
            && let Some(id) = self
                .jobs
                .iter()
                .find(|(_, job)| !job.active())
                .map(|(id, _)| *id)
        {
            self.jobs.remove(&id);
        }
        self.jobs.insert(
            self.next,
            Job {
                title: title.into(),
                phase: Phase::Queued,
                progress: None,
                item: None,
                cancel_policy: policy,
                summary: "Waiting to start.".into(),
                technical: String::new(),
                result,
                cancel: cancellable.then(|| Arc::new(AtomicBool::new(false))),
                started: None,
                finished: None,
            },
        );
        self.next
    }
    pub fn start(&mut self, id: u64) {
        if let Some(job) = self.jobs.get_mut(&id) {
            if job.phase != Phase::Queued {
                return;
            }
            job.phase = Phase::Running;
            job.started = Some(Instant::now());
            job.summary = "Working. You can keep browsing.".into();
        }
    }
    pub fn finish(&mut self, id: u64, summary: String, error: Option<String>) {
        if let Some(job) = self.jobs.get_mut(&id)
            && job.active()
        {
            let stopped = error.as_deref() == Some(CANCELLED);
            job.phase = if stopped {
                Phase::Cancelled
            } else if error.is_some() {
                Phase::Failed
            } else if job
                .cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                Phase::Cancelled
            } else {
                Phase::Complete
            };
            job.summary = summary;
            job.technical = if stopped {
                String::new()
            } else {
                error.unwrap_or_default()
            };
            job.finished = Some(Instant::now());
        }
    }

    /// Applies a progress report. Ignored unless the job is running, so a late
    /// message can never revive a finished job, and ignored when it would move
    /// work backwards inside the same phase.
    pub fn update_progress(&mut self, id: u64, next: JobProgress) {
        if let Some(job) = self.jobs.get_mut(&id)
            && job.phase == Phase::Running
        {
            let updated = match &job.progress {
                Some(current) => current.advanced_by(&next),
                None => Some(next),
            };
            if updated.is_some() {
                job.progress = updated;
            }
        }
    }

    /// Ends a job by what its worker reported. A worker that finished is
    /// "Finished" even if Cancel was pressed too late to matter; only a worker
    /// that stopped early is "Stopped". Already finished jobs never change.
    pub fn settle(&mut self, id: u64, summary: String, how: Settled) {
        if let Some(job) = self.jobs.get_mut(&id)
            && job.active()
        {
            job.phase = match &how {
                Settled::Done => Phase::Complete,
                Settled::Failed(_) => Phase::Failed,
                Settled::Stopped => Phase::Cancelled,
            };
            job.summary = summary;
            job.technical = match how {
                Settled::Failed(technical) => technical,
                _ => String::new(),
            };
            job.finished = Some(Instant::now());
        }
    }

    pub fn failed(&self) -> usize {
        self.jobs
            .values()
            .filter(|job| job.phase == Phase::Failed)
            .count()
    }

    pub fn supersede(&mut self, id: u64, summary: String) {
        if let Some(job) = self.jobs.get_mut(&id)
            && job.active()
        {
            job.phase = Phase::Superseded;
            job.summary = summary;
            job.finished = Some(Instant::now());
        }
    }

    pub fn queued(&self) -> usize {
        self.jobs
            .values()
            .filter(|job| job.phase == Phase::Queued)
            .count()
    }

    pub fn running(&self) -> usize {
        self.jobs
            .values()
            .filter(|job| job.phase == Phase::Running)
            .count()
    }

    pub fn active(&self) -> usize {
        self.queued() + self.running()
    }
}
