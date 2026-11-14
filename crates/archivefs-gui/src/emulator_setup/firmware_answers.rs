//! Firmware answers for the launch planner, kept between readiness builds.
//!
//! A PCSX2 profile's BIOS answer and a Flycast profile's Dreamcast BIOS
//! answer never depend on the selected game, but each one reads and hashes a
//! BIOS file, and the readiness input is built on the interface thread.
//!
//! The same readiness input is built for two purposes, and they are served
//! differently (see [`FirmwareUse`]):
//!
//! - **Anything that can lead to a launch** (Play, the Launch page, the bezel
//!   preview: they read the planner's firmware verdict to decide whether to
//!   ask for a launch at all), the legacy pages and the background readiness
//!   workers ask for [`FirmwareUse::Fresh`] answers. They get exactly what
//!   they got before this module watched files: an answer computed for the
//!   same profiles and evidence less than [`FIRMWARE_ANSWER_RECHECK`] ago,
//!   or else one computed there and then.
//! - **The drawn readiness card** asks for [`FirmwareUse::Shown`] answers.
//!   Those are reused for as long as the files they were read from are the
//!   same files: same device, inode, size, modification time and change
//!   time, and for a folder the same for every entry directly inside it. A
//!   file changed less than [`UNSETTLED_FILE_AGE`] ago is not trusted to stay
//!   the same yet. When a file or the firmware evidence has changed, on
//!   Recheck, and after [`FIRMWARE_ANSWER_BACKSTOP`] in any case, the answers
//!   are verified again on a thread of their own; the card shows the answers
//!   already held until the new ones arrive, and the interface is woken if
//!   they differ. They are computed where they are asked for only when there
//!   is nothing to show: the first time, and when the list of profiles
//!   itself has changed.
//!
//! A held answer that is out of date can therefore be drawn for a moment,
//! but it is never what a launch is decided on. Each launch also verifies
//! the firmware again in the core launch preflight, which does not use this
//! module.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;

/// What the answers are wanted for. See the module text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FirmwareUse {
    /// For the drawn readiness card only: answers already held may be shown
    /// while they are verified again on another thread.
    Shown,
    /// For everything else, including whatever can lead to a launch: never
    /// older than [`FIRMWARE_ANSWER_RECHECK`], computed here if need be.
    Fresh,
}

/// How long an answer counts as fresh for [`FirmwareUse::Fresh`], and how
/// often [`FirmwareUse::Shown`] answers are verified again when the files
/// they were read from cannot be relied on to show a change (see
/// [`MAX_WATCHED_ENTRIES`], [`UNSETTLED_FILE_AGE`]). This was the reuse
/// window of every answer before the files were watched.
pub(crate) const FIRMWARE_ANSWER_RECHECK: Duration = Duration::from_secs(2);

/// Shown answers whose files have not changed are still verified again this
/// often, in case a change left no trace in what is watched: a configured
/// BIOS name that is not a plain file name in the watched folder, or a file
/// system that does not keep inodes and timestamps reliably.
pub(crate) const FIRMWARE_ANSWER_BACKSTOP: Duration = Duration::from_secs(60);

/// A watched folder with more entries than this is not watched entry by
/// entry; its answers fall back to [`FIRMWARE_ANSWER_RECHECK`].
const MAX_WATCHED_ENTRIES: usize = 256;

/// A file whose times are this recent can be written again without them
/// changing: the kernel stamps files with a clock that moves in steps of a
/// few milliseconds, and some file systems keep whole seconds. Until a file
/// is older than this, its answers fall back to [`FIRMWARE_ANSWER_RECHECK`].
const UNSETTLED_FILE_AGE: Duration = Duration::from_secs(2);

