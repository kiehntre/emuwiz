//! Durable serial execution for reviewed conversion plans.
//!
//! `open` owns the queue until dropped and recovers abandoned running jobs.
//! `inspect` is strictly read-only. Publication/rollback remain Repair jobs;
//! this file persists scheduling, original plans, attempts and their receipts.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::ArchiveFsError;
use crate::dat::rename_apply::identity::{capture_identity, identity_matches};
use crate::dat::rename_apply::model::{EntryState, ObjectIdentity, TransactionState};
use crate::dat::sources::now_unix;
use crate::repair::execute::RepairExecutionOptions;
use crate::safe_read::TrustedRoots;
use crate::wiiu_conversion::{self, WiiUConversionError, WiiUConversionPlan, WiiUConversionRecord};

type Result<T> = std::result::Result<T, ArchiveFsError>;
pub const QUEUE_SCHEMA_VERSION: u32 = 1;
pub const MAX_QUEUE_JOBS: usize = 256;
pub const MAX_JOB_ATTEMPTS: usize = 32;
pub const MAX_QUEUE_BYTES: u64 = 8 * 1024 * 1024;
const SNAPSHOT_FILE: &str = "queue.json";

/// Uses `Completed`, as existing conversion/history models do, for success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    BlockedStale,
    BlockedInputMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryDisposition {
    /// Revalidate the ORIGINAL plan, retain old staging, start at byte zero.
    RestartFromBeginning,
    /// Publication may have happened. Inspect the referenced Repair journals.
    RequiresReview,
}

/// A typed, original reviewed plan, including its private evidence binding.
/// Add proven converters here; preview-only formats are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "conversion", content = "plan", rename_all = "snake_case")]
pub enum ReviewedConversion {
    WuxToWud(Box<WiiUConversionPlan>),
    WudToWux(Box<WiiUConversionPlan>),
}

impl ReviewedConversion {
    pub fn source(&self) -> &Path {
        match self {
            Self::WuxToWud(plan) | Self::WudToWux(plan) => &plan.source,
        }
    }

    pub fn destination(&self) -> &Path {
        match self {
            Self::WuxToWud(plan) | Self::WudToWux(plan) => &plan.destination,
        }
    }

