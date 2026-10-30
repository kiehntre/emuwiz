//! Manual, conservative staging retirement. No age or basename grants ownership.
//! Only a completed Undo's redundant reconstructed output is eligible for
//! `cleanup`. Originals and pre-publication failures remain retained there.
//! The one way to surrender Undo is `finalise_replacement`: a separately
//! confirmed, journalled and sealed transition that is recorded before the
//! retained original is touched, and that keeps the journal as history.
use super::MAME_RECONSTRUCTION_WORKFLOW;
use super::owned_attempt::Directory;
use crate::dat::rename_apply::identity::{capture_identity, identity_matches};
use crate::dat::rename_apply::model::{
    EntryState, ObjectIdentity, RenameTransaction, TransactionOperation, TransactionState,
};
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagingItem {
    pub path: PathBuf,
    pub transaction_id: Option<String>,
    pub transaction_state: Option<TransactionState>,
    pub retention_reason: String,
    pub cleanup_eligible: bool,
    /// The retained original of a published replacement that may be finalised
    /// (Undo surrendered, original deleted) after separate confirmation.
    #[serde(default)]
    pub finalise_eligible: bool,
    /// Exact object reviewed by the caller; not inferred again at confirmation.
    pub identity: Option<ObjectIdentity>,
}

fn unowned_reason(path: &Path) -> &'static str {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("mame-") && name.ends_with(".zip.staged"))
    {
        if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.is_symlink()) {
            return "No recovery journal owns this staged entry, and it is missing or not a regular file. It was not followed or changed. A fresh publication will refuse this path; inspect it in Recovery before retrying.";
        }
        "No recovery journal owns this staged file. It may be an interrupted write or a foreign file; it cannot be published, adopted, undone or removed automatically. It is retained in place. Preview again to make a fresh, separately approved attempt."
    } else {
        "Unknown or manually added file; retained without ownership proof"
    }
}

/// Locks are advisory application coordination; identity is separate evidence.
/// Existing-only opens ensure that inventory never creates directories/files.
pub struct StagingLock {
    staging: Directory,
    journals: Directory,
}
impl StagingLock {
    pub fn acquire(staging: &Path, journals: &Path) -> Result<Self, String> {
        if staging == journals {
            return Err("staging and journals must be separate".into());
        }
        let staging = Directory::existing(staging)?.ok_or("staging directory does not exist")?;
        staging.lock()?;
        let journals = Directory::existing(journals)?
            .ok_or("journal directory does not exist; ownership cannot be proven")?;
        journals.lock()?;
        Ok(Self { staging, journals })
    }
    fn transactions(&self) -> Result<Vec<RenameTransaction>, String> {
        let txs = self.journals.transactions()?;
        let mut ids = std::collections::BTreeSet::new();
        // A canonical transaction must occupy its own journal slot. A second,
        // mismatched or substituted record is ambiguous ownership, not history.
        for t in &txs {
            if t.transaction_id.contains('/') || t.transaction_id.contains('\\') {
                return Err("unsafe journal transaction ID".into());
            }
            let value = self
                .journals
                .read_json(&format!("{}.json", t.transaction_id))?;
            let canonical: RenameTransaction =
                serde_json::from_value(value).map_err(|e| e.to_string())?;
            if canonical != *t {
                return Err("ambiguous journal slot; cleanup refused".into());
            }
        }
        for t in &txs {
            if t.transaction_id.is_empty() || !ids.insert(&t.transaction_id) {
                return Err("ambiguous recovery journal transaction IDs; cleanup refused".into());
            }
        }
        Ok(txs)
    }
}

