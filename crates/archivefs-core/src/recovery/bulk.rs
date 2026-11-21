//! Preparing several games in one reviewed batch.
//!
//! A batch is planned read-only (each ZIP's index and cue sheet), shown to the
//! person with its total size and the free space, and started by one explicit
//! decision. It then runs one game at a time, never in parallel, through the
//! same [`PreparedMediaStore::prepare`] a single game uses: a game whose copy
//! already exists is linked after a full content comparison, never extracted
//! again.
//!
//! The batch is a queue file in the store (`v1/queue.json`), rewritten after
//! every game. A cancelled, killed or failed run leaves the unfinished games
//! pending; running the queue again continues with them. Nothing is ever
//! added to a queue except the games the person chose.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use super::disc_zip::{DiscSetLimits, inspect_disc_zip};
use super::prepared_media::{
    FREE_SPACE_RESERVE_BYTES, PrepareError, PrepareProgress, PreparedMediaStore,
};
use crate::safe_read::TrustedRoots;

/// Most games in one batch. A larger selection is refused, not truncated.
pub const MAX_BULK_ITEMS: usize = 500;
/// Added to the summed disc sizes when judging free space, on top of the
/// store's fixed reserve: filesystem overhead and the receipts.
const ESTIMATE_MARGIN_PERCENT: u64 = 3;
const QUEUE_FORMAT: u32 = 1;
const QUEUE_NAME: &str = "queue.json";
const MAX_QUEUE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkCandidate {
    pub path: PathBuf,
    pub platform_id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkItemPlan {
    /// Would be extracted: this many new files and bytes.
    Extract { files: usize, bytes: u64 },
    /// A verified copy is already linked to this exact file; nothing to do.
    AlreadyPrepared,
    /// A copy of the same disc exists (`in_store`) or is earlier in this
    /// batch; this file would be read in full and linked, with nothing
    /// extracted. `bytes` is the disc's size: if the copy it relies on is not
    /// there when the batch runs, this game is extracted after all.
    Reuse { bytes: u64, in_store: bool },
    /// Cannot be prepared; why.
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BulkPlanItem {
    pub candidate: BulkCandidate,
    pub plan: BulkItemPlan,
    /// The disc's content fingerprint, when the ZIP could be inspected.
    pub fingerprint: Option<String>,
}

/// The read-only review of a proposed batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BulkPlan {
    pub store: PathBuf,
    pub items: Vec<BulkPlanItem>,
    /// Bytes of new files if every `Extract` item is run.
    pub extract_bytes: u64,
    /// `extract_bytes` plus the margin and the store's fixed reserve.
    pub required_free_bytes: u64,
    pub available_bytes: Option<u64>,
}

impl BulkPlan {
    pub fn count(&self, wanted: impl Fn(&BulkItemPlan) -> bool) -> usize {
        self.items.iter().filter(|item| wanted(&item.plan)).count()
    }

    /// The games that have something to do.
    pub fn actionable(&self) -> impl Iterator<Item = &BulkPlanItem> {
        self.items.iter().filter(|item| {
            matches!(
                item.plan,
                BulkItemPlan::Extract { .. } | BulkItemPlan::Reuse { .. }
            )
        })
    }

    /// Free space needed for just these games (by path), same estimate.
    /// Each disc is counted once. A game that would reuse a copy counts in
    /// full unless that copy is in the store (`in_store` says whether it is
    /// right now) or another chosen game of this batch extracts it.
    pub fn required_with(
        &self,
        chosen: &dyn Fn(&Path) -> bool,
        in_store: &dyn Fn(&str) -> bool,
    ) -> u64 {
        let mut counted = std::collections::BTreeSet::new();
        let mut bytes = 0_u64;
        for item in self
            .items
            .iter()
            .filter(|item| chosen(&item.candidate.path))
        {
            let size = match item.plan {
                BulkItemPlan::Extract { bytes, .. } | BulkItemPlan::Reuse { bytes, .. } => bytes,
                _ => continue,
            };
            let Some(fingerprint) = item.fingerprint.as_deref() else {
                bytes = bytes.saturating_add(size);
                continue;
            };
            if !in_store(fingerprint) && counted.insert(fingerprint) {
                bytes = bytes.saturating_add(size);
            }
        }
        estimate(bytes)
    }

    /// [`Self::required_with`] using what the review saw in the store.
    pub fn required_for(&self, chosen: &dyn Fn(&Path) -> bool) -> u64 {
        let reviewed: std::collections::BTreeSet<&str> = self
            .items
            .iter()
            .filter(|item| matches!(item.plan, BulkItemPlan::Reuse { in_store: true, .. }))
            .filter_map(|item| item.fingerprint.as_deref())
            .collect();
        self.required_with(chosen, &|fingerprint| reviewed.contains(fingerprint))
    }

    pub fn enough_space_for(&self, chosen: &dyn Fn(&Path) -> bool) -> bool {
        self.available_bytes
            .is_some_and(|available| available >= self.required_for(chosen))
    }
}

fn estimate(extract_bytes: u64) -> u64 {
    if extract_bytes == 0 {
        return 0;
    }
    extract_bytes
        .saturating_add(extract_bytes.saturating_mul(ESTIMATE_MARGIN_PERCENT) / 100)
        .saturating_add(FREE_SPACE_RESERVE_BYTES)
}

/// Reviews a proposed batch. Read-only: each ZIP's index and cue sheet.
pub fn plan_bulk(
    store: &PreparedMediaStore,
    candidates: &[BulkCandidate],
    trusted: &TrustedRoots,
    limits: &DiscSetLimits,
    cancel: &AtomicBool,
) -> Result<BulkPlan, String> {
    if candidates.len() > MAX_BULK_ITEMS {
        return Err(format!(
            "{} games were chosen; one batch holds at most {MAX_BULK_ITEMS}. Choose fewer.",
            candidates.len()
        ));
    }
    let mut seen_paths = std::collections::BTreeSet::new();
    let mut planned_sets = std::collections::BTreeSet::new();
    let mut items = Vec::with_capacity(candidates.len());
    let mut extract_bytes = 0_u64;
    for candidate in candidates {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let mut fingerprint = None;
        let plan = if !seen_paths.insert(candidate.path.clone()) {
            BulkItemPlan::Refused("This file is in the list twice.".into())
        } else if !super::diagnose::preparation_may_apply(
            &candidate.path,
            Some(&candidate.platform_id),
        ) {
            BulkItemPlan::Refused("EmuWiz has no preparation for this kind of file.".into())
        } else if store.prepared_for_source(&candidate.path).is_some() {
            BulkItemPlan::AlreadyPrepared
        } else {
            match inspect_disc_zip(&candidate.path, trusted, limits, cancel) {
                Err(refusal) => BulkItemPlan::Refused(sentence(&refusal.to_string())),
                Ok(inspection) => {
                    let in_store = store.lookup(&inspection.fingerprint).is_some();
                    let first = planned_sets.insert(inspection.fingerprint.clone());
                    fingerprint = Some(inspection.fingerprint.clone());
                    if in_store || !first {
                        BulkItemPlan::Reuse {
                            bytes: inspection.total_bytes,
                            in_store,
                        }
                    } else {
                        extract_bytes = extract_bytes.saturating_add(inspection.total_bytes);
                        BulkItemPlan::Extract {
                            files: inspection.members.len(),
                            bytes: inspection.total_bytes,
                        }
                    }
                }
            }
        };
        items.push(BulkPlanItem {
            candidate: candidate.clone(),
            plan,
            fingerprint,
        });
    }
    Ok(BulkPlan {
        store: store.root().to_path_buf(),
        items,
        extract_bytes,
        required_free_bytes: estimate(extract_bytes),
        available_bytes: store.available_bytes().ok(),
    })
}

fn sentence(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect::<String>() + ".",
        None => String::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueState {
    Pending,
    /// Extracted now.
    Prepared,
    /// An existing copy was verified by content and linked.
    Reused,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    pub candidate: BulkCandidate,
    pub state: QueueState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    pub format_version: u32,
    pub created_unix: u64,
    pub items: Vec<QueueItem>,
    /// Why the last run stopped before the end, when it did.
    pub stopped: Option<String>,
}

impl Queue {
    pub fn pending(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.state == QueueState::Pending)
            .count()
    }

    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for item in &self.items {
            *counts
                .entry(match item.state {
                    QueueState::Pending => "pending",
                    QueueState::Prepared => "prepared",
                    QueueState::Reused => "reused",
                    QueueState::Failed(_) => "failed",
                })
                .or_default() += 1;
        }
        counts
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BulkError {
    Store(String),
    /// A queue with unfinished games exists; continue or clear it first.
    QueueInProgress(usize),
    /// Another run holds the queue.
    Busy,
    NothingChosen,
    NotEnoughSpace {
        needed_bytes: u64,
        available_bytes: u64,
    },
    NoQueue,
}

impl std::fmt::Display for BulkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(detail) => write!(f, "the prepared-media folder cannot be used: {detail}"),
            Self::QueueInProgress(pending) => write!(
                f,
                "an earlier batch still has {pending} game(s) to do; continue it or clear it first"
            ),
            Self::Busy => f.write_str("a batch is already running"),
            Self::NothingChosen => f.write_str("no game in the selection has anything to prepare"),
            Self::NotEnoughSpace {
                needed_bytes,
                available_bytes,
            } => write!(
                f,
                "not enough free space for this selection: {needed_bytes} bytes needed, {available_bytes} available. Nothing was started"
            ),
            Self::NoQueue => f.write_str("there is no batch to continue"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BulkProgress<'a> {
    Starting {
        index: usize,
        count: usize,
        title: &'a str,
    },
    Step {
        index: usize,
        count: usize,
        step: PrepareProgress<'a>,
    },
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

struct QueueLock(fs::File);

impl Drop for QueueLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor is open for the lifetime of `self`.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn versioned(store: &PreparedMediaStore) -> PathBuf {
    store.root().join("v1")
}

fn queue_path(store: &PreparedMediaStore) -> PathBuf {
    versioned(store).join(QUEUE_NAME)
}

fn lock_queue(store: &PreparedMediaStore) -> Result<QueueLock, BulkError> {
    use std::os::fd::AsRawFd;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(versioned(store).join("queue.lock"))
        .map_err(|error| BulkError::Store(error.to_string()))?;
    // SAFETY: the descriptor is open; flock has no memory effects.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(BulkError::Busy);
    }
    Ok(QueueLock(file))
}

fn write_queue(store: &PreparedMediaStore, queue: &Queue) -> Result<(), BulkError> {
    let text =
        serde_json::to_string_pretty(queue).map_err(|error| BulkError::Store(error.to_string()))?;
    crate::atomic_write_text(&queue_path(store), &format!("{text}\n"))
        .map_err(|error| BulkError::Store(error.to_string()))
}

/// The saved batch, if there is one. Read-only.
pub fn load_queue(store: &PreparedMediaStore) -> Option<Queue> {
    use std::io::Read;
    let path = queue_path(store);
    let file = crate::safe_read::open_regular_file_nonblocking(&path, false).ok()?;
    let mut text = String::new();
    file.take(MAX_QUEUE_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > MAX_QUEUE_BYTES {
        return None;
    }
    let queue: Queue = serde_json::from_str(&text).ok()?;
    (queue.format_version == QUEUE_FORMAT && queue.items.len() <= MAX_BULK_ITEMS).then_some(queue)
}

/// Saves the chosen games of a reviewed plan as the batch to run. Refuses
/// when an earlier batch is unfinished, when nothing chosen has work to do,
/// and when the chosen games do not fit in the free space (nothing is
/// extracted in that case).
pub fn start_queue(
    store: &PreparedMediaStore,
    plan: &BulkPlan,
    chosen: &dyn Fn(&Path) -> bool,
) -> Result<Queue, BulkError> {
    let items: Vec<QueueItem> = plan
        .actionable()
        .filter(|item| chosen(&item.candidate.path))
        .map(|item| QueueItem {
            candidate: item.candidate.clone(),
            state: QueueState::Pending,
        })
        .collect();
    if items.is_empty() {
        return Err(BulkError::NothingChosen);
    }
    // Judged against the store as it is now, not as it was at the review: a
    // copy that has gone since makes its games count in full. Nothing is
    // created before this check passes.
    let needed = plan.required_with(chosen, &|fingerprint| store.lookup(fingerprint).is_some());
    let available = store.available_bytes().map_err(BulkError::Store)?;
    if available < needed {
        return Err(BulkError::NotEnoughSpace {
            needed_bytes: needed,
            available_bytes: available,
        });
    }
    let store = store.ready().map_err(BulkError::Store)?;
    let _lock = lock_queue(&store)?;
    if let Some(existing) = load_queue(&store)
        && existing.pending() > 0
    {
        return Err(BulkError::QueueInProgress(existing.pending()));
    }
    let queue = Queue {
        format_version: QUEUE_FORMAT,
        created_unix: now_unix(),
        items,
        stopped: None,
    };
    write_queue(&store, &queue)?;
    Ok(queue)
}

/// Forgets the saved batch. Prepared copies are not touched.
pub fn clear_queue(store: &PreparedMediaStore) -> Result<(), BulkError> {
    let Some(store) = store.resolved() else {
        return Ok(());
    };
    let _lock = lock_queue(&store)?;
    match fs::remove_file(queue_path(&store)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BulkError::Store(error.to_string())),
    }
}

/// Runs the saved batch's pending games, one at a time, in order. Leftovers
/// of an interrupted preparation are cleaned first. A game that cannot be
/// prepared is recorded and the run goes on; running out of space, a store
/// problem or cancellation stops the run and leaves the rest pending.
pub fn run_queue(
    store: &PreparedMediaStore,
    remembered_file: Option<&Path>,
    trusted: &TrustedRoots,
    limits: &DiscSetLimits,
    cancel: &AtomicBool,
    progress: &dyn Fn(BulkProgress<'_>),
) -> Result<Queue, BulkError> {
    let store = store.resolved().ok_or(BulkError::NoQueue)?;
    let _lock = lock_queue(&store)?;
    let mut queue = load_queue(&store).ok_or(BulkError::NoQueue)?;
    let _ = store.clean_interrupted();
    queue.stopped = None;
    let count = queue.items.len();
    for index in 0..count {
        if queue.items[index].state != QueueState::Pending {
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            queue.stopped = Some("Cancelled.".into());
            break;
        }
        let candidate = queue.items[index].candidate.clone();
        progress(BulkProgress::Starting {
            index,
            count,
            title: &candidate.title,
        });
        // The queue file is the store's own, but it is still re-checked:
        // only a game the engine has a preparation for is ever prepared.
        if !super::diagnose::preparation_may_apply(&candidate.path, Some(&candidate.platform_id)) {
            queue.items[index].state =
                QueueState::Failed("EmuWiz has no preparation for this kind of file.".into());
            write_queue(&store, &queue)?;
            continue;
        }
        let result = store.prepare(
            &candidate.path,
            &candidate.platform_id,
            trusted,
            limits,
            cancel,
            &|step| progress(BulkProgress::Step { index, count, step }),
        );
        if let Ok(outcome) = &result {
            super::moves::adopt_remembered_choice(remembered_file, outcome, &candidate.path);
        }
        let mut stop = None;
        queue.items[index].state = match result {
            Ok(outcome) if outcome.link_warning.is_some() => QueueState::Failed(format!(
                "The copy is in place but could not be linked to this file ({}).",
                outcome.link_warning.unwrap_or_default()
            )),
            Ok(outcome) if outcome.already_prepared => QueueState::Reused,
            Ok(_) => QueueState::Prepared,
            Err(PrepareError::Cancelled) => {
                stop = Some("Cancelled.".to_string());
                QueueState::Pending
            }
            Err(error @ (PrepareError::NotEnoughSpace { .. } | PrepareError::Store(_))) => {
                stop = Some(sentence(&error.to_string()));
                QueueState::Pending
            }
            Err(PrepareError::Busy) => {
                stop = Some("Another preparation is running.".to_string());
                QueueState::Pending
            }
            Err(error) => QueueState::Failed(sentence(&error.to_string())),
        };
        if let Some(reason) = stop {
            queue.stopped = Some(reason);
            write_queue(&store, &queue)?;
            break;
        }
        write_queue(&store, &queue)?;
        test_hook(index);
    }
    write_queue(&store, &queue)?;
    Ok(queue)
}

#[cfg(test)]
pub(crate) type AfterItem = Box<dyn Fn(usize)>;

#[cfg(test)]
thread_local! {
    /// Test-only: called after each finished game with its index.
    pub(crate) static AFTER_ITEM: std::cell::RefCell<Option<AfterItem>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn test_hook(index: usize) {
    AFTER_ITEM.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook(index);
        }
    });
}

#[cfg(not(test))]
#[inline(always)]
fn test_hook(_index: usize) {}