    fn revalidate(&self) -> std::result::Result<(), WiiUConversionError> {
        use wiiu_conversion::WiiUConversionDirection;
        let (plan, direction) = match self {
            Self::WuxToWud(plan) => (plan, WiiUConversionDirection::WuxToWud),
            Self::WudToWux(plan) => (plan, WiiUConversionDirection::WudToWux),
        };
        if plan.direction != direction {
            return Err(WiiUConversionError::StalePlan);
        }
        wiiu_conversion::revalidate(plan, true).map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressSnapshot {
    pub bytes_processed: Option<u64>,
    pub total_bytes: Option<u64>,
    pub phase: String,
    pub updated_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionAttempt {
    /// Every attempt has its own Repair journal directory, including failures.
    pub journal_dir: PathBuf,
    /// Destination-adjacent directory; abandoned output is retained, not reused.
    pub staging_root: PathBuf,
    pub started_at_unix: u64,
    /// Previous failed/interrupted attempts remain visible after a retry.
    pub state: JobState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionJob {
    pub id: u64,
    /// FIFO admission sequence. Retrying goes behind already queued work.
    pub order: u64,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    pub reviewed: ReviewedConversion,
    pub source_identity: ObjectIdentity,
    pub state: JobState,
    pub attempts: Vec<ConversionAttempt>,
    pub progress: Option<ProgressSnapshot>,
    pub result: Option<WiiUConversionRecord>,
    pub retry: Option<RetryDisposition>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueSnapshot {
    pub schema_version: u32,
    pub next_sequence: u64,
    pub jobs: Vec<ConversionJob>,
}

impl Default for QueueSnapshot {
    fn default() -> Self {
        Self {
            schema_version: QUEUE_SCHEMA_VERSION,
            next_sequence: 1,
            jobs: vec![],
        }
    }
}

pub fn default_queue_directory() -> Result<PathBuf> {
    Ok(crate::app_dirs::data_dir()?.join("conversion-queue"))
}

/// One owner/worker per queue directory, enforced across processes by the OS.
/// The lock is released on exit/crash; no PID or stale lock-file guessing.
/// Inspection does not acquire this lock and can run while conversion runs.
#[derive(Debug)]
pub struct DurableConversionQueue {
    root: PathBuf,
    lock: File,
    snapshot: QueueSnapshot,
    poisoned: bool,
}

impl Drop for DurableConversionQueue {
    fn drop(&mut self) {
        // A concurrent fork can inherit the open descriptor until exec. Closing
        // our copy alone would leave its lock held by that unrelated child.
        let _ = self.lock.unlock();
    }
}

impl DurableConversionQueue {
    pub fn inspect(root: &Path) -> Result<QueueSnapshot> {
        if !safe_directory(root, false)? {
            return Ok(QueueSnapshot::default());
        }
        let path = root.join(SNAPSHOT_FILE);
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if fs::read_dir(root)
                    .map_err(|e| ArchiveFsError::io(root, e))?
                    .next()
                    .is_some()
                {
                    return Err(problem(
                        "queue snapshot missing but recovery files remain; preserved for review",
                    ));
                }
                return Ok(QueueSnapshot::default());
            }
            Err(e) => return Err(ArchiveFsError::io(path, e)),
            Ok(m) if !m.is_file() || m.file_type().is_symlink() || m.len() > MAX_QUEUE_BYTES => {
                return Err(problem(format!(
                    "unsafe or oversized queue: {}",
                    path.display()
                )));
            }
            Ok(_) => {}
        }
        let file = crate::safe_read::open_bounded_read(&path, &TrustedRoots::none())
            .map_err(|e| problem(format!("cannot read {}: {e:?}", path.display())))?
            .into_file();
        let mut bytes = Vec::new();
        file.take(MAX_QUEUE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| ArchiveFsError::io(path.clone(), e))?;
        if bytes.len() as u64 > MAX_QUEUE_BYTES {
            return Err(problem("queue exceeded read bound"));
        }
        let snapshot = serde_json::from_slice(&bytes).map_err(|e| {
            problem(format!(
                "corrupt queue {}; preserved for review: {e}",
                path.display()
            ))
        })?;
        validate_snapshot(root, &snapshot)?;
        Ok(snapshot)
    }

    /// Explicit startup recovery. Never executes, deletes staging, or publishes.
    pub fn open(root: &Path) -> Result<Self> {
        safe_directory(root, true)?;
        let lock = File::open(root).map_err(|e| ArchiveFsError::io(root, e))?;
        lock.try_lock().map_err(|e| {
            problem(format!(
                "queue is busy or cannot lock {}: {e}",
                root.display()
            ))
        })?;
        let snapshot = Self::inspect(root)?;
        let mut queue = Self {
            root: root.into(),
            lock,
            snapshot,
            poisoned: false,
        };
        let mut recovered = queue.snapshot.clone();
        let mut changed = false;
        for job in &mut recovered.jobs {
            if job.state == JobState::Running {
                job.state = JobState::Interrupted;
                if let Some(attempt) = job.attempts.last_mut() {
                    attempt.state = JobState::Interrupted;
                }
                job.retry = Some(retry_disposition(job));
                job.updated_at_unix = now_unix();
                job.diagnostic = Some("Previous worker exited; retained staging and journals. Retry starts from byte zero after revalidation.".into());
                if let Some(progress) = &mut job.progress {
                    progress.phase = "interrupted (historical progress only)".into();
                }
                changed = true;
            }
        }
        if changed || !root.join(SNAPSHOT_FILE).exists() {
            queue.commit(recovered)?;
        }
        Ok(queue)
    }

    pub fn snapshot(&self) -> &QueueSnapshot {
        &self.snapshot
    }

    /// Enqueue only a reviewed, currently valid typed plan. A full source
    /// identity supplements the converter's bounded preview evidence.
    pub fn enqueue(&mut self, reviewed: ReviewedConversion) -> Result<u64> {
        self.ensure_writable()?;
        if self.snapshot.jobs.len() >= MAX_QUEUE_JOBS {
            return Err(problem("queue is full; explicitly prune completed jobs"));
        }
        self.check_collision(reviewed.destination(), None)?;
        reviewed
            .revalidate()
            .map_err(|e| problem(format!("plan refused: {e}")))?;
        let source_identity = capture_identity(reviewed.source())
            .map_err(|e| ArchiveFsError::io(reviewed.source(), e))?;
        reviewed
            .revalidate()
            .map_err(|e| problem(format!("plan changed during admission: {e}")))?;
        let mut next = self.snapshot.clone();
        let id = take_sequence(&mut next)?;
        next.jobs.push(ConversionJob {
            id,
            order: id,
            created_at_unix: now_unix(),
            updated_at_unix: now_unix(),
            reviewed,
            source_identity,
            state: JobState::Queued,
            attempts: vec![],
            progress: None,
            result: None,
            retry: None,
            diagnostic: None,
        });
        self.commit(next)?;
        Ok(id)
    }

    pub fn cancel_queued(&mut self, id: u64) -> Result<()> {
        let index = self.index(id)?;
        if self.snapshot.jobs[index].state != JobState::Queued {
            return Err(problem("only queued jobs can be cancelled by ID"));
        }
        self.transition(
            index,
            JobState::Cancelled,
            "Cancelled before execution",
            None,
        )
    }

    /// Requeues the same job/plan at the FIFO tail; never replaces evidence.
    pub fn retry(&mut self, id: u64) -> Result<JobState> {
        let index = self.index(id)?;
        let job = &self.snapshot.jobs[index];
        if !matches!(
            job.state,
            JobState::Failed
                | JobState::Interrupted
                | JobState::BlockedStale
                | JobState::BlockedInputMissing
        ) {
            return Err(problem("job is not retryable"));
        }
        if retry_disposition(job) == RetryDisposition::RequiresReview {
            return Err(problem(
                "publication/staging evidence requires review; original attempts retained",
            ));
        }
        if let Some((state, reason)) = self.revalidation_failure(index) {
            self.transition(
                index,
                state,
                &reason,
                Some(RetryDisposition::RestartFromBeginning),
            )?;
            return Ok(state);
        }
        if job.attempts.len() >= MAX_JOB_ATTEMPTS {
            return Err(problem("attempt limit reached; recovery evidence retained"));
        }
        let mut next = self.snapshot.clone();
        let order = take_sequence(&mut next)?;
        let job = &mut next.jobs[index];
        job.order = order;
        job.state = JobState::Queued;
        job.updated_at_unix = now_unix();
        job.progress = None;
        job.retry = None;
        job.diagnostic = None;
        self.commit(next)?;
        Ok(JobState::Queued)
    }

    /// Runs at most one FIFO job synchronously. Pass an AtomicBool shared with
    /// the caller for cooperative cancellation. Encode checks each 32 KiB,
    /// decode each 64 KiB;
    /// hashing/verification stops at the next converter cancellation boundary.
    pub fn run_next(
        &mut self,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&ProgressSnapshot),
    ) -> Result<Option<ConversionJob>> {
        self.ensure_writable()?;
        if self
            .snapshot
            .jobs
            .iter()
            .any(|j| j.state == JobState::Running)
        {
            return Err(problem(
                "unfinished worker; close and reopen the queue for recovery",
            ));
        }
        let Some(index) = self
            .snapshot
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| j.state == JobState::Queued)
            .min_by_key(|(_, j)| j.order)
            .map(|(i, _)| i)
        else {
            return Ok(None);
        };
        if cancel.load(Ordering::Relaxed) {
            self.transition(
                index,
                JobState::Cancelled,
                "Cancelled before execution",
                None,
            )?;
            return Ok(Some(self.snapshot.jobs[index].clone()));
        }
        if let Some((state, reason)) = self.revalidation_failure(index) {
            self.transition(
                index,
                state,
                &reason,
                Some(RetryDisposition::RestartFromBeginning),
            )?;
            return Ok(Some(self.snapshot.jobs[index].clone()));
        }
        if !self.snapshot.jobs[index].attempts.is_empty()
            && retry_disposition(&self.snapshot.jobs[index]) == RetryDisposition::RequiresReview
        {
            self.transition(
                index,
                JobState::Failed,
                "Previous publication evidence changed after retry; review required",
                Some(RetryDisposition::RequiresReview),
            )?;
            return Ok(Some(self.snapshot.jobs[index].clone()));
        }
        let mut next = self.snapshot.clone();
        let job = &mut next.jobs[index];
        if job.attempts.len() >= MAX_JOB_ATTEMPTS {
            return Err(problem("attempt limit reached"));
        }
        let parent = job
            .reviewed
            .destination()
            .parent()
            .ok_or_else(|| problem("destination has no parent"))?;
        let journal_dir = self
            .root
            .join("transactions")
            .join(job.id.to_string())
            .join((job.attempts.len() + 1).to_string());
        let preparation = (|| {
            let staging = tempfile::Builder::new()
                .prefix(".emuwiz-conversion-queue-")
                .tempdir_in(parent)
                .map_err(|e| ArchiveFsError::io(parent, e))?;
            safe_directory(&journal_dir, true)?;
            if fs::read_dir(&journal_dir)
                .map_err(|e| ArchiveFsError::io(&journal_dir, e))?
                .next()
                .is_some()
            {
                return Err(problem(format!(
                    "attempt journal directory is occupied: {}",
                    journal_dir.display()
                )));
            }
            File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|e| ArchiveFsError::io(parent, e))?;
            Ok(staging)
        })();
        let staging = match preparation {
            Ok(staging) => staging,
            Err(error) => {
                self.transition(
                    index,
                    JobState::Failed,
                    &error.to_string(),
                    Some(RetryDisposition::RequiresReview),
                )?;
                return Ok(Some(self.snapshot.jobs[index].clone()));
            }
        };
        job.attempts.push(ConversionAttempt {
            journal_dir: journal_dir.clone(),
            staging_root: staging.path().into(),
            started_at_unix: now_unix(),
            state: JobState::Running,
        });
        job.state = JobState::Running;
        job.updated_at_unix = now_unix();
        job.progress = Some(ProgressSnapshot {
            bytes_processed: None,
            total_bytes: None,
            phase: "validating source".into(),
            updated_at_unix: now_unix(),
        });
        self.commit(next)?; // Must precede any converter output or publication.
        let staging_root = staging.keep();
        let reviewed = self.snapshot.jobs[index].reviewed.clone();
        let options = RepairExecutionOptions {
            trusted: TrustedRoots::none(),
            journal_dir,
            audit_cache: crate::dat::sources::audit_cache::AuditCacheConfig::Disabled,
        };
        let mut last_checkpoint = None::<Instant>;
        let mut checkpoint_error = None;
        let outcome = match &reviewed {
            ReviewedConversion::WuxToWud(plan) | ReviewedConversion::WudToWux(plan) => {
                wiiu_conversion::execute_wiiu_conversion_in_stage(
                    plan,
                    &options,
                    cancel,
                    &mut |p| {
                        let snapshot = ProgressSnapshot {
                            bytes_processed: Some(p.written_bytes),
                            total_bytes: Some(p.expected_bytes),
                            phase: if p.written_bytes == p.expected_bytes {
                                "verifying output"
                            } else {
                                "converting"
                            }
                            .into(),
                            updated_at_unix: now_unix(),
                        };
                        // At most one durable progress checkpoint per second. The
                        // callback remains live between checkpoints; no per-chunk fsync.
                        if checkpoint_error.is_none()
                            && last_checkpoint.is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
                        {
                            let mut next = self.snapshot.clone();
                            next.jobs[index].progress = Some(snapshot.clone());
                            if let Err(e) = self.commit(next) {
                                checkpoint_error = Some(e);
                                cancel.store(true, Ordering::Relaxed);
                            }
                            last_checkpoint = Some(Instant::now());
                        }
                        progress(&snapshot);
                    },
                    Some(&staging_root),
                )
            }
        };
        if let Some(e) = checkpoint_error {
            return Err(e);
        }
        match outcome {
            Ok((record, _transaction)) => {
                let mut next = self.snapshot.clone();
                let job = &mut next.jobs[index];
                job.state = JobState::Completed;
                job.attempts.last_mut().unwrap().state = JobState::Completed;
                job.updated_at_unix = now_unix();
                job.progress = Some(ProgressSnapshot {
                    bytes_processed: Some(record.output_bytes),
                    total_bytes: Some(record.output_bytes),
                    phase: "completed (verified)".into(),
                    updated_at_unix: now_unix(),
                });
                job.result = Some(record);
                job.retry = None;
                self.commit(next)?;
            }
            Err(error) => {
                let disposition = retry_disposition(&self.snapshot.jobs[index]);
                let state = match &error {
                    WiiUConversionError::Cancelled => JobState::Cancelled,
                    WiiUConversionError::StalePlan => JobState::BlockedStale,
                    WiiUConversionError::Refused(_) => JobState::BlockedStale,
                    WiiUConversionError::Transaction { .. }
                        if cancel.load(Ordering::Relaxed)
                            && disposition == RetryDisposition::RestartFromBeginning =>
                    {
                        JobState::Cancelled
                    }
                    _ => JobState::Failed,
                };
                self.transition(index, state, &error.to_string(), Some(disposition))?;
            }
        }
        Ok(Some(self.snapshot.jobs[index].clone()))
    }

