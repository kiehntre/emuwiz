//! One progress vocabulary for long operations.
//!
//! Progress is evidence, not animation: a phase, work done, and a total only
//! when the operation really knows it. Nothing here invents a percentage, and
//! an estimate of the time remaining is offered only when it is defensible.

use std::time::{Duration, Instant};

/// Where a long operation is right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobProgress {
    /// What the operation is doing, in plain words ("Hashing candidates").
    pub phase: String,
    /// Units of work finished in this phase. Never above `total` when known.
    pub completed: u64,
    /// Units of work in this phase, when the operation actually knows it.
    pub total: Option<u64>,
    /// What one unit is ("files", "games").
    pub unit: &'static str,
    /// A short, optional note (never a path dump).
    pub message: Option<String>,
}

impl JobProgress {
    pub fn new(phase: impl Into<String>, unit: &'static str) -> Self {
        Self {
            phase: phase.into(),
            completed: 0,
            total: None,
            unit,
            message: None,
        }
    }

    /// A zero total means "nothing to do", not "unknown": it is treated as
    /// unknown so no fraction is ever computed from it.
    pub fn with_total(mut self, total: u64) -> Self {
        self.total = (total > 0).then_some(total);
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn at(mut self, completed: u64) -> Self {
        self.completed = self.total.map_or(completed, |total| completed.min(total));
        self
    }

    /// Done / total in `0.0..=1.0`, only when the total is known.
    pub fn fraction(&self) -> Option<f32> {
        self.total
            .filter(|total| *total > 0)
            .map(|total| (self.completed.min(total) as f32 / total as f32).clamp(0.0, 1.0))
    }

    /// Whole percent, rounded down so "100%" means finished.
    pub fn percent(&self) -> Option<u8> {
        self.fraction().map(|fraction| (fraction * 100.0) as u8)
    }

    /// The next reported state, or `None` when it must be ignored. Within a
    /// phase work only moves forward and never exceeds a known total; a
    /// different phase starts again from its own numbers.
    pub fn advanced_by(&self, next: &JobProgress) -> Option<JobProgress> {
        let mut next = next.clone();
        if let Some(total) = next.total {
            next.completed = next.completed.min(total);
        }
        if next.phase == self.phase {
            if next.completed < self.completed {
                return None;
            }
            // A known total is never forgotten within a phase.
            if next.total.is_none() {
                next.total = self.total;
                next.completed = self.total.map_or(next.completed, |t| next.completed.min(t));
            }
        }
        Some(next)
    }
}

/// A defensible estimate of the time left, or `None`. It needs a known total,
/// at least 5% of the work, and a few seconds of evidence, so an early burst
/// or a stalled start never produces nonsense such as "0% · 0 seconds left".
pub fn eta(elapsed: Duration, progress: &JobProgress) -> Option<Duration> {
    const MIN_ELAPSED: Duration = Duration::from_secs(3);
    const MAX_ETA: Duration = Duration::from_secs(24 * 60 * 60);
    let total = progress.total?;
    let done = progress.completed.min(total);
    if done == 0 || done >= total || elapsed < MIN_ELAPSED || done * 20 < total {
        return None;
    }
    let remaining = elapsed.as_secs_f64() * (total - done) as f64 / done as f64;
    let remaining = Duration::from_secs_f64(remaining);
    (remaining <= MAX_ETA).then_some(remaining)
}

/// Coalesces high-frequency updates for one operation: a worker may call
/// [`ProgressReporter::tick`] for every file and only a bounded number of
/// updates reach the interface. A new phase and the final state always pass.
pub struct ProgressReporter<'a> {
    sink: &'a mut dyn FnMut(JobProgress),
    interval: Duration,
    last_sent: Option<Instant>,
    current: Option<JobProgress>,
    pending: bool,
}

impl<'a> ProgressReporter<'a> {
    pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(150);

    pub fn new(sink: &'a mut dyn FnMut(JobProgress)) -> Self {
        Self::with_interval(sink, Self::DEFAULT_INTERVAL)
    }

    pub fn with_interval(sink: &'a mut dyn FnMut(JobProgress), interval: Duration) -> Self {
        Self {
            sink,
            interval,
            last_sent: None,
            current: None,
            pending: false,
        }
    }

    /// Begins a phase. Always reported (a phase change is a real checkpoint).
    pub fn phase(&mut self, name: &str, unit: &'static str, total: Option<u64>) {
        self.flush();
        let mut progress = JobProgress::new(name, unit);
        if let Some(total) = total {
            progress = progress.with_total(total);
        }
        self.send(progress);
    }