/// Read-only inventory including unknown files and missing transaction sources.
/// Unreadable/ambiguous journals refuse the entire inventory rather than hide a
/// potential reference. No unknown file is opened or content-hashed.
pub fn inventory(staging: &Path, journals: &Path) -> Result<Vec<StagingItem>, String> {
    if Directory::existing(journals)?.is_none() {
        let Some(dir) = Directory::existing(staging)? else {
            return Ok(Vec::new());
        };
        dir.lock()?;
        let mut items = Vec::new();
        for e in std::fs::read_dir(&dir.path).map_err(|e| e.to_string())? {
            let path = e.map_err(|e| e.to_string())?.path();
            items.push(StagingItem {
                retention_reason: unowned_reason(&path).into(),
                path,
                transaction_id: None,
                transaction_state: None,
                identity: None,
                cleanup_eligible: false,
                finalise_eligible: false,
            });
        }
        dir.require_binding()?;
        items.sort_by(|a, b| a.path.cmp(&b.path));
        return Ok(items);
    }
    if Directory::existing(staging)?.is_none() {
        return inventory_without_staging(staging, journals);
    }
    let lock = StagingLock::acquire(staging, journals)?;
    inventory_locked(&lock)
}
/// A never-rebuilt folder has no staging directory: that is an empty inventory,
/// unless a journal still references a path inside it, which stays refused.
fn inventory_without_staging(staging: &Path, journals: &Path) -> Result<Vec<StagingItem>, String> {
    if staging == journals {
        return Err("staging and journals must be separate".into());
    }
    let Some(journals) = Directory::existing(journals)? else {
        return Ok(Vec::new());
    };
    journals.lock()?;
    let referenced = journals.transactions()?.iter().any(|t| {
        t.entries
            .iter()
            .any(|e| e.source_path.parent() == Some(staging))
    });
    journals.require_binding()?;
    if referenced {
        return Err("staging directory does not exist but a recovery journal references it".into());
    }
    Ok(Vec::new())
}
fn inventory_locked(lock: &StagingLock) -> Result<Vec<StagingItem>, String> {
    let txs = lock.transactions()?;
    let mut paths = std::collections::BTreeSet::new();
    for e in std::fs::read_dir(&lock.staging.path).map_err(|e| e.to_string())? {
        paths.insert(e.map_err(|e| e.to_string())?.path());
    }
    for t in &txs {
        for e in &t.entries {
            if e.source_path.parent() == Some(lock.staging.path.as_path()) {
                paths.insert(e.source_path.clone());
            }
        }
    }
    let mut result = Vec::new();
    for path in paths {
        result.push(classify(lock, &txs, path)?);
    }
    lock.staging.require_binding()?;
    lock.journals.require_binding()?;
    Ok(result)
}
fn contains_path(value: &serde_json::Value, path: &Path) -> bool {
    match value {
        serde_json::Value::String(s) => s.contains(path.to_string_lossy().as_ref()),
        serde_json::Value::Array(a) => a.iter().any(|v| contains_path(v, path)),
        serde_json::Value::Object(o) => o
            .iter()
            .any(|(k, v)| k.contains(path.to_string_lossy().as_ref()) || contains_path(v, path)),
        _ => false,
    }
}
fn references(t: &RenameTransaction, path: &Path) -> bool {
    serde_json::to_value(t).map_or(true, |v| contains_path(&v, path))
}
fn extra_references(t: &RenameTransaction, path: &Path) -> bool {
    let Ok(mut value) = serde_json::to_value(t) else {
        return true;
    };
    if let Some(entries) = value["entries"].as_array_mut() {
        for e in entries {
            if let Some(o) = e.as_object_mut() {
                o.remove("source_path");
                o.remove("destination_path");
            }
        }
    }
    contains_path(&value, path)
}
/// Journal key holding the informational timestamps of a Finalise Replacement.
/// The authority is `TransactionState::Finalising`/`Finalised`, never this key.
pub const FINALISE_KEY: &str = "mame_replacement_finalise";

/// Where a Finalise Replacement stands. `Finalising` is written (sealed) before
/// the retained original is touched; `Finalised` only after it is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finalise {
    Finalising {
        started_unix: u64,
    },
    Finalised {
        started_unix: u64,
        finished_unix: u64,
    },
}

/// Read where a Finalise Replacement stands, from the journal state alone.
pub fn finalise_marker(t: &RenameTransaction) -> Option<Finalise> {
    let started_unix = t
        .unknown
        .get(FINALISE_KEY)
        .and_then(|v| v["started_unix"].as_u64())
        .unwrap_or(0);
    match t.state {
        TransactionState::Finalising => Some(Finalise::Finalising { started_unix }),
        TransactionState::Finalised => Some(Finalise::Finalised {
            started_unix,
            finished_unix: t
                .unknown
                .get(FINALISE_KEY)
                .and_then(|v| v["finished_unix"].as_u64())
                .unwrap_or(0),
        }),
        _ => None,
    }
}

/// Whether a published, still-Undoable MAME replacement may be finalised now.
fn finalise_preconditions(
    txs: &[RenameTransaction],
    t: &RenameTransaction,
    e: &crate::dat::rename_apply::model::TransactionEntry,
    original: &ObjectIdentity,
) -> Result<(), &'static str> {
    if original.freshness.is_none() || e.identity.freshness.is_none() {
        return Err("original or published output has no content proof");
    }
    if !capture_identity(&e.destination_path).is_ok_and(|i| identity_matches(&e.identity, &i)) {
        return Err("the published output at the destination no longer matches the journal");
    }
    if matches!(finalise_marker(t), Some(Finalise::Finalised { .. })) {
        return Err("already finalised");
    }
    if txs.iter().any(|o| {
        o.transaction_id != t.transaction_id
            && (o.state.needs_recovery()
                || o.entries.iter().any(|x| {
                    matches!(
                        x.state,
                        EntryState::Applying | EntryState::RollingBack | EntryState::RollbackFailed
                    )
                }))
    }) {
        return Err("another transaction needs Recovery first");
    }
    Ok(())
}