    /// Explicit retention: removes only completed/cancelled queue rows with no
    /// failed/interrupted attempts. Repair journals and staging are NEVER pruned.
    /// Failed, blocked and interrupted recovery evidence always consumes capacity.
    pub fn prune_completed(&mut self, retain: usize) -> Result<usize> {
        self.ensure_writable()?;
        let mut next = self.snapshot.clone();
        let mut eligible: Vec<_> = next
            .jobs
            .iter()
            .filter(|j| {
                j.attempts
                    .iter()
                    .all(|a| matches!(a.state, JobState::Completed | JobState::Cancelled))
                    && (j.state == JobState::Completed
                        || (j.state == JobState::Cancelled
                            && j.retry != Some(RetryDisposition::RequiresReview)))
            })
            .map(|j| (j.updated_at_unix, j.id))
            .collect();
        eligible.sort_unstable();
        let remove: BTreeSet<_> = eligible
            .iter()
            .take(eligible.len().saturating_sub(retain))
            .map(|(_, id)| *id)
            .collect();
        next.jobs.retain(|j| !remove.contains(&j.id));
        if !remove.is_empty() {
            self.commit(next)?;
        }
        Ok(remove.len())
    }

    fn revalidation_failure(&self, index: usize) -> Option<(JobState, String)> {
        let job = &self.snapshot.jobs[index];
        let fail = |e: String| Some((JobState::BlockedStale, e));
        match fs::symlink_metadata(job.reviewed.source()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Some((
                    JobState::BlockedInputMissing,
                    "Reviewed source is missing".into(),
                ));
            }
            Err(e) => return fail(e.to_string()),
            Ok(_) => {}
        }
        if let Err(e) = self.check_collision(job.reviewed.destination(), Some(job.id)) {
            return fail(e.to_string());
        }
        if let Err(e) = job.reviewed.revalidate() {
            return fail(e.to_string());
        }
        match capture_identity(job.reviewed.source()) {
            Ok(current) if identity_matches(&job.source_identity, &current) => None,
            Ok(_) => fail("Source identity differs from the original reviewed job".into()),
            Err(e) => fail(e.to_string()),
        }
    }

    fn check_collision(&self, destination: &Path, except: Option<u64>) -> Result<()> {
        if self.snapshot.jobs.iter().any(|j| {
            Some(j.id) != except
                && !matches!(j.state, JobState::Completed | JobState::Cancelled)
                && (j.reviewed.destination().starts_with(destination)
                    || destination.starts_with(j.reviewed.destination()))
        }) {
            return Err(problem(
                "destination is reserved by another queued/recovery job",
            ));
        }
        match fs::symlink_metadata(destination) {
            Ok(_) => Err(problem(
                "destination already exists; no overwrite permitted",
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ArchiveFsError::io(destination, e)),
        }
    }

    fn index(&self, id: u64) -> Result<usize> {
        self.ensure_writable()?;
        self.snapshot
            .jobs
            .iter()
            .position(|j| j.id == id)
            .ok_or_else(|| problem("unknown conversion job"))
    }

    fn transition(
        &mut self,
        index: usize,
        state: JobState,
        diagnostic: &str,
        retry: Option<RetryDisposition>,
    ) -> Result<()> {
        let mut next = self.snapshot.clone();
        let job = &mut next.jobs[index];
        job.state = state;
        if let Some(attempt) = job
            .attempts
            .last_mut()
            .filter(|a| a.state == JobState::Running)
        {
            attempt.state = state;
        }
        job.updated_at_unix = now_unix();
        job.retry = retry;
        job.diagnostic = Some(diagnostic.chars().take(2048).collect());
        self.commit(next)
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.poisoned {
            Err(problem(
                "queue persistence failed; close and reopen for recovery",
            ))
        } else {
            if !safe_directory(&self.root, false)? {
                return Err(problem("queue directory disappeared"));
            }
            let current =
                fs::metadata(&self.root).map_err(|e| ArchiveFsError::io(self.root.clone(), e))?;
            let locked = self
                .lock
                .metadata()
                .map_err(|e| ArchiveFsError::io(self.root.clone(), e))?;
            if crate::filesystem_identity(&current) != crate::filesystem_identity(&locked) {
                return Err(problem(
                    "queue directory was replaced; refusing writes without its lock",
                ));
            }
            Ok(())
        }
    }

    fn commit(&mut self, next: QueueSnapshot) -> Result<()> {
        self.ensure_writable()?;
        validate_snapshot(&self.root, &next)?;
        // ponytail: rewrite one bounded snapshot (256 jobs); split records only
        // if measured checkpoint cost warrants the extra recovery protocol.
        let body = serde_json::to_string(&next).map_err(|e| problem(e.to_string()))?;
        if body.len() as u64 > MAX_QUEUE_BYTES {
            return Err(problem("queue exceeds persistence bound"));
        }
        let result = (|| {
            // Detect corruption or external replacement while this owner was
            // open instead of overwriting the only recovery evidence.
            if Self::inspect(&self.root)? != self.snapshot {
                return Err(problem(
                    "queue changed outside its owner; preserved for review",
                ));
            }
            crate::atomic_write_text(&self.root.join(SNAPSHOT_FILE), &body)?;
            // Queue durability requires a supported directory fsync, rather
            // than relying solely on the helper's best-effort directory sync.
            self.lock
                .sync_all()
                .map_err(|e| ArchiveFsError::io(self.root.clone(), e))
        })();
        if let Err(e) = result {
            self.poisoned = true;
            return Err(e);
        }
        self.snapshot = next;
        Ok(())
    }
}

