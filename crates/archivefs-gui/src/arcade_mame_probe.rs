//! Blue-side wiring for per-set MAME routing of extracted Arcade set folders.
//!
//! The probe (two short MAME start-ups) never runs on the UI thread. Rendering
//! asks [`lookup`], which answers from an in-memory cache or starts ONE
//! background probe per key and answers [`Lookup::Checking`] until it
//! finishes; a registered repaint hook wakes the UI when a result arrives.
//! Background-readiness workers (already off the UI thread) use
//! [`with_blocking`] to wait for the same cached answer instead.
//!
//! Safety rails: a global cap on concurrent MAME processes, cancellation when
//! the selection moves on, failures/timeouts are never cached as answers
//! (retried with back-off), and nothing is persisted or written.
//!
//! Cache freshness (documented limits): a result is keyed on the executable,
//! the set folder and the extra rompaths, and is re-validated at most every
//! [`REVALIDATE_EVERY`] against a fingerprint of the set folder's mtime, the
//! newest mtime among its immediate entries, the collection (parent) folder's
//! mtime, each extra rompath's mtime and the mtimes of the dependency folders
//! MAME named. Edits deep inside a dependency folder that change none of these
//! are not seen until the user rechecks.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use archivefs_core::arcade_mame_routing::{
    ArcadeBlockReason, ArcadeMameVerdict, ArcadeReadiness, is_probe_safe_set_name,
    probe_arcade_set, read_extra_rompath_file, validate_extra_rompath,
};
use archivefs_core::dat::dependency::{DependencyState, SetDependencyReport};
use archivefs_core::dat::set::{SetIdentity, SetResolution, SetState};
use archivefs_core::launch::{
    LaunchBlocker, LaunchBlockerKind, LaunchPlan, LaunchReadiness, LaunchTarget, LaunchWarning,
    LaunchWarningKind,
};

/// File (read only, optional) listing extra read-only MAME ROM search paths,
/// one absolute folder per line. Absent means none. This app never writes it.
pub(crate) const EXTRA_ROMPATHS_FILE: &str = "mame_extra_rompaths.txt";
const PROBE_TIMEOUT: Duration = Duration::from_secs(25);
/// Most MAME processes at once, across the whole app.
const MAX_CONCURRENT: usize = 2;
const REVALIDATE_EVERY: Duration = Duration::from_secs(2);
const BACKOFF_BASE: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub(crate) struct ArcadeSetProbe {
    pub(crate) set: String,
    pub(crate) resolution: SetResolution,
    pub(crate) verdict: ArcadeMameVerdict,
    pub(crate) extra_rompaths: Vec<PathBuf>,
    /// Lines of the extra-rompath list that were ignored, with the reason.
    pub(crate) extra_diagnostics: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Lookup {
    /// A background probe is running; ask again after the repaint.
    Checking,
    Done(Arc<ArcadeSetProbe>),
}

type Id = (PathBuf, PathBuf, Vec<PathBuf>);

#[derive(Clone, PartialEq, Eq)]
struct Fingerprint {
    set_dir: Option<SystemTime>,
    newest_entry: Option<SystemTime>,
    parent: Option<SystemTime>,
    extras: Vec<Option<SystemTime>>,
    deps: Vec<(PathBuf, Option<SystemTime>)>,
}

struct Entry {
    fingerprint: Fingerprint,
    validated: Instant,
    probe: Arc<ArcadeSetProbe>,
}

struct Failure {
    at: Instant,
    attempts: u32,
    probe: Arc<ArcadeSetProbe>,
}

struct Inflight {
    cancel: Arc<AtomicBool>,
    foreground: bool,
}

#[derive(Default)]
struct State {
    done: HashMap<Id, Entry>,
    failed: HashMap<Id, Failure>,
    inflight: HashMap<Id, Inflight>,
    running: usize,
}

struct Manager {
    state: Mutex<State>,
    changed: Condvar,
    repaint: Mutex<Option<eframe::egui::Context>>,
}

fn manager() -> &'static Manager {
    static MANAGER: OnceLock<Manager> = OnceLock::new();
    MANAGER.get_or_init(|| Manager {
        state: Mutex::new(State::default()),
        changed: Condvar::new(),
        repaint: Mutex::new(None),
    })
}

/// Lets a finished probe wake the UI. Safe to call every frame.
pub(crate) fn register_repaint(context: &eframe::egui::Context) {
    if let Ok(mut slot) = manager().repaint.lock()
        && slot.is_none()
    {
        *slot = Some(context.clone());
    }
}