fn classify(
    lock: &StagingLock,
    txs: &[RenameTransaction],
    path: PathBuf,
) -> Result<StagingItem, String> {
    let owners: Vec<_> = txs.iter().filter(|t| references(t, &path)).collect();
    let mut item = StagingItem {
        retention_reason: unowned_reason(&path).into(),
        path,
        transaction_id: None,
        transaction_state: None,
        cleanup_eligible: false,
        finalise_eligible: false,
        identity: None,
    };
    if owners.is_empty()
        && item
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(".retiring-"))
    {
        item.retention_reason = "Set aside by an interrupted or refused cleanup and not removed; review manually; never cleaned automatically".into();
        return Ok(item);
    }
    if owners.len() != 1 {
        if !owners.is_empty() {
            item.retention_reason = "Ambiguous journal references; retained".into();
        }
        return Ok(item);
    }
    let t = owners[0];
    item.transaction_id = Some(t.transaction_id.clone());
    item.transaction_state = Some(t.state);
    item.retention_reason = "Unresolved recovery or unsupported ownership; retained".into();
    if extra_references(t, &item.path) {
        item.retention_reason = "Ambiguous additional journal reference; retained".into();
        return Ok(item);
    }
    if t.unknown.get("workflow").and_then(|v| v.as_str()) != Some(MAME_RECONSTRUCTION_WORKFLOW)
        || t.entries.len() != 1
        || Path::new(&t.source_scan_root) != lock.staging.path
    {
        return Ok(item);
    }
    let e = &t.entries[0];
    if e.source_path != item.path || item.path.parent() != Some(lock.staging.path.as_path()) {
        return Ok(item);
    }
    let marker = finalise_marker(t);
    if let Some(Finalise::Finalised { .. }) = marker {
        item.retention_reason = if capture_identity(&item.path).is_ok() {
            "Replacement finalised, but a file is still present at the original's path; Recovery must inspect"
        } else {
            "Replacement finalised: the original was deleted after explicit confirmation; Undo is no longer available; history retained"
        }
        .into();
        return Ok(item);
    }
    let actual = match capture_identity(&item.path) {
        Ok(i) => i,
        Err(_) if marker.is_some() => {
            item.retention_reason = "Finalisation was interrupted and the original is no longer at its path; complete the interrupted finalisation to confirm it; Undo is unavailable".into();
            return Ok(item);
        }
        Err(_) => {
            item.retention_reason = if t.state == TransactionState::RolledBack
                && e.state == EntryState::RolledBack
            {
                "Completed Undo output absent or unreadable; no cleanup possible; history retained (an interrupted cleanup may have left it under a .retiring- name)"
                    .into()
            } else {
                "Missing or unreadable staged file; Recovery must inspect".into()
            };
            return Ok(item);
        }
    };
    item.identity = Some(actual.clone());
    if matches!(
        t.state,
        TransactionState::Applied | TransactionState::Finalising
    ) && e.state == EntryState::Applied
    {
        item.retention_reason = "Active exact-byte Undo original; must remain".into();
        if let TransactionOperation::ReplaceExisting {
            original_identity, ..
        } = &e.operation
        {
            if !identity_matches(original_identity, &actual) {
                item.retention_reason = "Undo original changed; Recovery must inspect".into();
            } else if !crate::dat::rename_apply::seal::verify(
                &lock.journals.path,
                &t.transaction_id,
            )
            .trusted()
            {
                item.retention_reason = "Active exact-byte Undo original; must remain (the journal's seal is missing or does not match, so it cannot be finalised)".into();
            } else if let Err(why) = finalise_preconditions(txs, t, e, original_identity) {
                item.retention_reason =
                    format!("Active exact-byte Undo original; must remain ({why})");
            } else {
                item.finalise_eligible = true;
                item.retention_reason = if marker.is_some() {
                    "Interrupted finalisation: the Undo original is still present; resume or cancel it (Undo is unavailable meanwhile)".into()
                } else {
                    "Active exact-byte Undo original; retained until Finalise Replacement is separately confirmed".into()
                };
            }
        }
        return Ok(item);
    }
    if t.state != TransactionState::RolledBack || e.state != EntryState::RolledBack {
        return Ok(item);
    }
    item.retention_reason = "Completed transaction artifact, but ownership/content proof is missing or changed; retained".into();
    if e.identity.freshness.is_none() || !identity_matches(&e.identity, &actual) {
        return Ok(item);
    }
    // Independent exclusive-creation diagnostic, written before Apply, binds
    // this inode/content to this transaction. A journal alone is insufficient.
    let Some(leaf) = item.path.file_name().and_then(|s| s.to_str()) else {
        return Ok(item);
    };
    let mut noted = false;
    for n in 0..64 {
        let Ok(v) = lock.staging.read_json(&format!("{leaf}.note-{n}.json")) else {
            continue;
        };
        if v["schema"] != 1
            || v["staged_path"].as_str() != item.path.to_str()
            || v["detail"]["state"] != "journal_reserved"
            || v["detail"]["transaction_id"].as_str() != Some(t.transaction_id.as_str())
        {
            continue;
        }
        if let Ok(i) = serde_json::from_value::<ObjectIdentity>(v["identity"].clone()) {
            if i.freshness.is_some() && identity_matches(&i, &actual) {
                noted = true;
            }
        }
    }
    if !noted {
        return Ok(item);
    }
    match &e.operation {
        TransactionOperation::ReplaceExisting {
            original_identity, ..
        } => {
            if original_identity.freshness.is_none()
                || !capture_identity(&e.destination_path)
                    .is_ok_and(|i| identity_matches(original_identity, &i))
            {
                item.retention_reason =
                    "Undo restoration no longer independently verified; retained".into();
                return Ok(item);
            }
        }
        TransactionOperation::RenameMove => {
            if std::fs::symlink_metadata(&e.destination_path).is_ok() {
                return Ok(item);
            }
        }
        _ => return Ok(item),
    }
    if txs.iter().any(|t| {
        t.state.needs_recovery()
            || t.entries.iter().any(|e| {
                matches!(
                    e.state,
                    EntryState::Applying | EntryState::RollingBack | EntryState::RollbackFailed
                )
            })
    }) {
        item.retention_reason =
            "Recovery remains unresolved (acknowledgement does not settle it); retained".into();
        return Ok(item);
    }
    item.cleanup_eligible = true;
    item.retention_reason = "Superseded reconstructed output after verified completed Undo; publication reversed; cleanup preserves Undo history".into();
    Ok(item)
}