fn problem(message: impl Into<String>) -> ArchiveFsError {
    ArchiveFsError::Config(message.into())
}

fn take_sequence(snapshot: &mut QueueSnapshot) -> Result<u64> {
    let id = snapshot.next_sequence;
    snapshot.next_sequence = id
        .checked_add(1)
        .ok_or_else(|| problem("queue sequence exhausted"))?;
    Ok(id)
}

/// Retain all abandoned staging. With no publication (or a confirmed rollback)
/// and no output, restarting is safe; old staged bytes are never trusted/reused.
fn retry_disposition(job: &ConversionJob) -> RetryDisposition {
    if fs::symlink_metadata(job.reviewed.destination()).is_ok()
        || job.attempts.iter().any(|a| {
            !matches!(safe_directory(&a.staging_root, false), Ok(true))
                || !matches!(safe_directory(&a.journal_dir, false), Ok(true))
                || !publication_settled_without_output(&a.journal_dir)
        })
    {
        RetryDisposition::RequiresReview
    } else {
        RetryDisposition::RestartFromBeginning
    }
}

// Each integrated conversion publishes through exactly one Repair transaction.
// An explicitly completed rollback permits retry; unknown files, corrupt data,
// pending transactions and applied outputs remain human-review cases.
fn publication_settled_without_output(directory: &Path) -> bool {
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    let entries: Vec<_> = entries.take(2).collect();
    if entries.is_empty() {
        return true;
    }
    if entries.len() != 1 {
        return false;
    }
    let Ok(entry) = &entries[0] else {
        return false;
    };
    let path = entry.path();
    if path.extension().is_none_or(|e| e != "json")
        || !fs::symlink_metadata(&path)
            .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink() && m.len() <= MAX_QUEUE_BYTES)
    {
        return false;
    }
    let transaction = (|| {
        let file = crate::safe_read::open_bounded_read(&path, &TrustedRoots::none())
            .ok()?
            .into_file();
        let mut bytes = Vec::new();
        file.take(MAX_QUEUE_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_QUEUE_BYTES {
            return None;
        }
        serde_json::from_slice::<crate::dat::rename_apply::model::RenameTransaction>(&bytes).ok()
    })();
    let Some(transaction) = transaction else {
        return false;
    };
    transaction.state == TransactionState::RolledBack
        && transaction.entries.iter().all(|e| {
            matches!(
                e.state,
                EntryState::RolledBack
                    | EntryState::Skipped
                    | EntryState::Planned
                    | EntryState::PreflightPassed
                    | EntryState::ApplyFailed
            )
        })
}

