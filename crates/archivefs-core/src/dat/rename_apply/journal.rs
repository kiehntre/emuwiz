//! Atomic text journal persistence for rename transactions.
//!
//! A journal is the source of truth for a rename batch: it is written before
//! the first mutation and updated after state transitions. The shared publisher
//! syncs the owned staging file and atomically replaces the journal, but parent
//! directory sync is best effort: success does not certify universal power-loss
//! durability. Recovery reads these journals; it never resumes automatically.
//!
//! # Format and privacy
//!
//! The journal is JSON, serialised from [`RenameTransaction`]. It deliberately
//! contains the full source and destination paths - rollback cannot work
//! without them - and this is the one place those paths are kept verbatim.
//! General History & Logs never sees them. Unknown fields written by a future
//! build round-trip verbatim rather than failing the read. No secrets are ever
//! written here.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ArchiveFsError;
use crate::dat::rename_apply::model::{EntryState, RenameTransaction, TransactionState};

/// The journal directory's name under the EmuWiz data directory.
/// Sealed journals live in a versioned folder that builds without seals never
/// open, so they cannot rewrite sealed recovery state.
pub const RENAME_TRANSACTIONS_DIRECTORY: &str = "rename-transactions-sealed-v1";
/// The folder used before seals existed; its journals are legacy and are only
/// inventoried, never rewritten or sealed.
pub const LEGACY_RENAME_TRANSACTIONS_DIRECTORY: &str = "rename-transactions";

/// How long a journal filename may grow before it is unusable.
const MAX_JOURNAL_NAME_BYTES: usize = 128;

/// Monotonic per-process sequence for transaction ids and journal names.
static TRANSACTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The default journal directory: the sealed, versioned folder under the effective
/// data directory (EmuWiz's or the legacy ArchiveFS one).
pub fn default_rename_transaction_dir() -> Result<PathBuf, ArchiveFsError> {
    Ok(crate::app_dirs::data_dir()?.join(RENAME_TRANSACTIONS_DIRECTORY))
}

/// The pre-seal journal folder, for read-only legacy inventory.
pub fn legacy_rename_transaction_dir() -> Result<PathBuf, ArchiveFsError> {
    Ok(crate::app_dirs::data_dir()?.join(LEGACY_RENAME_TRANSACTIONS_DIRECTORY))
}

/// The logic behind [`default_rename_transaction_dir`], with the home injected
/// so tests can exercise the no-home case without racing on the environment.
pub fn rename_transaction_dir_in(home: Option<OsString>) -> Result<PathBuf, ArchiveFsError> {
    let home = home.ok_or_else(|| ArchiveFsError::Config("HOME is not set".to_string()))?;
    Ok(crate::app_dirs::data_dir_in(Path::new(&home)).join(RENAME_TRANSACTIONS_DIRECTORY))
}

/// Per journal folder and transaction id: how many operations in this process
/// are applying, undoing or resuming it, and how many recovery reconciliations
/// are working on it. The two never overlap (see [`begin_reconcile`]).
struct LiveState {
    live: std::collections::BTreeMap<LiveKey, usize>,
    reconciling: std::collections::BTreeMap<LiveKey, usize>,
}

type LiveKey = (PathBuf, String);

static LIVE_STATE: std::sync::Mutex<LiveState> = std::sync::Mutex::new(LiveState {
    live: std::collections::BTreeMap::new(),
    reconciling: std::collections::BTreeMap::new(),
});
static LIVE_CHANGED: std::sync::Condvar = std::sync::Condvar::new();

fn live_state() -> std::sync::MutexGuard<'static, LiveState> {
    LIVE_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keyed by the folder exactly as the caller names it: every production
/// caller takes it from the same resolver ([`default_rename_transaction_dir`]
/// or a page's stored copy of it). A differently spelled folder only fails to
/// defer a reconciliation, which is the behaviour without this registry.
fn live_key(journal_dir: &Path, transaction_id: &str) -> LiveKey {
    (journal_dir.to_path_buf(), transaction_id.to_string())
}

fn release(counts: &mut std::collections::BTreeMap<LiveKey, usize>, key: &LiveKey) {
    if let Some(count) = counts.get_mut(key) {
        *count -= 1;
        if *count == 0 {
            counts.remove(key);
        }
    }
}

/// Marks a transaction as running in this process for as long as the guard
/// lives (also across a panic). Recovery reconciliation refuses a live
/// transaction: its in-flight journal belongs to the running operation, and
/// rewriting it underneath that operation races the operation's own journal
/// writes. A reconciliation already working on it is waited for first, so it
/// can never write a snapshot taken before the operation started.
/// In-process only; it does not exclude another process.
#[must_use = "the transaction is only marked live while the guard is held"]
pub struct LiveTransactionGuard {
    key: LiveKey,
}