/// Delete only the exact reviewed redundant output, with explicit confirmation.
/// The caller must separately attest exclusive namespace access, after stopping
/// other staging/journal writers and manual file operations. Linux has no
/// inode-conditional unlink; flock and a file lease cannot enforce this against
/// an uncooperative namespace writer. Without that prerequisite cleanup refuses.
/// No mode exists for deleting originals/surrendering Undo. Journals/diagnostics
/// are retained as history. This never follows a staged leaf symlink.
pub fn cleanup(
    staging: &Path,
    journals: &Path,
    reviewed: &StagingItem,
    confirmation: &str,
    exclusive_access: &str,
) -> Result<(), String> {
    if exclusive_access != "I HAVE EXCLUSIVE STAGING AND JOURNAL ACCESS" {
        return Err("cleanup requires separate confirmation: stop all other writers to staging and journals, including manual file operations; flock/leases cannot exclude an uncooperative namespace writer".into());
    }
    if confirmation != "DELETE REDUNDANT MAME OUTPUT" || !reviewed.cleanup_eligible {
        return Err("explicit redundant-output cleanup confirmation required; Undo originals cannot be cleaned".into());
    }
    let lock = StagingLock::acquire(staging, journals)?;
    let txs = lock.transactions()?;
    let current = classify(&lock, &txs, reviewed.path.clone())?;
    if &current != reviewed || !current.cleanup_eligible {
        return Err(
            "staging identity, retention evidence or transaction changed; cleanup refused".into(),
        );
    }
    retire_exact(&lock, reviewed)
}