fn safe_directory(path: &Path, create: bool) -> Result<bool> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(problem("queue paths must be absolute without traversal"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(false);
                }
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        safe_directory(&current, false)?;
                    }
                    Err(e) => return Err(ArchiveFsError::io(current, e)),
                }
                if let Some(parent) = current.parent() {
                    File::open(parent)
                        .and_then(|f| f.sync_all())
                        .map_err(|e| ArchiveFsError::io(parent, e))?;
                }
            }
            Ok(_) => return Err(problem(format!("unsafe directory: {}", current.display()))),
            Err(e) => return Err(ArchiveFsError::io(current, e)),
        }
    }
    Ok(true)
}

fn validate_snapshot(root: &Path, snapshot: &QueueSnapshot) -> Result<()> {
    if snapshot.schema_version != QUEUE_SCHEMA_VERSION
        || snapshot.jobs.len() > MAX_QUEUE_JOBS
        || snapshot.next_sequence == 0
        || snapshot
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Running)
            .count()
            > 1
    {
        return Err(problem(
            "unsupported or invalid queue schema/capacity; state preserved",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut orders = BTreeSet::new();
    for job in &snapshot.jobs {
        if job.id == 0
            || job.id >= snapshot.next_sequence
            || job.order < job.id
            || job.order >= snapshot.next_sequence
            || !ids.insert(job.id)
            || !orders.insert(job.order)
            || job.attempts.len() > MAX_JOB_ATTEMPTS
            || !job.reviewed.source().is_absolute()
            || !job.reviewed.destination().is_absolute()
            || (job.state == JobState::Running
                && job
                    .attempts
                    .last()
                    .is_none_or(|a| a.state != JobState::Running))
            || (job.state == JobState::Completed && job.result.is_none())
            || (job.state != JobState::Completed && job.result.is_some())
        {
            return Err(problem("invalid queue job identity/state; state preserved"));
        }
        for (i, attempt) in job.attempts.iter().enumerate() {
            if !matches!(
                attempt.state,
                JobState::Running
                    | JobState::Completed
                    | JobState::Failed
                    | JobState::Cancelled
                    | JobState::Interrupted
                    | JobState::BlockedStale
            ) || (attempt.state == JobState::Running
                && (job.state != JobState::Running || i + 1 != job.attempts.len()))
                || attempt.journal_dir
                    != root
                        .join("transactions")
                        .join(job.id.to_string())
                        .join((i + 1).to_string())
                || attempt.staging_root.parent() != job.reviewed.destination().parent()
                || !attempt
                    .staging_root
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(".emuwiz-conversion-queue-"))
            {
                return Err(problem("invalid queue attempt paths; state preserved"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