fn wake_ui() {
    if let Ok(slot) = manager().repaint.lock()
        && let Some(context) = slot.as_ref()
    {
        // A probe has finished: its answer is collected by the next build.
        crate::emulator_setup::answers::arrived(context);
        context.request_repaint();
    }
}

thread_local! {
    /// Set only inside [`with_blocking`]: the cancel flag of a worker thread
    /// that may wait for a probe.
    static BLOCKING: Cell<Option<*const AtomicBool>> = const { Cell::new(None) };
}

/// Runs `f` with probes waited for (not started in the background). ONLY for
/// threads that are not the UI thread.
pub(crate) fn with_blocking<R>(cancel: &AtomicBool, f: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            BLOCKING.with(|b| b.set(None));
        }
    }
    BLOCKING.with(|b| b.set(Some(cancel as *const AtomicBool)));
    let _reset = Reset;
    f()
}

/// The validated extra rompaths and a diagnostic per ignored line.
pub(crate) fn configured_extra_rompaths() -> (Vec<PathBuf>, Vec<String>) {
    let Ok(file) = archivefs_core::app_dirs::config_path(EXTRA_ROMPATHS_FILE) else {
        return (Vec::new(), Vec::new());
    };
    split_extra_rompaths(&read_extra_rompath_file(&file))
}