/// See [`LiveTransactionGuard`].
pub fn mark_transaction_live(journal_dir: &Path, transaction_id: &str) -> LiveTransactionGuard {
    let key = live_key(journal_dir, transaction_id);
    let mut state = live_state();
    while state.reconciling.contains_key(&key) {
        state = LIVE_CHANGED
            .wait(state)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    *state.live.entry(key.clone()).or_insert(0) += 1;
    LiveTransactionGuard { key }
}

/// Whether an operation in this process is running this transaction now.
pub fn is_transaction_live(journal_dir: &Path, transaction_id: &str) -> bool {
    live_state()
        .live
        .contains_key(&live_key(journal_dir, transaction_id))
}

impl Drop for LiveTransactionGuard {
    fn drop(&mut self) {
        release(&mut live_state().live, &self.key);
        LIVE_CHANGED.notify_all();
    }
}

/// Held by a recovery reconciliation for its whole read-classify-write span.
pub(super) struct ReconcileGuard {
    key: LiveKey,
}

/// Atomically refuses a live transaction or claims it for reconciliation;
/// operations starting on the same transaction wait until the claim is dropped.
pub(super) fn begin_reconcile(journal_dir: &Path, transaction_id: &str) -> Option<ReconcileGuard> {
    let key = live_key(journal_dir, transaction_id);
    let mut state = live_state();
    if state.live.contains_key(&key) {
        return None;
    }
    *state.reconciling.entry(key.clone()).or_insert(0) += 1;
    Some(ReconcileGuard { key })
}

impl Drop for ReconcileGuard {
    fn drop(&mut self) {
        release(&mut live_state().reconciling, &self.key);
        LIVE_CHANGED.notify_all();
    }
}

/// A stable, unique transaction id: `<unix_seconds>-<sequence>`.
pub fn new_transaction_id(now_unix: u64) -> String {
    let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{now_unix}-{sequence}")
}

/// The journal filename for a transaction id, or `None` when the id is not a
/// safe single filename component.
pub fn journal_file_name(transaction_id: &str) -> Option<String> {
    if transaction_id.is_empty()
        || transaction_id.len() > MAX_JOURNAL_NAME_BYTES
        || transaction_id.contains(['/', '\\', '\0'])
        || transaction_id == "."
        || transaction_id == ".."
    {
        return None;
    }
    Some(format!("{transaction_id}.json"))
}

/// The full path of a transaction's journal.
pub fn journal_path(dir: &Path, transaction_id: &str) -> Option<PathBuf> {
    journal_file_name(transaction_id).map(|name| dir.join(name))
}

/// The only legal moves into, within and out of the Finalise states:
/// `Applied -> Finalising -> Finalised` (completion) and `Finalising ->
/// Applied` (cancel, while the original is proven intact). `Finalised` is
/// terminal and immutable: nothing may rewrite it, so a status edit can never
/// make a finalised replacement undoable again. Every other state pair is
/// outside this rule and unaffected.
fn finalise_transition_allowed(old: Option<TransactionState>, new: TransactionState) -> bool {
    use TransactionState::{Applied, Finalised, Finalising};
    match (old, new) {
        (Some(Finalised), _) => false,
        // Cancelling restores Undo, so it is only legal through the one caller
        // that has just proven the retained original intact.
        (Some(Finalising), Applied) => FINALISE_CANCEL.with(std::cell::Cell::get),
        (Some(Applied), Finalising) | (Some(Finalising), Finalising | Finalised) => true,
        (_, Finalising | Finalised) => false,
        (Some(Finalising), _) => false,
        _ => true,
    }
}

thread_local! {
    static FINALISE_CANCEL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` with the `Finalising -> Applied` (cancel) move permitted. Only
/// Finalise Replacement's own cancel paths call this, after proving the
/// retained original intact.
pub(crate) fn with_finalise_cancel<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            FINALISE_CANCEL.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(FINALISE_CANCEL.with(|c| c.replace(true)));
    f()
}

/// Reads a legacy journal entry's bytes without following a symlink or
/// waiting on a FIFO or device, through the shared
/// [`crate::launch::process_spawn::open_regular_file_nonblocking`]: the type is
/// checked on the opened descriptor rather than the path, so an entry swapped
/// after any earlier path check cannot substitute a nonregular object.
/// Refusals are `InvalidInput` (see [`is_not_regular`]).
pub(super) fn read_regular_journal(path: &Path) -> std::io::Result<Vec<u8>> {
    #[cfg(test)]
    BEFORE_REGULAR_OPEN.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(path);
        }
    });
    #[cfg(unix)]
    {
        use std::io::Read;
        let not_regular = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("journal {} is not a regular file", path.display()),
            )
        };
        let mut file =
            crate::launch::process_spawn::open_regular_file_nonblocking(path).map_err(|error| {
                // The helper's own type refusal carries no errno. ELOOP: a
                // symlink (O_NOFOLLOW). ENXIO: a socket, or a device with no
                // driver. Any other errno (EINVAL included) is a storage
                // error and passes through unchanged.
                if (error.kind() == std::io::ErrorKind::InvalidInput
                    && error.raw_os_error().is_none())
                    || matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENXIO))
                {
                    not_regular()
                } else {
                    error
                }
            })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::read(path)
    }
}

/// Whether [`read_regular_journal`] refused the entry's type.
pub(super) fn is_not_regular(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::InvalidInput && error.raw_os_error().is_none()
}

#[cfg(test)]
thread_local! {
    /// Runs as each [`read_regular_journal`] call starts, after any caller's
    /// own path check: lets a test swap the entry inside that window.
    pub(super) static BEFORE_REGULAR_OPEN: std::cell::RefCell<Option<Box<dyn FnMut(&Path)>>> =
        const { std::cell::RefCell::new(None) };
}

fn check_finalise_transition(path: &Path, next: &RenameTransaction) -> Result<(), ArchiveFsError> {
    let old = match read_regular_journal(path) {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => match serde_json::from_value::<TransactionState>(value["state"].clone()) {
                Ok(state) => Some(state),
                // Unreadable existing state: only a plain write may proceed
                // (the seal check refuses it anyway); never a Finalise move.
                Err(_) => {
                    if matches!(
                        next.state,
                        TransactionState::Finalising | TransactionState::Finalised
                    ) {
                        return Err(ArchiveFsError::Config(
                            "the journal on disk has an unreadable state; finalising refused"
                                .into(),
                        ));
                    }
                    return Ok(());
                }
            },
            Err(_) => None,
        },
        // A FIFO, socket, device or symlink is never a journal: refuse and
        // preserve it rather than let the write replace it.
        Err(error) if is_not_regular(&error) => {
            return Err(ArchiveFsError::Config(format!(
                "{error}; it is preserved and the journal write is refused"
            )));
        }
        Err(_) => None,
    };
    if finalise_transition_allowed(old, next.state) {
        Ok(())
    } else {
        Err(ArchiveFsError::Config(format!(
            "journal {} cannot move from {} to {}: finalised and finalising journals change only along Applied -> Finalising -> Finalised (or Finalising -> Applied) and a Finalised journal is immutable",
            next.transaction_id,
            old.map_or("nothing", TransactionState::label),
            next.state.label()
        )))
    }
}

/// Writes (or rewrites) a transaction journal through owned exclusive staging,
/// file `sync_all`, atomic replacement and best-effort parent-directory sync.
/// This is not a universal durable-commit or cross-process exclusion guarantee.
///
/// Every write is sealed independently (see [`super::seal`]): the journal now on
/// disk must be the sealed one before it may be replaced, the intent is recorded
/// before the write and committed after it. A changed, missing or legacy
/// unsealed journal is refused and left exactly as found.
pub fn write_journal(dir: &Path, transaction: &RenameTransaction) -> Result<(), ArchiveFsError> {
    #[cfg(target_os = "linux")]
    if super::compact_recovery::is_enrolled(dir, &transaction.transaction_id) {
        return Err(ArchiveFsError::Config(
            "compact transaction requires enrolled checkpoint authority".into(),
        ));
    }

    let name = journal_file_name(&transaction.transaction_id).ok_or_else(|| {
        ArchiveFsError::Config(format!(
            "transaction id '{}' cannot name a journal file",
            transaction.transaction_id
        ))
    })?;
    let path = dir.join(name);
    check_finalise_transition(&path, transaction)?;
    let body = serde_json::to_string_pretty(transaction).map_err(|error| {
        ArchiveFsError::Config(format!(
            "failed to serialize rename transaction journal: {error}"
        ))
    })?;
    let intent = super::seal::begin(dir, &path, transaction).map_err(ArchiveFsError::Config)?;
    #[cfg(test)]
    super::seal::hooks::AFTER_INTENT.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    crate::atomic_write_text(&path, &format!("{body}\n"))?;
    #[cfg(test)]
    if let Some(hook) = super::seal::hooks::BEFORE_COMMIT.with(|hook| hook.borrow_mut().take()) {
        hook().map_err(ArchiveFsError::Config)?;
    }
    super::seal::commit(intent).map_err(ArchiveFsError::Config)
}

/// Reads a journal back, tolerating unknown fields written by a future build.
pub fn read_journal(path: &Path) -> Result<RenameTransaction, ArchiveFsError> {
    #[cfg(target_os = "linux")]
    if let Some(result) = super::compact_recovery::enrolled_history_read(path) {
        let (tx, verified_historical) = result.map_err(|e| ArchiveFsError::io(path, e))?;
        // Only the compact reader's freshly authenticated, released settlement
        // can describe old names historically. No serialized flag grants this.
        if !verified_historical {
            verify_applied_containment(&tx)?;
        }
        return Ok(tx);
    }

    let text = read_regular_journal(path)
        .and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        })
        .map_err(|source| ArchiveFsError::io(path.to_path_buf(), source))?;
    let transaction: RenameTransaction = serde_json::from_str(&text).map_err(|error| {
        ArchiveFsError::Config(format!(
            "failed to parse rename transaction journal {}: {error}",
            path.display()
        ))
    })?;
    verify_applied_containment(&transaction)?;
    Ok(transaction)
}

pub(super) fn verify_applied_containment(
    transaction: &RenameTransaction,
) -> Result<(), ArchiveFsError> {
    #[cfg(target_os = "linux")]
    super::identity::fd_publication_probe::contract::verify_records(transaction)
        .map_err(|error| ArchiveFsError::Config(format!("FD recovery evidence: {error}")))?;
    // Applied is historical evidence, never proof of current containment.
    // Reject demonstrably redirected paths on reload; absence is handled by
    // existing identity/recovery checks rather than inventing a new location.
    for entry in &transaction.entries {
        if entry.state == super::model::EntryState::Applied
            && entry.operation == super::model::TransactionOperation::RenameMove
        {
            for path in [
                Some(Path::new(&transaction.source_scan_root)),
                entry.source_path.parent(),
                entry.destination_path.parent(),
            ]
            .into_iter()
            .flatten()
            {
                if path.is_absolute() && std::fs::canonicalize(path).is_ok_and(|real| real != path)
                {
                    return Err(ArchiveFsError::Config(format!(
                        "journal records historical Applied but {} is redirected; current containment is unverified; preserve journal and review recovery",
                        path.display()
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Every transaction journal in `dir`, parsed, in a stable order.
///
/// A journal that fails to parse is reported as a problem rather than silently
/// dropped: an unreadable journal may still be the only record of renames that
/// happened, and deleting it would destroy that record.
pub fn list_journals(dir: &Path) -> (Vec<RenameTransaction>, Vec<String>) {
    let mut transactions = Vec::new();
    let mut problems = Vec::new();
    let mut files = Vec::new();
    match super::lookup::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) if entry.path().extension().is_some_and(|ext| ext == "json") => {
                        files.push(entry.path())
                    }
                    Ok(_) => (),
                    Err(error) => problems.push(format!(
                        "journal enumeration incomplete at {}: {error}",
                        dir.display()
                    )),
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => problems.push(format!(
            "journal storage unavailable at {}: {error}",
            dir.display()
        )),
    }
    #[cfg(target_os = "linux")]
    if let Some(discovered) = super::compact_recovery::discovery_path(dir) {
        match discovered {
            Ok(path) if !files.contains(&path) => files.push(path),
            Ok(_) => (),
            Err(error) => problems.push(format!("compact recovery evidence: {error}")),
        }
    }
    files.sort();
    for path in files {
        match read_journal(&path) {
            Ok(transaction) => transactions.push(transaction),
            Err(error) => problems.push(format!("{}: {error}", path.display())),
        }
    }
    transactions.sort_by(|a, b| a.transaction_id.cmp(&b.transaction_id));
    (transactions, problems)
}

/// The transactions in `dir` that an interrupted batch left behind: journals
/// whose overall state is not settled, in a deterministic order.
pub fn find_recovery_transactions(dir: &Path) -> (Vec<RenameTransaction>, Vec<String>) {
    let (all, problems) = list_journals(dir);
    let interrupted: Vec<RenameTransaction> = all
        .into_iter()
        .filter(|transaction| transaction.state.needs_recovery())
        .collect();
    (interrupted, problems)
}

/// The transactions in `dir` a user can still act on after a restart: every
/// settled `Applied` transaction that still has applied entries (eligible for
/// rollback) plus every interrupted transaction recovery must surface.
///
/// Unlike [`find_recovery_transactions`], this does not drop a settled
/// `Applied` batch - a completed rename is still reversible, so its journal
/// must stay visible and rollback-able across restarts, exactly as the
/// release checklist's restart-and-rollback journey requires.
pub fn find_rollbackable_transactions(dir: &Path) -> (Vec<RenameTransaction>, Vec<String>) {
    let (all, problems) = list_journals(dir);
    let rollbackable: Vec<RenameTransaction> = all
        .into_iter()
        .filter(RenameTransaction::is_rollbackable)
        .collect();
    (rollbackable, problems)
}

/// Whether a transaction's journal still exists on disk.
pub fn journal_exists(dir: &Path, transaction_id: &str) -> bool {
    journal_path(dir, transaction_id)
        .is_some_and(|path| super::lookup::symlink_metadata(&path).is_ok())
}

/// Persists the user's choice to leave an interrupted transaction
/// untouched: reads the current journal, records
/// [`RecoveryResolution::LeaveUntouched`] and when, and writes the whole
/// transaction back durably via [`write_journal`]. `state` and every
/// [`crate::dat::rename_apply::model::TransactionEntry`] are read back and
/// written out completely unchanged - this never claims a rename happened,
/// never rolls anything back, and never removes the journal.
///
/// Idempotent: resolving an already-resolved transaction again just
/// re-records the same choice (and a newer timestamp), which is harmless.
/// Returns the updated transaction so a caller can refresh its own
/// in-memory view without a second read.
pub fn resolve_leave_untouched(
    dir: &Path,
    transaction_id: &str,
) -> Result<RenameTransaction, ArchiveFsError> {
    let path = journal_path(dir, transaction_id).ok_or_else(|| {
        ArchiveFsError::Config(format!(
            "transaction id '{transaction_id}' cannot name a journal file"
        ))
    })?;
    let mut transaction = read_journal(&path)?;
    transaction.recovery_resolution =
        Some(crate::dat::rename_apply::model::RecoveryResolution::LeaveUntouched);
    transaction.recovery_resolved_at_unix = Some(crate::dat::sources::now_unix());
    write_journal(dir, &transaction)?;
    Ok(transaction)
}

/// Removes a transaction's journal. Used only after a transaction is fully
/// settled and the user has dismissed it from recovery.
pub fn remove_journal(dir: &Path, transaction_id: &str) -> Result<(), ArchiveFsError> {
    #[cfg(target_os = "linux")]
    if super::compact_recovery::is_enrolled(dir, transaction_id) {
        return Err(ArchiveFsError::Config(
            "compact recovery evidence is retained; use verified enrolled release".into(),
        ));
    }
    let Some(path) = journal_path(dir, transaction_id) else {
        return Ok(());
    };
    let present = match super::lookup::symlink_metadata(&path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(ArchiveFsError::io(&path, error)),
    };
    if !present {
        return Ok(());
    }
    // A finalised (or finalising) journal is the audit record of a deliberate,
    // irreversible deletion; automatic log removal never discards it.
    let bytes = match read_regular_journal(&path) {
        Err(error) if is_not_regular(&error) => {
            return Err(ArchiveFsError::Config(format!(
                "{error}; it is preserved and not removed"
            )));
        }
        result => result.ok(),
    };
    if bytes.is_some_and(|bytes| {
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .is_ok_and(|v| matches!(v["state"].as_str(), Some("finalising" | "finalised")))
    }) {
        return Err(ArchiveFsError::Config(
            "a finalised replacement's journal is retained as its audit record and is not removed"
                .into(),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let tx = read_journal(&path)?;
        if tx.entries.iter().any(|entry| {
            super::identity::fd_publication_probe::contract::enrolled(entry)
                && entry
                    .unknown
                    .contains_key(super::identity::fd_publication_probe::contract::KEY)
        }) {
            return Err(ArchiveFsError::Config(
                "FD recovery records require explicit operator review; automatic log removal refused".into()));
        }
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(ArchiveFsError::io(path, source)),
    }
    super::seal::forget(dir, transaction_id).map_err(ArchiveFsError::Config)
}

/// Counts applied entries in a state persisted to the journal.
pub fn persisted_applied_count(transaction: &RenameTransaction) -> usize {
    transaction
        .entries
        .iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .count()
}

/// The overall state a transaction should move to after an apply pass.
pub fn terminal_state_after_apply(transaction: &RenameTransaction) -> TransactionState {
    if transaction.failed_count() > 0 {
        TransactionState::ApplyFailed
    } else {
        TransactionState::Applied
    }
}

/// Internal executor seam. Public write_journal remains immutable and legacy.
/// A selected compact writer always returns its error; never retry as legacy.
pub(super) fn checkpoint_journal(
    dir: &Path,
    tx: &mut RenameTransaction,
) -> Result<(), ArchiveFsError> {
    #[cfg(target_os = "linux")]
    if let Some(result) = super::compact_recovery::enrolled_checkpoint(dir, tx) {
        return result.map_err(|e| ArchiveFsError::io(dir, e));
    }
    write_journal(dir, tx)
}

#[cfg(all(test, unix))]
pub(crate) mod fifo_guard {
    //! Bounds a FIFO regression: a defect that opens the FIFO for a blocking
    //! read fails the test within about ten seconds instead of hanging the suite.
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    /// Runs `work`, and panics if it opened any of `fifos` for a blocking read.
    /// Every ten seconds `work` is still running, a watchdog opens each FIFO
    /// write-only and nonblocking. That open succeeds only while a reader holds
    /// the FIFO (a reader still there after ten seconds is a blocked one; a
    /// safe nonblocking open lasts microseconds), and, being a writer, it
    /// releases that reader so `work` returns. A merely slow `work` that never
    /// opened a FIFO is not reported.
    pub(crate) fn bounded<T>(fifos: &[&Path], work: impl FnOnce() -> T) -> T {
        let fired = Arc::new(AtomicBool::new(false));
        let (done, wait) = mpsc::channel::<()>();
        let watchdog = {
            let fired = Arc::clone(&fired);
            let fifos: Vec<_> = fifos.iter().map(|path| path.to_path_buf()).collect();
            std::thread::spawn(move || {
                while let Err(mpsc::RecvTimeoutError::Timeout) =
                    wait.recv_timeout(Duration::from_secs(10))
                {
                    for fifo in &fifos {
                        let reader = std::fs::OpenOptions::new()
                            .write(true)
                            .custom_flags(libc::O_NONBLOCK)
                            .open(fifo)
                            .is_ok();
                        if reader {
                            fired.store(true, Ordering::SeqCst);
                        }
                    }
                }
            })
        };
        let result = work();
        done.send(()).unwrap();
        watchdog.join().unwrap();
        assert!(
            !fired.load(Ordering::SeqCst),
            "opened a FIFO journal entry for a blocking read"
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_directory_uses_emuwiz_for_a_fresh_home() {
        let home = tempfile::tempdir().unwrap();
        let path = rename_transaction_dir_in(Some(home.path().as_os_str().to_os_string())).unwrap();
        assert_eq!(
            path,
            home.path()
                .join(".local/share/emuwiz")
                .join(RENAME_TRANSACTIONS_DIRECTORY)
        );
    }

    #[test]
    fn journal_directory_reuses_legacy_and_prefers_emuwiz() {
        let home = tempfile::tempdir().unwrap();
        let legacy = home.path().join(".local/share/archivefs");
        std::fs::create_dir_all(&legacy).unwrap();
        let home_arg = Some(home.path().as_os_str().to_os_string());
        assert_eq!(
            rename_transaction_dir_in(home_arg.clone()).unwrap(),
            legacy.join(RENAME_TRANSACTIONS_DIRECTORY)
        );

        let primary = home.path().join(".local/share/emuwiz");
        std::fs::create_dir_all(&primary).unwrap();
        assert_eq!(
            rename_transaction_dir_in(home_arg).unwrap(),
            primary.join(RENAME_TRANSACTIONS_DIRECTORY)
        );
    }

    /// Upgrading an install whose pre-seal journal folder holds an interrupted
    /// transaction: sealed journals resolve to the versioned folder beside it,
    /// and the legacy journal is only inventoried - found exactly once, never
    /// rewritten, sealed, copied or adopted as sealed recovery state.
    #[test]
    fn a_legacy_in_flight_journal_is_inventoried_after_upgrade_without_adoption() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".local/share/archivefs");
        let legacy_dir = data.join(LEGACY_RENAME_TRANSACTIONS_DIRECTORY);
        std::fs::create_dir_all(&legacy_dir).unwrap();

        // Written by a build without seals: a plain file, no seal.
        let mut tx = transaction(TransactionState::Applying);
        tx.transaction_id = "legacy-in-flight".to_string();
        let path = journal_path(&legacy_dir, &tx.transaction_id).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&tx).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();

        let resolved_dir =
            rename_transaction_dir_in(Some(home.path().as_os_str().to_os_string())).unwrap();
        assert_eq!(resolved_dir, data.join(RENAME_TRANSACTIONS_DIRECTORY));
        assert_ne!(resolved_dir, legacy_dir);

        let inventory = super::super::seal::inventory(&legacy_dir);
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].transaction_id, "legacy-in-flight");
        assert_eq!(inventory[0].seal, super::super::seal::SealCheck::Legacy);
        assert_eq!(inventory[0].state, Some(TransactionState::Applying));

        // A legacy journal is never rewritten (and so never sealed).
        let refused = write_journal(&legacy_dir, &tx).unwrap_err().to_string();
        assert!(refused.contains("legacy unsealed journal"), "{refused}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read_dir(&legacy_dir).unwrap().count(), 1);
        assert!(!resolved_dir.exists(), "resolution creates nothing");
    }
    use crate::dat::rename_apply::model::{ObjectIdentity, ObjectKind, TransactionEntry};
    use std::path::PathBuf;

    fn entry(source: &str, destination: &str) -> TransactionEntry {
        TransactionEntry {
            source_path: PathBuf::from(source),
            destination_path: PathBuf::from(destination),
            original_basename: "old.bin".to_string(),
            proposed_basename: "new.bin".to_string(),
            identity: ObjectIdentity {
                freshness: None,
                size_bytes: 100,
                modified_unix: 1,
                kind: ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 7,
                #[cfg(unix)]
                dev: 8,
            },
            operation: Default::default(),
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }
    }

    fn transaction(state: TransactionState) -> RenameTransaction {
        RenameTransaction {
            transaction_id: "1-0".to_string(),
            plan_generation: 3,
            classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
            created_at_unix: 10,
            source_scan_root: "/tmp/roms".to_string(),
            state,
            entries: vec![entry("/tmp/roms/a.bin", "/tmp/roms/A.bin")],
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    #[test]
    fn a_journal_round_trips_through_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let tx = transaction(TransactionState::Planned);
        write_journal(dir.path(), &tx).unwrap();
        let loaded = read_journal(&journal_path(dir.path(), "1-0").unwrap()).unwrap();
        assert_eq!(loaded, tx);
    }

    #[test]
    fn unknown_future_fields_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1-0.json");
        std::fs::write(
            &path,
            r#"{
  "transaction_id": "1-0",
  "plan_generation": 3,
  "created_at_unix": 10,
  "source_scan_root": "/tmp/roms",
  "state": "planned",
  "future_transaction_field": 42,
  "entries": [
    {
      "source_path": "/tmp/roms/a.bin",
      "destination_path": "/tmp/roms/A.bin",
      "original_basename": "old.bin",
      "proposed_basename": "new.bin",
      "identity": {"size_bytes": 100, "modified_unix": 1, "kind": "regular_file"},
      "preflight_passed": false,
      "preflight_failures": [],
      "state": "planned",
      "future_entry_field": "kept"
    }
  ]
}
"#,
        )
        .unwrap();
        let loaded = read_journal(&path).unwrap();
        assert_eq!(loaded.state, TransactionState::Planned);
        assert_eq!(
            loaded.unknown.get("future_transaction_field"),
            Some(&serde_json::json!(42))
        );
        assert_eq!(
            loaded.entries[0].unknown.get("future_entry_field"),
            Some(&serde_json::json!("kept"))
        );
    }

    #[test]
    fn an_older_journal_without_classifier_version_decodes_as_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        std::fs::write(&path, r#"{"transaction_id":"x","plan_generation":0,"created_at_unix":0,"source_scan_root":"","entries":[]}"#).unwrap();
        let loaded = read_journal(&path).unwrap();
        assert_eq!(loaded.state, TransactionState::Planned);
        assert_eq!(loaded.classifier_version, None);
        assert!(loaded.entries.is_empty());
    }

    #[test]
    fn incomplete_transactions_are_found_and_settled_ones_are_not() {
        let dir = tempfile::tempdir().unwrap();
        for (id, state) in [
            ("interrupted", TransactionState::Applying),
            ("failed", TransactionState::ApplyFailed),
            ("settled", TransactionState::RolledBack),
            ("complete", TransactionState::Applied),
        ] {
            let mut tx = transaction(state);
            tx.transaction_id = id.to_string();
            write_journal(dir.path(), &tx).unwrap();
        }
        let (recovery, problems) = find_recovery_transactions(dir.path());
        assert!(problems.is_empty(), "{problems:?}");
        let ids: Vec<&str> = recovery
            .iter()
            .map(|tx| tx.transaction_id.as_str())
            .collect();
        assert_eq!(ids, vec!["failed", "interrupted"]);
    }

    #[test]
    fn an_unparseable_journal_is_reported_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("corrupt.json"), "not json {{").unwrap();
        let (recovery, problems) = find_recovery_transactions(dir.path());
        assert!(recovery.is_empty());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(std::fs::symlink_metadata(dir.path().join("corrupt.json")).is_ok());
    }

    #[test]
    fn rollbackable_transactions_include_applied_but_not_rolled_back() {
        let dir = tempfile::tempdir().unwrap();
        for (id, state) in [
            ("applied", TransactionState::Applied),
            ("interrupted", TransactionState::Applying),
            ("rolled-back", TransactionState::RolledBack),
        ] {
            let mut tx = transaction(state);
            tx.transaction_id = id.to_string();
            // A settled Applied transaction must actually have an Applied entry
            // to be reversible.
            if state == TransactionState::Applied {
                tx.entries[0].state = EntryState::Applied;
            }
            write_journal(dir.path(), &tx).unwrap();
        }
        let (rollbackable, problems) = find_rollbackable_transactions(dir.path());
        assert!(problems.is_empty(), "{problems:?}");
        let ids: Vec<&str> = rollbackable
            .iter()
            .map(|tx| tx.transaction_id.as_str())
            .collect();
        assert_eq!(ids, vec!["applied", "interrupted"]);
    }

    #[test]
    fn an_applied_journal_with_no_applied_entries_is_not_offered_for_rollback() {
        let dir = tempfile::tempdir().unwrap();
        // A settled Applied transaction whose entries are not Applied has
        // nothing to reverse; it must not be offered for rollback.
        let tx = transaction(TransactionState::Applied);
        write_journal(dir.path(), &tx).unwrap();
        let (rollbackable, _) = find_rollbackable_transactions(dir.path());
        assert!(rollbackable.is_empty());
    }

    #[test]
    fn an_unparseable_journal_is_still_reported_by_rollbackable_discovery() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("corrupt.json"), "not json {{").unwrap();
        let (rollbackable, problems) = find_rollbackable_transactions(dir.path());
        assert!(rollbackable.is_empty());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(std::fs::symlink_metadata(dir.path().join("corrupt.json")).is_ok());
    }
    #[test]
    fn injected_journal_directory_errors_are_not_empty_history() {
        for errno in [
            libc::EIO,
            libc::ENODEV,
            libc::ENOTCONN,
            libc::ESTALE,
            libc::EACCES,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let journals = dir.path().join("journals");
            let tx = transaction(TransactionState::Applying);
            write_journal(&journals, &tx).unwrap();
            let _fault = super::super::lookup::Fault::at(&journals, errno);
            let (found, problems) = list_journals(&journals);
            assert!(found.is_empty());
            assert!(!problems.is_empty(), "errno={errno}: lost discovery error");
        }
    }

    #[test]
    fn injected_journal_lookup_errors_cannot_authorize_removal() {
        let dir = tempfile::tempdir().unwrap();
        let tx = transaction(TransactionState::ApplyFailed);
        write_journal(dir.path(), &tx).unwrap();
        let path = journal_path(dir.path(), &tx.transaction_id).unwrap();
        let before = std::fs::read(&path).unwrap();
        let _fault = super::super::lookup::Fault::at(&path, libc::EIO);
        assert!(remove_journal(dir.path(), &tx.transaction_id).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unavailable_compact_namespace_cannot_select_legacy_or_empty_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let namespace = dir.path().join("emuwiz-enrolled-recovery-v3");
        for errno in [libc::EIO, libc::ENODEV, libc::ENOTCONN, libc::ESTALE] {
            let _fault = super::super::lookup::Fault::at(&namespace, errno);
            assert!(
                super::super::compact_recovery::is_enrolled(dir.path(), "uncertain"),
                "errno={errno}: permitted legacy route on unavailable namespace"
            );
            let (found, problems) = list_journals(dir.path());
            assert!(found.is_empty());
            assert!(!problems.is_empty());
        }
    }

    fn mkfifo(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_regular_legacy_journal_is_still_read_listed_rewritten_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let tx = transaction(TransactionState::Planned);
        write_journal(dir.path(), &tx).unwrap();
        let path = journal_path(dir.path(), "1-0").unwrap();
        let bytes = read_regular_journal(&path).unwrap();
        assert_eq!(bytes, std::fs::read(&path).unwrap());
        assert_eq!(read_journal(&path).unwrap(), tx);
        let (found, problems) = list_journals(dir.path());
        assert_eq!(found, vec![tx.clone()]);
        assert!(problems.is_empty(), "{problems:?}");
        let mut next = tx.clone();
        next.state = TransactionState::Applying;
        write_journal(dir.path(), &next).unwrap();
        assert_eq!(read_journal(&path).unwrap(), next);
        let mut settled = next;
        settled.state = TransactionState::RolledBack;
        write_journal(dir.path(), &settled).unwrap();
        remove_journal(dir.path(), "1-0").unwrap();
        assert!(std::fs::symlink_metadata(&path).is_err());
    }

    /// Every legacy journal reader and writer refuses a FIFO without opening
    /// it for a blocking read, and leaves it in place.
    #[cfg(unix)]
    #[test]
    fn a_fifo_journal_entry_is_refused_by_every_legacy_route_without_blocking() {
        use std::os::unix::fs::FileTypeExt;
        let dir = tempfile::tempdir().unwrap();
        let path = journal_path(dir.path(), "1-0").unwrap();
        mkfifo(&path);
        fifo_guard::bounded(&[&path], || {
            let error = read_regular_journal(&path).unwrap_err();
            assert!(is_not_regular(&error), "{error}");
            assert!(
                read_journal(&path)
                    .unwrap_err()
                    .to_string()
                    .contains("not a regular file")
            );
            let (found, problems) = list_journals(dir.path());
            assert!(found.is_empty());
            assert_eq!(problems.len(), 1, "{problems:?}");
            assert!(problems[0].contains("not a regular file"), "{problems:?}");
            let error = write_journal(dir.path(), &transaction(TransactionState::Planned))
                .unwrap_err()
                .to_string();
            assert!(error.contains("not a regular file"), "{error}");
            let error = remove_journal(dir.path(), "1-0").unwrap_err().to_string();
            assert!(error.contains("not a regular file"), "{error}");
            assert!(resolve_leave_untouched(dir.path(), "1-0").is_err());
        });
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_fifo()
        );
    }

    /// A regular journal later replaced in place (by rename, as a swap would)
    /// with a FIFO or a symlink: the next rewrite, read and removal see the
    /// replacement, not the earlier regular file, and preserve it.
    #[cfg(unix)]
    #[test]
    fn a_regular_journal_replaced_by_a_fifo_or_symlink_is_refused_and_preserved() {
        use std::os::unix::fs::FileTypeExt;
        let dir = tempfile::tempdir().unwrap();
        let mut tx = transaction(TransactionState::Planned);
        write_journal(dir.path(), &tx).unwrap();
        let path = journal_path(dir.path(), "1-0").unwrap();
        let staged = dir.path().join("staged-fifo");
        mkfifo(&staged);
        std::fs::rename(&staged, &path).unwrap();
        tx.state = TransactionState::Applying;
        fifo_guard::bounded(&[&path], || {
            let error = write_journal(dir.path(), &tx).unwrap_err().to_string();
            assert!(error.contains("not a regular file"), "{error}");
            assert!(read_journal(&path).is_err());
            assert!(remove_journal(dir.path(), "1-0").is_err());
        });
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_fifo()
        );

        let decoy = dir.path().join("decoy.txt");
        std::fs::write(&decoy, "not a journal").unwrap();
        let staged = dir.path().join("staged-link");
        std::os::unix::fs::symlink(&decoy, &staged).unwrap();
        std::fs::rename(&staged, &path).unwrap();
        let error = write_journal(dir.path(), &tx).unwrap_err().to_string();
        assert!(error.contains("not a regular file"), "{error}");
        assert!(remove_journal(dir.path(), "1-0").is_err());
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&decoy).unwrap(), "not a journal");
    }

    /// Installs a [`BEFORE_REGULAR_OPEN`] hook that, on the `nth` safe open of
    /// `target` (counting from 1), replaces it with a FIFO by rename: the swap
    /// lands after the caller's own path check and before the open.
    #[cfg(unix)]
    fn swap_for_fifo_on_open(target: &Path, nth: usize) {
        let target = target.to_path_buf();
        let staged = target.with_extension("staged-fifo");
        mkfifo(&staged);
        let mut seen = 0;
        BEFORE_REGULAR_OPEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path: &Path| {
                if path == target {
                    seen += 1;
                    if seen == nth {
                        std::fs::rename(&staged, &target).unwrap();
                    }
                }
            }));
        });
    }

    #[cfg(unix)]
    fn clear_open_hook() {
        BEFORE_REGULAR_OPEN.with(|hook| *hook.borrow_mut() = None);
    }

    /// The seal's journal read passes its own path check on the regular
    /// journal, then meets a FIFO swapped in before its open: the descriptor
    /// check refuses it as a seal mismatch, without blocking, and keeps it.
    #[cfg(unix)]
    #[test]
    fn a_fifo_swapped_in_after_the_seal_path_check_is_refused_on_the_descriptor() {
        use std::os::unix::fs::FileTypeExt;
        let dir = tempfile::tempdir().unwrap();
        let mut tx = transaction(TransactionState::Planned);
        write_journal(dir.path(), &tx).unwrap();
        let path = journal_path(dir.path(), "1-0").unwrap();
        // Open 1: check_finalise_transition. Open 2: the seal's disk read.
        swap_for_fifo_on_open(&path, 2);
        tx.state = TransactionState::Applying;
        let error = fifo_guard::bounded(&[&path], || {
            write_journal(dir.path(), &tx).unwrap_err().to_string()
        });
        clear_open_hook();
        assert!(super::super::seal::is_seal_refusal(&error), "{error}");
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_fifo()
        );
    }

    /// The same for the journal folder's identity file.
    #[cfg(unix)]
    #[test]
    fn a_fifo_swapped_in_after_the_identity_path_check_is_refused_on_the_descriptor() {
        use std::os::unix::fs::FileTypeExt;
        let dir = tempfile::tempdir().unwrap();
        let mut tx = transaction(TransactionState::Planned);
        write_journal(dir.path(), &tx).unwrap();
        let identity = dir.path().join(super::super::seal::NAMESPACE_FILE);
        let before = std::fs::read(journal_path(dir.path(), "1-0").unwrap()).unwrap();
        swap_for_fifo_on_open(&identity, 1);
        tx.state = TransactionState::Applying;
        let error = fifo_guard::bounded(&[&identity], || {
            write_journal(dir.path(), &tx).unwrap_err().to_string()
        });
        clear_open_hook();
        assert!(error.contains("not a plain file"), "{error}");
        assert!(
            std::fs::symlink_metadata(&identity)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        assert_eq!(
            std::fs::read(journal_path(dir.path(), "1-0").unwrap()).unwrap(),
            before
        );
    }

    /// The journal folder's identity file is read the same way: a FIFO there
    /// refuses the write as a seal mismatch without blocking.
    #[cfg(unix)]
    #[test]
    fn a_fifo_journal_folder_identity_refuses_the_write_without_blocking() {
        use std::os::unix::fs::FileTypeExt;
        let dir = tempfile::tempdir().unwrap();
        let identity = dir.path().join(super::super::seal::NAMESPACE_FILE);
        mkfifo(&identity);
        let error = fifo_guard::bounded(&[&identity], || {
            write_journal(dir.path(), &transaction(TransactionState::Planned))
                .unwrap_err()
                .to_string()
        });
        assert!(error.contains("not a plain file"), "{error}");
        assert!(
            std::fs::symlink_metadata(&identity)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        assert!(std::fs::symlink_metadata(journal_path(dir.path(), "1-0").unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn socket_device_directory_and_symlink_journal_entries_are_refused_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let tx = transaction(TransactionState::Planned);
        // Socket.
        let socket = journal_path(dir.path(), "1-0").unwrap();
        // mknod, not bind: a socket inode without SUN_LEN's path limit.
        {
            use std::os::unix::ffi::OsStrExt;
            let cpath = std::ffi::CString::new(socket.as_os_str().as_bytes()).unwrap();
            assert_eq!(
                unsafe { libc::mknod(cpath.as_ptr(), libc::S_IFSOCK | 0o600, 0) },
                0
            );
        }
        // Directory.
        let folder = dir.path().join("folder.json");
        std::fs::create_dir(&folder).unwrap();
        // Symlink to a real, valid journal elsewhere: never followed.
        let elsewhere = tempfile::tempdir().unwrap();
        let mut linked_tx = tx.clone();
        linked_tx.transaction_id = "linked".into();
        write_journal(elsewhere.path(), &linked_tx).unwrap();
        let target = journal_path(elsewhere.path(), "linked").unwrap();
        let target_before = std::fs::read(&target).unwrap();
        let link = journal_path(dir.path(), "linked").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        // Character device, reached through a symlink and directly.
        let device = journal_path(dir.path(), "device").unwrap();
        std::os::unix::fs::symlink("/dev/null", &device).unwrap();
        for path in [&socket, &folder, &link, &device, Path::new("/dev/null")] {
            let error = read_regular_journal(path).unwrap_err();
            assert!(is_not_regular(&error), "{}: {error}", path.display());
            assert!(read_journal(path).is_err(), "{}", path.display());
        }
        let (found, problems) = list_journals(dir.path());
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(
            problems.iter().all(|p| p.contains("not a regular file")),
            "{problems:?}"
        );
        for (id, mut next) in [("1-0", tx.clone()), ("linked", linked_tx), ("device", tx)] {
            next.transaction_id = id.into();
            let error = write_journal(dir.path(), &next).unwrap_err().to_string();
            assert!(error.contains("not a regular file"), "{id}: {error}");
            let error = remove_journal(dir.path(), id).unwrap_err().to_string();
            assert!(error.contains("not a regular file"), "{id}: {error}");
        }
        use std::os::unix::fs::FileTypeExt;
        assert!(
            std::fs::symlink_metadata(&socket)
                .unwrap()
                .file_type()
                .is_socket()
        );
        assert!(std::fs::symlink_metadata(&folder).unwrap().is_dir());
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::symlink_metadata(&device)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), target_before);
    }
}