    /// Records work done. Cheap enough for a hot loop: most calls only update
    /// a counter.
    pub fn tick(&mut self, completed: u64) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        current.completed = current.total.map_or(completed, |t| completed.min(t));
        self.pending = true;
        if self
            .last_sent
            .is_none_or(|at| at.elapsed() >= self.interval)
        {
            self.flush();
        }
    }

    pub fn message(&mut self, message: &str) {
        if let Some(current) = self.current.as_mut() {
            current.message = Some(message.to_string());
            self.pending = true;
        }
    }

    /// Sends the latest state if it has not been sent yet.
    pub fn flush(&mut self) {
        if self.pending
            && let Some(current) = self.current.clone()
        {
            self.send(current);
        }
    }

    fn send(&mut self, progress: JobProgress) {
        self.current = Some(progress.clone());
        self.pending = false;
        self.last_sent = Some(Instant::now());
        (self.sink)(progress);
    }
}

impl Drop for ProgressReporter<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counted(phase: &str, done: u64, total: Option<u64>) -> JobProgress {
        let mut progress = JobProgress::new(phase, "files");
        progress.total = total;
        progress.completed = done;
        progress
    }

    #[test]
    fn a_known_total_gives_a_fraction_and_never_exceeds_it() {
        let progress = JobProgress::new("Scanning", "games")
            .with_total(100)
            .at(250);
        assert_eq!(progress.completed, 100);
        assert_eq!(progress.percent(), Some(100));
        let half = JobProgress::new("Scanning", "games").with_total(200).at(99);
        assert_eq!(half.percent(), Some(49), "rounded down, never up to a lie");
    }

    #[test]
    fn unknown_and_zero_totals_never_produce_a_percentage() {
        assert_eq!(counted("Hashing", 12_831, None).fraction(), None);
        let zero = JobProgress::new("Empty", "files").with_total(0);
        assert_eq!(zero.total, None);
        assert_eq!(zero.percent(), None);
        assert_eq!(counted("x", 5, Some(0)).fraction(), None);
    }

    #[test]
    fn progress_is_monotonic_within_a_phase_and_resets_between_phases() {
        let now = counted("Hashing", 50, Some(100));
        assert!(
            now.advanced_by(&counted("Hashing", 40, Some(100)))
                .is_none()
        );
        assert_eq!(
            now.advanced_by(&counted("Hashing", 60, Some(100)))
                .unwrap()
                .completed,
            60
        );
        let next_phase = now.advanced_by(&counted("Comparing", 0, Some(7))).unwrap();
        assert_eq!(
            (next_phase.phase.as_str(), next_phase.completed),
            ("Comparing", 0)
        );
        // a known total is not forgotten by an update that omits it
        let kept = now.advanced_by(&counted("Hashing", 70, None)).unwrap();
        assert_eq!(kept.total, Some(100));
        // over-reporting is clamped
        assert_eq!(
            now.advanced_by(&counted("Hashing", 900, Some(100)))
                .unwrap()
                .completed,
            100
        );
    }

    #[test]
    fn eta_needs_a_total_enough_work_and_enough_time() {
        let early = counted("x", 1, Some(1000));
        assert_eq!(eta(Duration::from_secs(60), &early), None, "under 5%");
        let quick = counted("x", 500, Some(1000));
        assert_eq!(
            eta(Duration::from_secs(1), &quick),
            None,
            "too little evidence"
        );
        assert_eq!(eta(Duration::from_secs(60), &counted("x", 500, None)), None);
        assert_eq!(
            eta(Duration::from_secs(60), &counted("x", 0, Some(10))),
            None
        );
        assert_eq!(
            eta(Duration::from_secs(60), &counted("x", 10, Some(10))),
            None
        );
        let ok = eta(Duration::from_secs(60), &quick).unwrap();
        assert_eq!(ok.as_secs(), 60);
    }

    #[test]
    fn a_hot_loop_reports_a_bounded_number_of_updates_and_always_the_last() {
        let mut seen = Vec::new();
        {
            let mut sink = |progress: JobProgress| seen.push(progress);
            let mut reporter =
                ProgressReporter::with_interval(&mut sink, Duration::from_secs(3600));
            reporter.phase("Hashing", "files", Some(1_000_000));
            for done in 1..=1_000_000u64 {
                reporter.tick(done);
            }
            reporter.phase("Comparing", "groups", Some(3));
        }
        // the phase start, the flushed final state of phase one, the next phase
        assert!(seen.len() <= 4, "{} updates", seen.len());
        let hashing_final = seen
            .iter()
            .filter(|p| p.phase == "Hashing")
            .next_back()
            .unwrap();
        assert_eq!(hashing_final.completed, 1_000_000);
        assert_eq!(seen.last().unwrap().phase, "Comparing");
    }

    #[test]
    fn progress_messaging_costs_almost_nothing_for_a_large_synthetic_run() {
        let mut updates = 0u64;
        let start = Instant::now();
        {
            let mut sink = |_: JobProgress| updates += 1;
            let mut reporter = ProgressReporter::new(&mut sink);
            reporter.phase("Scanning", "files", Some(5_000_000));
            for done in 1..=5_000_000u64 {
                reporter.tick(done);
            }
        }
        let elapsed = start.elapsed();
        assert!(updates < 100, "{updates} updates for 5M items");
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }
}
