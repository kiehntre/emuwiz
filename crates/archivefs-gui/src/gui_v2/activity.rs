//! Shared, honest activity vocabulary. Progress is evidence, not animation.
use super::routes::Route;
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Queued,
    Running,
    Complete,
    Failed,
    Cancelled,
}

impl Phase {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Queued => "Waiting to start",
            Self::Running => "In progress",
            Self::Complete => "Finished",
            Self::Failed => "Needs attention",
            Self::Cancelled => "Stopped",
        }
    }
}

pub(super) struct Job {
    pub title: String,
    pub phase: Phase,
    pub progress: Option<(u64, u64)>,
    pub item: Option<String>,
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
        self.progress
            .filter(|(_, total)| *total > 0)
            .map(|(done, total)| (done.min(total) as f32 / total as f32).clamp(0.0, 1.0))
    }
    pub fn request_cancel(&self) {
        if self.active()
            && let Some(cancel) = &self.cancel
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
pub(super) struct Activity {
    pub jobs: BTreeMap<u64, Job>,
    next: u64,
}

impl Activity {
    pub fn queue(&mut self, title: &str, result: Route, cancellable: bool) -> u64 {
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
            job.phase = Phase::Running;
            job.started = Some(Instant::now());
            job.summary = "Working. You can keep browsing.".into();
        }
    }
    pub fn finish(&mut self, id: u64, summary: String, error: Option<String>) {
        if let Some(job) = self.jobs.get_mut(&id) {
            job.phase = if error.is_some() {
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
            job.technical = error.unwrap_or_default();
            job.finished = Some(Instant::now());
        }
    }
    pub fn running(&self) -> usize {
        self.jobs.values().filter(|job| job.active()).count()
    }
}