/// The proven deletion boundary shared by `cleanup` and `finalise_replacement`:
/// open without following links, re-prove identity, hold an exclusive lease,
/// set the entry aside under a private name, re-prove it there and only then
/// unlink. The caller has already decided that this exact item may go.
fn retire_exact(lock: &StagingLock, reviewed: &StagingItem) -> Result<(), String> {
    let leaf = reviewed
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid staged basename")?;
    let name = std::ffi::CString::new(leaf).map_err(|e| e.to_string())?;
    // O_NOFOLLOW/O_NONBLOCK prevent opening substituted symlinks/special files.
    use std::os::fd::FromRawFd;
    let fd = unsafe {
        libc::openat(
            lock.staging.file.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("staged file is not regular".into());
    }
    lock.staging.require_file_binding(leaf, &file)?;
    // Re-read all references and proofs before acquiring the exclusive lease.
    super::owned_attempt::event("before_cleanup", &reviewed.path)?;
    let txs = lock.transactions()?;
    if classify(lock, &txs, reviewed.path.clone())? != *reviewed {
        return Err("file or journal changed; cleanup refused".into());
    }
    super::owned_attempt::event("before_cleanup_lease", &reviewed.path)?;
    // A write lease refuses all other open readers/writers. Do not reopen the
    // staged file while leased (that would request a break of our own lease).
    // SIGURG defaults to ignore; a break is detected with F_GETLEASE below.
    if unsafe {
        libc::fcntl(
            fd,
            10, /* Linux F_SETSIG (not exported by libc) */
            libc::SIGURG,
        )
    } < 0
    {
        return Err("exclusive file leases are unavailable; cleanup refused".into());
    }
    // A write lease needs every other open file description of the inode to
    // be gone. A concurrent fork() elsewhere in this process briefly keeps an
    // earlier, already-closed EmuWiz descriptor alive until the child execs,
    // so a short bounded retry is allowed. Anything still open after it
    // refuses, exactly as before.
    let mut leased = false;
    for attempt in 0..20 {
        if unsafe { libc::fcntl(fd, libc::F_SETLEASE, libc::F_WRLCK) } == 0 {
            leased = true;
            break;
        }
        let busy = matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EAGAIN)
        );
        if !busy || attempt == 19 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !leased {
        return Err("another process may be using the file, or exclusive file leases are unavailable; cleanup refused".into());
    }
    // The lease belongs to the open file description, which a concurrent
    // fork() elsewhere in the process can briefly duplicate until its exec.
    // Release it explicitly on every exit so a refused cleanup never leaves
    // the kept file leased (F_UNLCK acts on the shared description).
    struct ReleaseLease(i32);
    impl Drop for ReleaseLease {
        fn drop(&mut self) {
            // SAFETY: the descriptor stays open until after this guard drops.
            unsafe {
                libc::fcntl(self.0, libc::F_SETLEASE, libc::F_UNLCK);
            }
        }
    }
    let _lease = ReleaseLease(fd);
    if lock.transactions()? != txs {
        return Err("journal references changed; cleanup refused".into());
    }
    lock.staging.require_binding()?;
    lock.journals.require_binding()?;
    lock.staging.require_file_binding(leaf, &file)?;
    super::owned_attempt::event("cleanup_leased", &reviewed.path)?;
    let held = file.metadata().map_err(|e| e.to_string())?;
    let identity = reviewed
        .identity
        .as_ref()
        .ok_or("missing reviewed identity")?;
    if (held.dev(), held.ino()) != (identity.dev, identity.ino) {
        return Err("file identity changed; cleanup refused".into());
    }
    // Rehash through the held descriptor after acquiring the lease: a writer
    // could have changed and closed this inode between classify and F_SETLEASE.
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::FileExt;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut offset = 0u64;
    loop {
        let limit = buffer
            .len()
            .min(identity.size_bytes.saturating_add(1).saturating_sub(offset) as usize);
        if limit == 0 {
            break;
        }
        let n = file
            .read_at(&mut buffer[..limit], offset)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        offset += n as u64;
    }
    let after = file.metadata().map_err(|e| e.to_string())?;
    if offset != identity.size_bytes
        || after.len() != identity.size_bytes
        || identity.freshness.as_ref().is_none_or(|f| {
            after.modified().ok() != Some(f.modified)
                || <[u8; 32]>::from(hash.finalize()) != f.sha256
        })
    {
        return Err("file content changed before exclusive lease; cleanup refused".into());
    }
    if unsafe { libc::fcntl(fd, libc::F_GETLEASE) } != libc::F_WRLCK {
        return Err("exclusive lease changed; cleanup refused".into());
    }
    super::owned_attempt::event("before_cleanup_unlink", &reviewed.path)?;
    if lock.transactions()? != txs {
        return Err("journal changed before deletion; cleanup refused".into());
    }
    lock.staging.require_binding()?;
    lock.journals.require_binding()?;
    lock.staging.require_file_binding(leaf, &file)?;
    if unsafe { libc::fcntl(fd, libc::F_GETLEASE) } != libc::F_WRLCK {
        return Err("another process requested file access; cleanup refused".into());
    }
    // Linux has no inode-conditional unlink and a lease is not broken by a
    // rename of the directory entry. So the entry is first moved, no-replace,
    // to a fresh private name in the same pinned directory and re-proven
    // there; only that private name is ever unlinked. A substituted entry is
    // never deleted. This narrows, but cannot remove, the exposure to an
    // uncooperative writer; the exclusive-access attestation still applies.
    let holding = format!(".retiring-{}", super::owned_attempt::token()?);
    let holding_name = std::ffi::CString::new(holding.as_str()).map_err(|e| e.to_string())?;
    let dir = lock.staging.file.as_raw_fd();
    let rename = |from: &std::ffi::CStr, to: &std::ffi::CStr| {
        // SAFETY: single components inside the same retained directory fd.
        let rc = unsafe {
            libc::renameat2(dir, from.as_ptr(), dir, to.as_ptr(), libc::RENAME_NOREPLACE)
        };
        rc == 0
    };
    super::owned_attempt::event("before_cleanup_set_aside", &reviewed.path)?;
    if !rename(&name, &holding_name) {
        return Err(format!(
            "the staged file could not be set aside for removal ({}); nothing was removed",
            std::io::Error::last_os_error()
        ));
    }
    super::owned_attempt::event("cleanup_set_aside", &lock.staging.path.join(&holding))?;
    if let Err(error) = lock.staging.require_file_binding(&holding, &file) {
        // What now carries the private name is not the proven file: an
        // uncooperative writer swapped an entry around the rename. Whatever it
        // is was moved to the private name but is never removed, and it is not
        // moved again (a rename back could relocate a foreign object twice).
        return Err(format!(
            "the entry set aside as {} is not the reviewed file ({error}); it was moved there but nothing was removed; inspect it and {}",
            lock.staging.path.join(&holding).display(),
            reviewed.path.display()
        ));
    }
    // Only the private name's binding authorises a rename-back or an unlink.
    // If it no longer binds the proven inode, nothing is moved or removed.
    let not_ours = |error: String| {
        format!(
            "the entry set aside as {} is no longer the reviewed file ({error}); nothing was removed and nothing was moved back; inspect it and {}",
            lock.staging.path.join(&holding).display(),
            reviewed.path.display()
        )
    };
    let unchanged = lock.transactions().map(|now| now == txs).unwrap_or(false)
        && unsafe { libc::fcntl(fd, libc::F_GETLEASE) } == libc::F_WRLCK
        && lock.staging.require_binding().is_ok()
        && lock.journals.require_binding().is_ok();
    super::owned_attempt::event(
        "before_cleanup_final_proof",
        &lock.staging.path.join(&holding),
    )?;
    if let Err(error) = lock.staging.require_file_binding(&holding, &file) {
        return Err(not_ours(error));
    }
    if !unchanged {
        // The private name was just re-proven to bind the held inode, so
        // putting it back (no-replace) cannot relocate a foreign object.
        let restored = rename(&holding_name, &name);
        return Err(if restored {
            "journal, lease or directory changed at the deletion boundary; the file was put back unchanged; cleanup refused".into()
        } else {
            format!(
                "journal, lease or directory changed at the deletion boundary; the file was kept as {} for review ({}); cleanup refused",
                lock.staging.path.join(&holding).display(),
                std::io::Error::last_os_error()
            )
        });
    }
    // SAFETY: the private name was just proven to bind the held, leased inode.
    if unsafe { libc::unlinkat(dir, holding_name.as_ptr(), 0) } != 0 {
        return Err(format!(
            "the file was set aside as {} but could not be removed: {}",
            lock.staging.path.join(&holding).display(),
            std::io::Error::last_os_error()
        ));
    }
    lock.staging
        .file
        .sync_all()
        .map_err(|e| format!("output removed but directory sync failed: {e}; inspect inventory"))
}