pub(crate) fn split_extra_rompaths(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let mut good = Vec::new();
    let mut diagnostics = Vec::new();
    for path in paths {
        match validate_extra_rompath(path) {
            Ok(canonical) if !good.contains(&canonical) => good.push(canonical),
            Ok(_) => {}
            Err(error) => diagnostics.push(format!("ignored extra ROM path: {error}")),
        }
    }
    (good, diagnostics)
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn fingerprint(set_dir: &Path, extras: &[PathBuf], dep_names: &[String]) -> Fingerprint {
    let newest_entry = std::fs::read_dir(set_dir).ok().and_then(|entries| {
        entries
            .flatten()
            .filter_map(|e| e.metadata().ok()?.modified().ok())
            .max()
    });
    let parent = set_dir.parent();
    Fingerprint {
        set_dir: mtime(set_dir),
        newest_entry,
        parent: parent.and_then(mtime),
        extras: extras.iter().map(|p| mtime(p)).collect(),
        deps: dep_names
            .iter()
            .filter_map(|name| {
                let path = parent?.join(name);
                Some((path.clone(), mtime(&path)))
            })
            .collect(),
    }
}

/// Whether `set_dir` is a plain, safely named set folder this route handles.
fn plain_set(set_dir: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(set_dir).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    let set = set_dir.file_name()?.to_str()?.to_string();
    is_probe_safe_set_name(&set).then_some(set)
}

/// Non-blocking (on the UI thread) answer for `set_dir`. `None` when the path
/// is not a plain set folder.
pub(crate) fn lookup(executable: &Path, set_dir: &Path) -> Option<Lookup> {
    let (extras, diagnostics) = configured_extra_rompaths();
    lookup_with(executable, set_dir, &extras, &diagnostics)
}

/// As [`lookup`] but NEVER starts a probe: only reports an answer or a probe
/// that the catalogue-gated readiness build already started. Click-time code
/// uses this so it cannot probe an uncatalogued folder.
pub(crate) fn peek(executable: &Path, set_dir: &Path) -> Option<Lookup> {
    let (extras, diagnostics) = configured_extra_rompaths();
    lookup_inner(executable, set_dir, &extras, &diagnostics, false)
}

pub(crate) fn lookup_with(
    executable: &Path,
    set_dir: &Path,
    extras: &[PathBuf],
    diagnostics: &[String],
) -> Option<Lookup> {
    lookup_inner(executable, set_dir, extras, diagnostics, true)
}

fn lookup_inner(
    executable: &Path,
    set_dir: &Path,
    extras: &[PathBuf],
    diagnostics: &[String],
    may_start: bool,
) -> Option<Lookup> {
    let set = plain_set(set_dir)?;
    let id: Id = (
        executable.to_path_buf(),
        set_dir.to_path_buf(),
        extras.to_vec(),
    );
    let blocking = BLOCKING.with(Cell::get);
    loop {
        let mut state = manager().state.lock().ok()?;
        if let Some(entry) = state.done.get_mut(&id) {
            if entry.validated.elapsed() < REVALIDATE_EVERY {
                return Some(Lookup::Done(entry.probe.clone()));
            }
            let now = fingerprint(set_dir, extras, &entry.probe_deps());
            if now == entry.fingerprint {
                entry.validated = Instant::now();
                return Some(Lookup::Done(entry.probe.clone()));
            }
            state.done.remove(&id);
        }
        if let Some(failure) = state.failed.get(&id) {
            let wait = (BACKOFF_BASE * 2u32.saturating_pow(failure.attempts.saturating_sub(1)))
                .min(BACKOFF_MAX);
            if failure.at.elapsed() < wait {
                return Some(Lookup::Done(failure.probe.clone()));
            }
        }
        if state.inflight.contains_key(&id) {
            match blocking {
                None => return Some(Lookup::Checking),
                Some(cancel) => {
                    // SAFETY: the pointer is live for the whole `with_blocking` scope.
                    if unsafe { &*cancel }.load(Ordering::Relaxed) {
                        return Some(Lookup::Checking);
                    }
                    let _ = manager()
                        .changed
                        .wait_timeout(state, Duration::from_millis(100));
                    continue;
                }
            }
        }
        if !may_start {
            return None;
        }
        // Nothing cached or running: start it.
        let cancel = Arc::new(AtomicBool::new(false));
        state.inflight.insert(
            id.clone(),
            Inflight {
                cancel: cancel.clone(),
                foreground: blocking.is_none(),
            },
        );
        drop(state);
        let job = Job {
            id,
            set,
            executable: executable.to_path_buf(),
            diagnostics: diagnostics.to_vec(),
        };
        match blocking {
            None => {
                std::thread::spawn(move || {
                    run_job(&job, &cancel);
                    wake_ui();
                });
                return Some(Lookup::Checking);
            }
            Some(outer) => {
                // SAFETY: as above.
                let outer = unsafe { &*outer };
                let flag = AtomicBool::new(false);
                let result = std::thread::scope(|scope| {
                    let watcher = scope.spawn(|| {
                        while !flag.load(Ordering::Relaxed) {
                            if outer.load(Ordering::Relaxed) {
                                cancel.store(true, Ordering::Relaxed);
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    });
                    let result = run_job(&job, &cancel);
                    flag.store(true, Ordering::Relaxed);
                    let _ = watcher.join();
                    result
                });
                return Some(result.map_or(Lookup::Checking, Lookup::Done));
            }
        }
    }
}

impl Entry {
    fn probe_deps(&self) -> Vec<String> {
        self.fingerprint
            .deps
            .iter()
            .filter_map(|(path, _)| path.file_name()?.to_str().map(str::to_string))
            .collect()
    }
}

struct Job {
    id: Id,
    set: String,
    executable: PathBuf,
    diagnostics: Vec<String>,
}

fn acquire_slot(cancel: &AtomicBool) -> bool {
    let manager = manager();
    let Ok(mut state) = manager.state.lock() else {
        return false;
    };
    while state.running >= MAX_CONCURRENT {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        state = match manager
            .changed
            .wait_timeout(state, Duration::from_millis(100))
        {
            Ok((guard, _)) => guard,
            Err(_) => return false,
        };
    }
    state.running += 1;
    true
}

/// Runs one probe and records the outcome. `None` when cancelled.
fn run_job(job: &Job, cancel: &AtomicBool) -> Option<Arc<ArcadeSetProbe>> {
    let (_, set_dir, extras) = (&job.id.0, &job.id.1, &job.id.2);
    // A cancelled probe leaves no trace in the caches.
    let finish = |_cancelled: bool| {
        if let Ok(mut state) = manager().state.lock() {
            state.inflight.remove(&job.id);
        }
        manager().changed.notify_all();
    };
    if !acquire_slot(cancel) {
        finish(true);
        return None;
    }
    // Taken before the probe so a change during it is seen next time.
    let before = fingerprint(set_dir, extras, &[]);
    let probe = probe_arcade_set(
        &job.executable,
        &job.set,
        set_dir,
        extras,
        PROBE_TIMEOUT,
        cancel,
    );
    if let Ok(mut state) = manager().state.lock() {
        state.running = state.running.saturating_sub(1);
    }
    if cancel.load(Ordering::Relaxed) {
        finish(true);
        return None;
    }
    let failed = probe.verdict.block_reason == Some(ArcadeBlockReason::CheckFailed);
    let blocked = probe.verdict.readiness == ArcadeReadiness::Blocked;
    let deps = {
        use archivefs_core::arcade_mame_routing::VerifyProblemKind;
        probe
            .verify
            .problems
            .iter()
            .filter_map(|p| match &p.kind {
                VerifyProblemKind::NotFound {
                    dependency: Some(d),
                } => Some(d.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut fp = before;
    fp.deps = fingerprint(set_dir, extras, &deps).deps;
    let result = Arc::new(ArcadeSetProbe {
        resolution: SetResolution {
            identity: SetIdentity {
                source_id: "mame-installed".to_string(),
                game_name: job.set.clone(),
            },
            archive_path: set_dir.clone(),
            state: if blocked {
                SetState::Incomplete
            } else {
                SetState::Complete
            },
            members_required: Vec::new(),
            members_verified: Vec::new(),
            members_bad: Vec::new(),
            members_optional: Vec::new(),
            members_borrowed: Vec::new(),
            disks_required: Vec::new(),
            disks_identity_matched: Vec::new(),
            disks_parent_required: Vec::new(),
            dependencies: SetDependencyReport {
                state: DependencyState::NotApplicable,
                requirements: Vec::new(),
            },
        },
        set: job.set.clone(),
        verdict: probe.verdict,
        extra_rompaths: extras.clone(),
        extra_diagnostics: job.diagnostics.clone(),
    });
    if let Ok(mut state) = manager().state.lock() {
        state.inflight.remove(&job.id);
        if failed {
            let attempts = state.failed.get(&job.id).map_or(0, |f| f.attempts) + 1;
            state.failed.insert(
                job.id.clone(),
                Failure {
                    at: Instant::now(),
                    attempts,
                    probe: result.clone(),
                },
            );
        } else {
            state.failed.remove(&job.id);
            if state.done.len() > 512 {
                state.done.clear();
            }
            state.done.insert(
                job.id.clone(),
                Entry {
                    fingerprint: fp,
                    validated: Instant::now(),
                    probe: result.clone(),
                },
            );
        }
    }
    manager().changed.notify_all();
    Some(result)
}

/// Forget every cached answer (and back-off) for `set_dir`, for any
/// executable or extra paths: the explicit "Recheck readiness" path.
pub(crate) fn invalidate_path(set_dir: &Path) {
    if let Ok(mut state) = manager().state.lock() {
        state.done.retain(|id, _| id.1 != set_dir);
        state.failed.retain(|id, _| id.1 != set_dir);
    }
}

/// Stops foreground probes for any folder other than `keep`.
pub(crate) fn cancel_foreground_except(keep: Option<&Path>) {
    if let Ok(state) = manager().state.lock() {
        for (id, flight) in state.inflight.iter() {
            if flight.foreground && Some(id.1.as_path()) != keep {
                flight.cancel.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// The launch blocker for a Blocked verdict, with MAME's exact reason.
pub(crate) fn blocker_for(probe: &ArcadeSetProbe) -> LaunchBlocker {
    let verdict = &probe.verdict;
    let kind = match verdict.block_reason {
        Some(ArcadeBlockReason::RomsIncomplete | ArcadeBlockReason::RomsWrongVersion) => {
            LaunchBlockerKind::MameSetIncomplete
        }
        Some(ArcadeBlockReason::BiosOrDeviceMissing) => LaunchBlockerKind::MameDependencyBlocked,
        _ => LaunchBlockerKind::MameSetVerdictUnavailable,
    };
    let mut detail = verdict.blockers.join("; ");
    for line in &probe.extra_diagnostics {
        detail.push_str(&format!(" ({line})"));
    }
    LaunchBlocker::new(kind, detail)
}

pub(crate) fn checking_blocker() -> LaunchBlocker {
    LaunchBlocker::new(
        LaunchBlockerKind::MameSetVerdictUnavailable,
        "Checking this machine with MAME…",
    )
}

/// Adds the verdict's named warnings (and extra-rompath diagnostics) to the
/// MAME candidate (never removes or adds a blocker) and refreshes readiness.
pub(crate) fn apply_warnings(plan: &mut LaunchPlan, probe: &ArcadeSetProbe) {
    let lines: Vec<&String> = probe
        .verdict
        .warnings
        .iter()
        .chain(probe.extra_diagnostics.iter())
        .collect();
    if lines.is_empty() {
        return;
    }
    for candidate in &mut plan.candidates {
        if !matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "mame",
                ..
            }
        ) || candidate.readiness == LaunchReadiness::Blocked
        {
            continue;
        }
        for line in &lines {
            candidate.warnings.push(LaunchWarning::new(
                LaunchWarningKind::MameMachineFlagged,
                (*line).clone(),
            ));
        }
        candidate.readiness = LaunchReadiness::ReadyWithWarnings;
    }
    let count = |r: LaunchReadiness| plan.candidates.iter().filter(|c| c.readiness == r).count();
    plan.summary.ready = count(LaunchReadiness::Ready);
    plan.summary.ready_with_warnings = count(LaunchReadiness::ReadyWithWarnings);
    plan.summary.blocked = count(LaunchReadiness::Blocked);
}

#[cfg(test)]
mod tests;