/// What identifies one file's current content without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileIdentity {
    /// Whether the file was modified or changed after `moment`.
    fn newer_than(&self, moment: SystemTime) -> bool {
        let moment = moment.duration_since(UNIX_EPOCH).map_or((0, 0), |since| {
            (since.as_secs() as i64, i64::from(since.subsec_nanos()))
        });
        self.modified > moment || self.changed > moment
    }

    fn of(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            size: metadata.size(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

/// One watched path as it is now: the path itself (a link is not followed),
/// what it leads to, and, for a folder, each entry directly inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Watched {
    path: PathBuf,
    itself: Option<FileIdentity>,
    target: Option<FileIdentity>,
    entries: Option<Vec<(OsString, Option<FileIdentity>)>>,
}

/// The current state of every watched path, or `None` when it cannot be
/// relied on to show the next change: a folder is too large to watch, or
/// something was changed less than [`UNSETTLED_FILE_AGE`] before `wall_now`.
/// A path that does not exist is a state like any other, so a file that
/// appears later is noticed.
fn stamp(paths: &[PathBuf], wall_now: SystemTime) -> Option<Vec<Watched>> {
    let settled_before = wall_now.checked_sub(UNSETTLED_FILE_AGE)?;
    let unsettled = |identity: &Option<FileIdentity>| {
        identity.is_some_and(|identity| identity.newer_than(settled_before))
    };
    let mut stamps = Vec::with_capacity(paths.len());
    for path in paths {
        let itself = std::fs::symlink_metadata(path).ok();
        let target = std::fs::metadata(path).ok();
        let mut entries = None;
        if target.as_ref().is_some_and(std::fs::Metadata::is_dir)
            && let Ok(listing) = std::fs::read_dir(path)
        {
            let mut found = Vec::new();
            for entry in listing {
                let Ok(entry) = entry else {
                    return None;
                };
                if found.len() == MAX_WATCHED_ENTRIES {
                    return None;
                }
                let identity = std::fs::symlink_metadata(entry.path())
                    .ok()
                    .map(|metadata| FileIdentity::of(&metadata));
                found.push((entry.file_name(), identity));
            }
            found.sort_by(|left, right| left.0.cmp(&right.0));
            entries = Some(found);
        }
        let watched = Watched {
            path: path.clone(),
            itself: itself.as_ref().map(FileIdentity::of),
            target: target.as_ref().map(FileIdentity::of),
            entries,
        };
        if unsettled(&watched.itself)
            || unsettled(&watched.target)
            || watched
                .entries
                .iter()
                .flatten()
                .any(|(_, identity)| unsettled(identity))
        {
            return None;
        }
        stamps.push(watched);
    }
    Some(stamps)
}

fn hash_of(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// What a set of answers is computed from: a list of profiles, or a list of
/// profiles together with the firmware evidence.
pub(crate) trait FirmwareInputs {
    fn profile_count(&self) -> usize;
    /// Two keys: the first changes when the list of profiles changes, the
    /// second when anything the answers are computed from changes.
    fn keys(&self) -> (u64, u64);
}

impl<P: std::fmt::Debug> FirmwareInputs for Vec<P> {
    fn profile_count(&self) -> usize {
        self.len()
    }
    fn keys(&self) -> (u64, u64) {
        let profiles = hash_of(&format!("{self:?}"));
        (profiles, profiles)
    }
}

impl<P: std::fmt::Debug, E: std::fmt::Debug> FirmwareInputs for (&Vec<P>, &Vec<E>) {
    fn profile_count(&self) -> usize {
        self.0.len()
    }
    fn keys(&self) -> (u64, u64) {
        let profiles = hash_of(&format!("{:?}", self.0));
        let evidence = hash_of(&format!("{:?}", self.1));
        (profiles, hash_of(&format!("{profiles}:{evidence}")))
    }
}

struct Entry<T> {
    key: u64,
    profiles: u64,
    answers: Vec<T>,
    /// The watched files as they were just before the answers were computed.
    stamp: Option<Vec<Watched>>,
    verified: Instant,
    /// Recheck was pressed: verify again whatever the files look like.
    due: bool,
}

struct Shared<T> {
    entry: Option<Entry<T>>,
    /// The verification that is running on its own thread: its number, and
    /// the key it was started for.
    refreshing: Option<(u64, u64)>,
    /// The number of the last verification started.
    started: u64,
    /// Woken when a verification delivers answers that differ.
    wake: Option<egui::Context>,
    /// Answers computed so far, wherever they were computed.
    computed: u64,
    /// The time of day, for judging how recently a file was changed.
    wall_clock: fn() -> SystemTime,
}

/// Per-profile firmware answers for the launch planner. See the module text.
pub(crate) struct FirmwareAnswerCache<T> {
    shared: Arc<Mutex<Shared<T>>>,
}

impl<T> Default for FirmwareAnswerCache<T> {
    fn default() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                entry: None,
                refreshing: None,
                started: 0,
                wake: None,
                computed: 0,
                wall_clock: SystemTime::now,
            })),
        }
    }
}

