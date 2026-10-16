//! Manual, conservative staging retirement. No age or basename grants ownership.
//! Only a completed Undo's redundant reconstructed output is eligible. Originals
//! and pre-publication failures remain retained; this API cannot surrender Undo.
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
    /// Exact object reviewed by the caller; not inferred again at confirmation.
    pub identity: Option<ObjectIdentity>,
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
            items.push(StagingItem {
                path: e.map_err(|e| e.to_string())?.path(), transaction_id: None,
                transaction_state: None, identity: None, cleanup_eligible: false,
                retention_reason: "No recovery journal directory; unpublished/unknown files retained without ownership proof".into(),
            });
        }
        dir.require_binding()?;
        items.sort_by(|a, b| a.path.cmp(&b.path));
        return Ok(items);
    }
    let lock = StagingLock::acquire(staging, journals)?;
    inventory_locked(&lock)
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
fn classify(
    lock: &StagingLock,
    txs: &[RenameTransaction],
    path: PathBuf,
) -> Result<StagingItem, String> {
    let owners: Vec<_> = txs.iter().filter(|t| references(t, &path)).collect();
    let mut item = StagingItem {
        path,
        transaction_id: None,
        transaction_state: None,
        retention_reason: "Unknown or manually added file; never cleaned".into(),
        cleanup_eligible: false,
        identity: None,
    };
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
    let actual = match capture_identity(&item.path) {
        Ok(i) => i,
        Err(_) => {
            item.retention_reason = if t.state == TransactionState::RolledBack
                && e.state == EntryState::RolledBack
            {
                "Completed Undo output absent or unreadable; no cleanup possible; history retained"
                    .into()
            } else {
                "Missing or unreadable staged file; Recovery must inspect".into()
            };
            return Ok(item);
        }
    };
    item.identity = Some(actual.clone());
    if t.state == TransactionState::Applied && e.state == EntryState::Applied {
        item.retention_reason = "Active exact-byte Undo original; must remain".into();
        if let TransactionOperation::ReplaceExisting {
            original_identity, ..
        } = &e.operation
        {
            if !identity_matches(original_identity, &actual) {
                item.retention_reason = "Undo original changed; Recovery must inspect".into();
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
    if classify(&lock, &txs, reviewed.path.clone())? != *reviewed {
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
        || unsafe { libc::fcntl(fd, libc::F_SETLEASE, libc::F_WRLCK) } < 0
    {
        return Err("another process may be using the file, or exclusive file leases are unavailable; cleanup refused".into());
    }
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
    if unsafe { libc::unlinkat(lock.staging.file.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    lock.staging
        .file
        .sync_all()
        .map_err(|e| format!("output removed but directory sync failed: {e}; inspect inventory"))
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
    if txs.iter().find(|v| v.transaction_id == t.transaction_id) != Some(t) {
        return Err("MAME Undo journal changed; reload history before retrying".into());
    }
    Ok(Some(lock))
}