pub const FINALISE_CONFIRMATION: &str = "FINALISE REPLACEMENT AND DELETE ORIGINAL";
const EXCLUSIVE_ACCESS: &str = "I HAVE EXCLUSIVE STAGING AND JOURNAL ACCESS";

/// What a finished Finalise Replacement did. Returned only when the original is
/// gone and the journal records it as finalised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinaliseReport {
    pub transaction_id: String,
    pub deleted_original: PathBuf,
}

fn marker_value(phase: &str, started_unix: u64, finished_unix: Option<u64>) -> serde_json::Value {
    let mut v = serde_json::json!({ "phase": phase, "started_unix": started_unix });
    if let Some(f) = finished_unix {
        v["finished_unix"] = f.into();
    }
    v
}

/// Move one MAME journal to `state` through the sealed journal writer, which
/// itself enforces the legal Finalise transitions. Entries are never touched.
fn write_state(
    lock: &StagingLock,
    t: &RenameTransaction,
    state: TransactionState,
    audit: Option<serde_json::Value>,
) -> Result<(), String> {
    crate::dat::rename_apply::writer_gate::check_finalise()?;
    lock.journals.require_binding()?;
    let mut next = t.clone();
    next.state = state;
    match audit {
        Some(v) => {
            next.unknown.insert(FINALISE_KEY.into(), v);
        }
        None => {
            next.unknown.remove(FINALISE_KEY);
        }
    }
    let write = || crate::dat::rename_apply::journal::write_journal(&lock.journals.path, &next);
    if state == TransactionState::Applied {
        // Only the cancel paths reach this, after proving the original intact.
        crate::dat::rename_apply::journal::with_finalise_cancel(write)
    } else {
        write()
    }
    .map_err(|e| e.to_string())?;
    lock.journals.require_binding()
}

