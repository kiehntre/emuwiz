//! A bounded background check whose result a page can read without waiting.
//!
//! Reading a ROM on a sleeping or disconnected drive can block for a long
//! time. The drawing thread therefore never performs the check: it asks for a
//! [`Snapshot`], which returns at once, and a detached worker does the reads.
//!
//! Rules:
//! - Work happens only while something asks (no polling loop of its own).
//! - One worker answers a configuration generation at a time; a silent one
//!   is retried occasionally, up to `max_workers` for that generation.
//!   Workers still stalled on an earlier generation are counted separately
//!   and capped at twice that, so threads never exceed `3 * max_workers`.
//! - A result belongs to the generation it was started for. After
//!   [`AsyncResolver::invalidate`] an older worker's late result is dropped.
//! - A result is shown as current only for `ttl + stall` after it was made.
//!   Past that the answer is `Checking`, or `Unavailable` once the worker has
//!   been silent for `stall`: stale readiness is never shown as current.
//! - Nothing ever joins a worker, so leaving a page or quitting never waits
//!   for stalled I/O.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Snapshot<T> {
    /// A check is running and nothing recent enough is known.
    Checking,
    /// The check has not answered for too long (or cannot be started).
    Unavailable,
    Current(T),
}

struct Inner<T> {
    generation: u64,
    /// The newest completed result, with the generation it answered and
    /// when it was completed.
    latest: Option<(u64, Instant, T)>,
    /// When the check that produced `latest` started.
    latest_started: Option<Instant>,
    /// The worker answering the current question, and when it started.
    running: Option<(u64, Instant)>,
    /// Live workers answering the current generation.
    workers: usize,
    /// Live workers still answering an earlier generation (a stalled read of
    /// a drive the user has since moved away from). Counted separately so
    /// they can never use up the current configuration's slots.
    superseded: usize,
}

/// Returns a worker's slot when it ends, however it ends (a panic included).
struct Slot<T> {
    shared: Arc<Mutex<Inner<T>>>,
    generation: u64,
    started: Instant,
}

impl<T> Drop for Slot<T> {
    fn drop(&mut self) {
        let mut inner = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if inner.generation == self.generation {
            inner.workers = inner.workers.saturating_sub(1);
        } else {
            inner.superseded = inner.superseded.saturating_sub(1);
        }
        if inner.running == Some((self.generation, self.started)) {
            inner.running = None;
        }
    }
}

pub(crate) struct AsyncResolver<T> {
    inner: Arc<Mutex<Inner<T>>>,
    ttl: Duration,
    stall: Duration,
    retry: Duration,
    max_workers: usize,
}