fn lock<T>(shared: &Mutex<Shared<T>>) -> MutexGuard<'_, Shared<T>> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A verification running on its own thread. If the thread ends without
/// delivering (the work panicked), its mark is cleared so that the next
/// request starts another instead of waiting for it for ever.
struct InFlight<T> {
    cache: Arc<Mutex<Shared<T>>>,
    mark: (u64, u64),
    delivered: bool,
}

impl<T> Drop for InFlight<T> {
    fn drop(&mut self) {
        if !self.delivered {
            let mut shared = lock(&self.cache);
            if shared.refreshing == Some(self.mark) {
                shared.refreshing = None;
            }
        }
    }
}

impl<T: Clone + PartialEq + Send + 'static> FirmwareAnswerCache<T> {
    /// The interface to wake when answers verified on their own thread
    /// arrive. Called on every frame; only the first call does anything.
    pub(crate) fn wake_with(&self, context: &egui::Context) {
        let mut shared = lock(&self.shared);
        if shared.wake.is_none() {
            shared.wake = Some(context.clone());
        }
    }

    /// Has the answers verified again the next time they are asked for, even
    /// if no file looks changed. A verification already running was started
    /// before this, so its answers are not taken. For Recheck.
    pub(crate) fn verify_again(&self) {
        let mut shared = lock(&self.shared);
        shared.refreshing = None;
        if let Some(entry) = shared.entry.as_mut() {
            entry.due = true;
        }
    }

    /// One answer per profile of `inputs`, as fresh as `usage` requires.
    ///
    /// `watched(index)` names the files and folders that profile's answer is
    /// read from. `job()` is called only when answers have to be computed and
    /// returns the work, owning everything it needs, so that it can run on
    /// another thread: `work(index)` is that profile's answer.
    pub(crate) fn answers<I, J>(
        &self,
        usage: FirmwareUse,
        inputs: &I,
        now: Instant,
        watched: impl Fn(usize) -> Vec<PathBuf>,
        job: impl FnOnce() -> J,
    ) -> Vec<T>
    where
        I: FirmwareInputs,
        J: Fn(usize) -> T + Send + 'static,
    {
        let count = inputs.profile_count();
        if count == 0 {
            return Vec::new();
        }
        let (profiles, key) = inputs.keys();
        // Always taken before anything is computed: a file that changes while
        // it is being read then differs from this, and is verified again.
        let stamp_now = |wall_clock: fn() -> SystemTime| {
            let paths: Vec<PathBuf> = (0..count).flat_map(&watched).collect();
            stamp(&paths, wall_clock())
        };
        // Computed on this thread and kept. A verification still running was
        // started for an earlier state: its answers are not taken.
        let compute_here = |stamp: Option<Vec<Watched>>, work: J| {
            let answers: Vec<T> = (0..count).map(&work).collect();
            let mut shared = lock(&self.shared);
            shared.computed += count as u64;
            shared.refreshing = None;
            shared.entry = Some(Entry {
                key,
                profiles,
                answers: answers.clone(),
                stamp,
                verified: now,
                due: false,
            });
            answers
        };

        let shared = lock(&self.shared);
        let wall_clock = shared.wall_clock;
        let held = shared
            .entry
            .as_ref()
            .filter(|entry| entry.profiles == profiles && entry.answers.len() == count);

        if usage == FirmwareUse::Fresh {
            // The rule from before files were watched, unchanged.
            if let Some(entry) = held
                && entry.key == key
                && !entry.due
                && now.saturating_duration_since(entry.verified) < FIRMWARE_ANSWER_RECHECK
            {
                return entry.answers.clone();
            }
            drop(shared);
            return compute_here(stamp_now(wall_clock), job());
        }

        let Some(entry) = held else {
            // Nothing to show: compute here, once.
            drop(shared);
            return compute_here(stamp_now(wall_clock), job());
        };
        let answers = entry.answers.clone();
        let (entry_key, due, verified) = (entry.key, entry.due, entry.verified);
        let then = entry.stamp.clone();
        drop(shared);

        let seen = stamp_now(wall_clock);
        let age = now.saturating_duration_since(verified);
        let current = entry_key == key
            && !due
            && match (&then, &seen) {
                (Some(then), Some(seen)) => then == seen && age < FIRMWARE_ANSWER_BACKSTOP,
                _ => age < FIRMWARE_ANSWER_RECHECK,
            };
        if current {
            return answers;
        }

        let mut shared = lock(&self.shared);
        if shared.refreshing.map(|(_, running_for)| running_for) == Some(key) {
            return answers; // already being verified
        }
        shared.started += 1;
        let mark = (shared.started, key);
        shared.refreshing = Some(mark);
        drop(shared);

        let work = job();
        let cache = Arc::clone(&self.shared);
        let verify = move || {
            let mut in_flight = InFlight {
                cache,
                mark,
                delivered: false,
            };
            let answers: Vec<T> = (0..count).map(&work).collect();
            let cache = Arc::clone(&in_flight.cache);
            let mut shared = lock(&cache);
            in_flight.delivered = true;
            if shared.refreshing != Some(mark) {
                return; // overtaken: something newer was asked for meanwhile
            }
            shared.refreshing = None;
            shared.computed += count as u64;
            let differ = shared
                .entry
                .as_ref()
                .is_none_or(|held| held.profiles != profiles || held.answers != answers);
            shared.entry = Some(Entry {
                key,
                profiles,
                answers,
                stamp: seen,
                verified: now,
                due: false,
            });
            let wake = differ.then(|| shared.wake.clone()).flatten();
            drop(shared);
            if let Some(context) = wake {
                super::answers::arrived(&context);
                context.request_repaint();
            }
        };
        let started = std::thread::Builder::new()
            .name("firmware-answers".into())
            .spawn(verify);
        if started.is_err() {
            // No thread to be had: the next request tries again.
            let mut shared = lock(&self.shared);
            if shared.refreshing == Some(mark) {
                shared.refreshing = None;
            }
        }
        answers
    }
}

#[cfg(test)]
impl<T> FirmwareAnswerCache<T> {
    /// A cache for which every fixture file was last changed long ago, as
    /// real BIOS files were: the time of day it uses is an hour ahead.
    pub(crate) fn with_settled_files() -> Self {
        let cache = Self::default();
        lock(&cache.shared).wall_clock = || SystemTime::now() + Duration::from_secs(3600);
        cache
    }

    /// Answers computed so far.
    pub(crate) fn computed(&self) -> u64 {
        lock(&self.shared).computed
    }

    /// A cache for which every fixture file was changed a moment ago: the
    /// time of day it uses is an hour behind.
    pub(crate) fn with_unsettled_files() -> Self {
        let cache = Self::default();
        lock(&cache.shared).wall_clock = || SystemTime::now() - Duration::from_secs(3600);
        cache
    }

    /// Verifications started on their own thread so far.
    pub(crate) fn refreshes(&self) -> u64 {
        lock(&self.shared).started
    }

    /// Waits for a verification running on its own thread to deliver.
    pub(crate) fn settle(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while lock(&self.shared).refreshing.is_some() {
            assert!(
                Instant::now() < deadline,
                "the verification never delivered"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[cfg(test)]
mod tests;