/// What the journal on disk now says, for wording an error truthfully after a
/// write that may or may not have landed.
fn recorded_state(lock: &StagingLock, id: &str) -> String {
    match lock
        .transactions()
        .ok()
        .and_then(|txs| txs.into_iter().find(|t| t.transaction_id == id))
    {
        Some(t) if t.state == TransactionState::Finalising => "the journal now records the finalisation as in progress: Undo is unavailable until it is completed or cancelled".into(),
        Some(t) if t.state == TransactionState::Finalised => "the journal records the replacement as finalised".into(),
        Some(_) => "the journal is unchanged: nothing was deleted and Undo is unchanged".into(),
        None => "the journal could not be re-read; Recovery must inspect it".into(),
    }
}

fn journal_of<'a>(txs: &'a [RenameTransaction], id: &str) -> Result<&'a RenameTransaction, String> {
    txs.iter()
        .find(|t| t.transaction_id == id)
        .ok_or_else(|| "the transaction's journal is no longer present; nothing was changed".into())
}

/// Surrender Undo for one published MAME replacement and delete its retained
/// original, after separate confirmation. The order is the safety property:
/// 1. every proof is re-read under the staging and journal locks;
/// 2. a sealed `finalising` marker is journalled *before* the original is
///    touched, so Undo is refused from then on and a crash is recognisable;
/// 3. the original goes through the same set-aside/re-prove/unlink boundary as
///    `cleanup`;
/// 4. only then is the journal marked `finalised`. It is kept as history.
///
/// `Ok` means all four happened. A refusal before step 3 removes the marker
/// again and Undo stays available; a failure after the original moved leaves
/// the marker in place and says so (see `complete_interrupted_finalise` and
/// `cancel_interrupted_finalise`).
pub fn finalise_replacement(
    staging: &Path,
    journals: &Path,
    reviewed: &StagingItem,
    confirmation: &str,
    exclusive_access: &str,
) -> Result<FinaliseReport, String> {
    if exclusive_access != EXCLUSIVE_ACCESS {
        return Err("finalising requires separate confirmation: stop all other writers to staging and journals, including manual file operations".into());
    }
    if confirmation != FINALISE_CONFIRMATION || !reviewed.finalise_eligible {
        return Err("explicit Finalise Replacement confirmation required; this permanently deletes the Undo original".into());
    }
    let id = reviewed
        .transaction_id
        .clone()
        .ok_or("reviewed item has no transaction")?;
    crate::dat::rename_apply::writer_gate::check_finalise()?;
    let lock = StagingLock::acquire(staging, journals)?;
    let txs = lock.transactions()?;
    let current = classify(&lock, &txs, reviewed.path.clone())?;
    if &current != reviewed || !current.finalise_eligible {
        return Err(
            "staging identity, retention evidence or transaction changed; finalising refused; nothing was deleted".into(),
        );
    }
    let t = journal_of(&txs, &id)?.clone();
    let resumed = t.state == TransactionState::Finalising;
    let started = match finalise_marker(&t) {
        Some(Finalise::Finalising { started_unix }) => started_unix,
        _ => crate::dat::sources::now_unix(),
    };
    super::owned_attempt::event("before_finalise_marker", &reviewed.path)?;
    if !resumed {
        write_state(
            &lock,
            &t,
            TransactionState::Finalising,
            Some(marker_value("finalising", started, None)),
        )
        .map_err(|e| {
            format!(
                "could not record the finalisation ({e}); the original was not touched, and {}",
                recorded_state(&lock, &id)
            )
        })?;
    }
    // Undo of this transaction is now refused. Re-prove against the journal as
    // it now stands before touching the original.
    let outcome = (|| {
        super::owned_attempt::event("after_finalise_marker", &reviewed.path)?;
        let txs = lock.transactions()?;
        let now = classify(&lock, &txs, reviewed.path.clone())?;
        if now.identity != reviewed.identity
            || now.transaction_id != reviewed.transaction_id
            || !now.finalise_eligible
        {
            return Err("the original or journal changed after the finalisation was recorded; nothing was deleted".to_string());
        }
        retire_exact(&lock, &now)
    })();
    if let Err(error) = outcome {
        let intact = capture_identity(&reviewed.path)
            .ok()
            .zip(reviewed.identity.as_ref())
            .is_some_and(|(a, b)| identity_matches(b, &a));
        if intact && !resumed {
            let txs = lock.transactions()?;
            return Err(
                match write_state(
                    &lock,
                    journal_of(&txs, &id)?,
                    TransactionState::Applied,
                    None,
                ) {
                    Ok(()) => format!("{error}; the original is intact and Undo remains available"),
                    Err(e) => format!(
                        "{error}; the original is intact, but the finalisation record could not be cleared ({e}); cancel the interrupted finalisation to restore Undo"
                    ),
                },
            );
        }
        return Err(format!(
            "{error}; finalisation is recorded as interrupted and Undo is unavailable until it is completed or cancelled"
        ));
    }
    super::owned_attempt::event("before_finalise_commit", &reviewed.path)?;
    let txs = lock.transactions()?;
    write_state(
        &lock,
        journal_of(&txs, &id)?,
        TransactionState::Finalised,
        Some(marker_value(
            "finalised",
            started,
            Some(crate::dat::sources::now_unix()),
        )),
    )
    .map_err(|e| {
        format!(
            "the original was deleted, but the journal could not be marked finalised ({e}); {}; complete the interrupted finalisation to settle it",
            recorded_state(&lock, &id)
        )
    })?;
    Ok(FinaliseReport {
        transaction_id: id,
        deleted_original: reviewed.path.clone(),
    })
}