impl<T: Clone + Send + 'static> AsyncResolver<T> {
    pub(crate) fn new(ttl: Duration, stall: Duration, retry: Duration, max_workers: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                generation: 0,
                latest: None,
                latest_started: None,
                running: None,
                workers: 0,
                superseded: 0,
            })),
            ttl,
            stall,
            retry,
            max_workers,
        }
    }

    /// The configuration changed: every earlier result, finished or still on
    /// its way, stops being an answer to the current question.
    pub(crate) fn invalidate(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.generation += 1;
        // Workers of the old configuration no longer hold its slots.
        inner.superseded += inner.workers;
        inner.workers = 0;
        inner.running = None;
    }

    /// The configuration generation, for callers that cache derived results.
    pub(crate) fn generation(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .generation
    }

    /// The last completed result of any generation, for callers that only
    /// need to know what kind of thing was there before.
    pub(crate) fn previous(&self) -> Option<T> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.latest.as_ref().map(|(_, _, value)| value.clone())
    }

    /// Never blocks. Starts `resolve` on a worker when a fresh answer is
    /// needed and none is on its way.
    pub(crate) fn snapshot(&self, resolve: impl FnOnce() -> T + Send + 'static) -> Snapshot<T> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let generation = inner.generation;
        let current = inner
            .latest
            .as_ref()
            .filter(|(answered, _, _)| *answered == generation)
            .map(|(_, at, value)| (at.elapsed(), value.clone()));
        if let Some((age, value)) = &current
            && *age < self.ttl
        {
            return Snapshot::Current(value.clone());
        }
        let running = inner
            .running
            .filter(|(answering, _)| *answering == generation)
            .map(|(_, started)| started.elapsed());
        let start = match running {
            None => true,
            // A silent worker is retried occasionally, within the worker cap.
            Some(silent) => silent >= self.retry,
        };
        // Superseded workers are bounded too, so a drive that never answers
        // cannot accumulate threads across many configuration changes.
        if start && inner.workers < self.max_workers && inner.superseded < self.max_workers * 2 {
            let started = Instant::now();
            inner.workers += 1;
            inner.running = Some((generation, started));
            let slot = Slot {
                shared: Arc::clone(&self.inner),
                generation,
                started,
            };
            // The lock is released while the thread is created: if creation
            // fails, the closure (and its `Slot`) is dropped right here, and
            // the slot hands itself back, which needs the lock.
            drop(inner);
            let _ = std::thread::Builder::new()
                .name("atari800-setup-check".into())
                .spawn(move || {
                    let value = resolve();
                    let mut inner = slot.shared.lock().unwrap_or_else(|e| e.into_inner());
                    // Only an answer to the current question, and never one
                    // that started before the answer already held.
                    if inner.generation == generation
                        && inner.latest_started.is_none_or(|held| held <= started)
                    {
                        inner.latest = Some((generation, Instant::now(), value));
                        inner.latest_started = Some(started);
                    }
                    drop(inner);
                    drop(slot);
                });
            inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        }
        let silent = inner
            .running
            .filter(|(answering, _)| *answering == generation)
            .map(|(_, started)| started.elapsed());
        match (current, silent) {
            (Some((age, value)), _) if age < self.ttl + self.stall => Snapshot::Current(value),
            (_, Some(silent)) if silent < self.stall => Snapshot::Checking,
            _ => Snapshot::Unavailable,
        }
    }

    /// Every live worker, superseded ones included.
    #[cfg(test)]
    fn live(&self) -> usize {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.workers + inner.superseded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    const TTL: Duration = Duration::from_millis(40);
    const STALL: Duration = Duration::from_millis(80);
    /// Far longer than any test: no retry worker unless a test asks for one.
    const NO_RETRY: Duration = Duration::from_secs(600);
    /// "Did not wait": generous for a loaded machine, far below any real stall.
    const PROMPT: Duration = Duration::from_secs(10);

    fn resolver() -> AsyncResolver<&'static str> {
        AsyncResolver::new(TTL, STALL, NO_RETRY, 3)
    }

    fn until<T: Clone + Send + 'static + PartialEq + std::fmt::Debug>(
        resolver: &AsyncResolver<T>,
        resolve: impl Fn() -> T + Send + Clone + 'static,
        wanted: impl Fn(&Snapshot<T>) -> bool,
    ) -> Snapshot<T> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let snapshot = resolver.snapshot(resolve.clone());
            if wanted(&snapshot) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "last state: {snapshot:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn idle<T: Clone + Send + 'static>(resolver: &AsyncResolver<T>) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while resolver.live() != 0 {
            assert!(Instant::now() < deadline, "workers never finished");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A read that does not return (a sleeping USB drive) until released.
    /// `entered` reports each worker that has actually started reading.
    fn blocked() -> (
        mpsc::Sender<()>,
        mpsc::Receiver<()>,
        impl Fn() -> &'static str + Send + Clone + 'static,
    ) {
        let (release, wait) = mpsc::channel::<()>();
        let (entered, entries) = mpsc::channel::<()>();
        let wait = Arc::new(Mutex::new(wait));
        (release, entries, move || {
            let _ = entered.send(());
            let _ = wait.lock().unwrap_or_else(|e| e.into_inner()).recv();
            "late"
        })
    }

    #[test]
    fn a_blocked_read_never_blocks_the_caller_and_is_reported_truthfully() {
        let resolver = resolver();
        let (release, _entries, read) = blocked();
        let started = Instant::now();
        assert_eq!(resolver.snapshot(read.clone()), Snapshot::Checking);
        assert!(started.elapsed() < PROMPT, "must not wait for the read");
        // Silent for longer than `stall`: no longer "checking", never "ready".
        let state = until(&resolver, read.clone(), |s| *s == Snapshot::Unavailable);
        assert_eq!(state, Snapshot::Unavailable);
        drop(release);
    }

    #[test]
    fn repeated_refreshes_use_one_worker() {
        let resolver = resolver();
        let (release, entries, read) = blocked();
        for _ in 0..500 {
            let _ = resolver.snapshot(read.clone());
        }
        assert_eq!(resolver.live(), 1, "one worker per question");
        entries.recv().unwrap();
        assert!(entries.try_recv().is_err(), "the read ran once");
        drop(release);
    }

    #[test]
    fn a_drive_that_never_answers_cannot_exceed_the_worker_cap() {
        let resolver = AsyncResolver::new(TTL, STALL, Duration::from_millis(10), 3);
        let calls = Arc::new(AtomicUsize::new(0));
        let (release, _entries, read) = blocked();
        let counted = {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                read()
            }
        };
        // Many retry intervals pass while every read stays blocked.
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            let _ = resolver.snapshot(counted.clone());
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(resolver.live() <= 3, "{}", resolver.live());
        assert!(calls.load(Ordering::SeqCst) <= 3, "bounded thread creation");
        drop(release);
    }

    #[test]
    fn a_result_is_current_then_refreshed_and_never_shown_once_stale() {
        let resolver = resolver();
        let state = until(&resolver, || "ready", |s| matches!(s, Snapshot::Current(_)));
        assert_eq!(state, Snapshot::Current("ready"));
        idle(&resolver);
        // The drive disappears: the refresh blocks. The old answer may be
        // shown only inside the bounded window, then it is withdrawn.
        let (release, _entries, read) = blocked();
        std::thread::sleep(TTL);
        let state = until(&resolver, read.clone(), |s| {
            !matches!(s, Snapshot::Current(_))
        });
        assert_ne!(state, Snapshot::Current("ready"));
        let state = until(&resolver, read, |s| *s == Snapshot::Unavailable);
        assert_eq!(state, Snapshot::Unavailable);
        drop(release);
    }

    #[test]
    fn a_configuration_change_discards_the_old_answer_and_any_late_result() {
        let resolver = resolver();
        until(&resolver, || "old", |s| matches!(s, Snapshot::Current(_)));
        idle(&resolver);
        // A check for the old configuration is really on its way...
        std::thread::sleep(TTL);
        let (release, entries, read) = blocked();
        let _ = resolver.snapshot(read);
        entries.recv().unwrap();
        // ...when the configuration changes.
        resolver.invalidate();
        assert_ne!(
            resolver.snapshot(|| "new"),
            Snapshot::Current("old"),
            "an answer to the old configuration is never current"
        );
        let state = until(&resolver, || "new", |s| matches!(s, Snapshot::Current(_)));
        assert_eq!(state, Snapshot::Current("new"));
        // The superseded worker finally answers; its result is ignored.
        drop(release);
        idle(&resolver);
        assert_eq!(resolver.previous(), Some("new"));
        assert_ne!(resolver.snapshot(|| "new"), Snapshot::Current("late"));
    }

    /// Reads stalled on a drive the user has since replaced must not use up
    /// the new configuration's slots.
    #[test]
    fn stalled_checks_of_an_old_configuration_do_not_starve_the_new_one() {
        let resolver = AsyncResolver::new(TTL, STALL, Duration::from_millis(10), 2);
        let (release, entries, read) = blocked();
        let deadline = Instant::now() + Duration::from_secs(20);
        while resolver.live() < 2 {
            let _ = resolver.snapshot(read.clone());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        entries.recv().unwrap();
        // Both slots are held by reads that never return.
        until(&resolver, read.clone(), |s| *s == Snapshot::Unavailable);
        assert_eq!(resolver.live(), 2);
        // The user points the setup somewhere healthy.
        resolver.invalidate();
        let state = until(
            &resolver,
            || "healthy",
            |s| matches!(s, Snapshot::Current(_)),
        );
        assert_eq!(state, Snapshot::Current("healthy"));
        drop(release);
        idle(&resolver);
    }

    #[test]
    fn a_check_that_panics_gives_its_slot_back() {
        let resolver: AsyncResolver<&'static str> = AsyncResolver::new(TTL, STALL, NO_RETRY, 1);
        let _ = resolver.snapshot(|| panic!("simulated failure inside a check"));
        idle(&resolver);
        // The single slot is free again, so the next check can run.
        let state = until(&resolver, || "ok", |s| matches!(s, Snapshot::Current(_)));
        assert_eq!(state, Snapshot::Current("ok"));
    }

    #[test]
    fn leaving_the_page_never_waits_for_a_stalled_worker() {
        let resolver = resolver();
        let (release, entries, read) = blocked();
        let _ = resolver.snapshot(read);
        entries.recv().unwrap();
        let started = Instant::now();
        drop(resolver);
        assert!(started.elapsed() < PROMPT);
        drop(release);
    }
}