fn interrupted<'a>(
    txs: &'a [RenameTransaction],
    id: &str,
) -> Result<&'a RenameTransaction, String> {
    let t = journal_of(txs, id)?;
    match finalise_marker(t) {
        Some(Finalise::Finalising { .. }) => Ok(t),
        Some(Finalise::Finalised { .. }) => Err("already finalised; nothing to resolve".into()),
        None => Err("no interrupted finalisation is recorded for this transaction".into()),
    }
}

/// The original is provably gone: record the finalisation as finished. Refuses
/// while anything remains at the original's path or any `.retiring-` entry is
/// present in staging, because that may be the original awaiting review.
pub fn complete_interrupted_finalise(
    staging: &Path,
    journals: &Path,
    transaction_id: &str,
) -> Result<FinaliseReport, String> {
    let lock = StagingLock::acquire(staging, journals)?;
    let txs = lock.transactions()?;
    let t = interrupted(&txs, transaction_id)?;
    let Some(Finalise::Finalising { started_unix }) = finalise_marker(t) else {
        return Err("no interrupted finalisation".into());
    };
    let path = t
        .entries
        .first()
        .ok_or("journal has no entry")?
        .source_path
        .clone();
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(
                "the original is still present; resume the finalisation or cancel it instead"
                    .into(),
            );
        }
        Err(e) => {
            return Err(format!(
                "the original's path cannot be inspected ({e}); nothing was changed"
            ));
        }
    }
    for e in std::fs::read_dir(&lock.staging.path).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        if e.file_name().to_string_lossy().starts_with(".retiring-") {
            return Err(format!(
                "{} was set aside during removal and may be the original; review it manually before completing",
                e.path().display()
            ));
        }
    }
    lock.staging.require_binding()?;
    write_state(
        &lock,
        t,
        TransactionState::Finalised,
        Some(marker_value(
            "finalised",
            started_unix,
            Some(crate::dat::sources::now_unix()),
        )),
    )?;
    Ok(FinaliseReport {
        transaction_id: transaction_id.into(),
        deleted_original: path,
    })
}

/// The original is still present and proven: drop the marker so Undo works
/// again. Nothing on disk besides the journal changes.
pub fn cancel_interrupted_finalise(
    staging: &Path,
    journals: &Path,
    transaction_id: &str,
) -> Result<(), String> {
    let lock = StagingLock::acquire(staging, journals)?;
    let txs = lock.transactions()?;
    let t = interrupted(&txs, transaction_id)?;
    let path = t
        .entries
        .first()
        .ok_or("journal has no entry")?
        .source_path
        .clone();
    let item = classify(&lock, &txs, path)?;
    if !item.finalise_eligible {
        return Err(format!(
            "the original is not in its proven place ({}); cancelling would not make Undo safe; nothing was changed",
            item.retention_reason
        ));
    }
    write_state(&lock, t, TransactionState::Applied, None)
}

/// MAME Undo shares publication/cleanup locks and rejects stale caller records.
/// Other rename and R2 workflows do not use this hook.
pub(crate) fn lock_undo(
    t: &RenameTransaction,
    journals: &Path,
) -> Result<Option<StagingLock>, String> {
    if t.unknown.get("workflow").and_then(|v| v.as_str()) != Some(MAME_RECONSTRUCTION_WORKFLOW) {
        return Ok(None);
    }
    let lock = StagingLock::acquire(Path::new(&t.source_scan_root), journals)?;
    let txs = lock.transactions()?;
    let current = txs.iter().find(|v| v.transaction_id == t.transaction_id);
    // Judge the journal as it is on disk, not the caller's possibly stale copy.
    match current.and_then(finalise_marker) {
        Some(Finalise::Finalised { .. }) => {
            return Err("this replacement was finalised: its original was deleted and Undo is no longer available".into());
        }
        Some(Finalise::Finalising { .. }) => {
            return Err("a Finalise Replacement was interrupted: Undo is unavailable until it is completed or cancelled in the MAME staging view".into());
        }
        None => {}
    }
    if current != Some(t) {
        return Err("MAME Undo journal changed; reload history before retrying".into());
    }
    Ok(Some(lock))
}
