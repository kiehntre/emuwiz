//! Contract 1-I experiment. The caller owns admission and the authoritative
//! checkpoint. Private holding entries are trusted; library names are not.
use super::*;
use crate::dat::rename_apply::{EntryState, RenameTransaction, TransactionState};
use serde::{Deserialize, Serialize};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;

pub(crate) const KEY: &str = "r2_fd_runtime_v1";
pub(crate) fn enrolled(entry: &crate::dat::rename_apply::TransactionEntry) -> bool {
    entry.unknown.get("r2_fd_enrolled") == Some(&serde_json::json!(1))
}
pub(crate) fn requires_runtime(entry: &crate::dat::rename_apply::TransactionEntry) -> bool {
    enrolled(entry)
        && (matches!(
            entry.state,
            EntryState::RollingBack | EntryState::RolledBack | EntryState::RollbackFailed
        ) || (entry.unknown.get("r2_fd_shared_engine_v1") == Some(&serde_json::json!(true))
            && matches!(entry.state, EntryState::Applying | EntryState::Applied)))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Phase {
    CapturePending,
    Captured,
    ForeignHeld,
    DestinationOccupied,
    CleanupPending,
    SalvagePending,
    Salvaged,
    PossiblyUnrecoverable,
    Incomplete,
    Complete,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Action {
    Apply,
    #[default]
    Undo,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Evidence {
    #[serde(default)]
    pub(crate) action: Action,
    pub(crate) phase: Phase,
    pub(crate) held_identity: Option<ObjectIdentity>,
    pub(crate) salvage_identity: Option<ObjectIdentity>,
    pub(crate) detail: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    Intent,
    BeforeCapture,
    AfterCapture,
    CapturedRecorded,
    BeforePublish,
    AfterPublish,
    CleanupRecorded,
    BeforeAnchorUnlink,
    AfterAnchorUnlink,
    BeforeUnlink,
    AfterUnlink,
    BeforeSalvage,
    AfterSalvage,
    BeforeComplete,
    CompletionRecorded,
}
#[derive(Clone, Copy, Default)]
pub(crate) enum Fault {
    #[default]
    None,
    #[cfg(test)]
    CopyAfter(usize, i32),
    #[cfg(test)]
    Sync,
}
pub(crate) fn evidence(tx: &RenameTransaction, index: usize) -> io::Result<Option<Evidence>> {
    if !enrolled(&tx.entries[index]) {
        return Ok(None);
    }
    if requires_runtime(&tx.entries[index]) && !tx.entries[index].unknown.contains_key(KEY) {
        return Err(io::Error::other(
            "FD runtime missing for Undo state; operator review required",
        ));
    }
    tx.entries[index]
        .unknown
        .get(KEY)
        .map(|v| serde_json::from_value(v.clone()).map_err(io::Error::other))
        .transpose()
}
pub(crate) fn paths(
    root: &Path,
    tx: &RenameTransaction,
    index: usize,
) -> io::Result<(PathBuf, PathBuf)> {
    // Reuse the established identifier validator; paths are derived, never
    // selected by mutable recovery evidence.
    crate::dat::rename_apply::journal_path(root, &tx.transaction_id)
        .ok_or_else(|| io::Error::other("invalid transaction identifier"))?;
    let private = root.join(".emuwiz-r2-holding").join(&tx.transaction_id);
    Ok((
        private.join(format!("{index}.held")),
        private.join(format!("{index}.salvage")),
    ))
}
fn private_dir(path: &Path, create: bool) -> io::Result<()> {
    match if create {
        DirBuilder::new().mode(0o700).create(path)
    } else {
        fs::metadata(path).map(|_| ())
    } {
        Ok(()) => File::open(path.parent().unwrap())?.sync_all()?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::other(
            "holding directory is not private and owned",
        ));
    }
    Ok(())
}
fn checkpoint(
    tx: &mut RenameTransaction,
    index: usize,
    mut value: Evidence,
    action: Action,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
) -> io::Result<()> {
    value.action = action;
    if action == Action::Apply {
        value.detail = value.detail.replace("Undo", "Apply");
    }
    let done = value.phase == Phase::Complete;
    tx.entries[index].state = if action == Action::Apply {
        if done {
            EntryState::Applied
        } else {
            EntryState::Applying
        }
    } else if done {
        EntryState::RolledBack
    } else {
        EntryState::RollingBack
    };
    if done && action == Action::Apply {
        tx.entries[index].applied_at_unix = Some(crate::dat::sources::now_unix());
    } else if done {
        tx.entries[index].rolled_back_at_unix = Some(crate::dat::sources::now_unix());
    }
    tx.entries[index].failure_reason = if done {
        None
    } else {
        Some(value.detail.clone())
    };
    tx.entries[index].unknown.insert(
        KEY.into(),
        serde_json::to_value(value).map_err(io::Error::other)?,
    );
    tx.state = if action == Action::Apply {
        if tx
            .entries
            .iter()
            .all(|e| matches!(e.state, EntryState::Applied | EntryState::Skipped))
        {
            TransactionState::Applied
        } else {
            TransactionState::Applying
        }
    } else if tx
        .entries
        .iter()
        .all(|e| matches!(e.state, EntryState::RolledBack | EntryState::Skipped))
    {
        TransactionState::RolledBack
    } else {
        TransactionState::RollingBack
    };
    persist(tx)
}
fn value(phase: Phase, held: Option<ObjectIdentity>, detail: &str) -> Evidence {
    Evidence {
        action: Action::Undo,
        phase,
        held_identity: held,
        salvage_identity: None,
        detail: detail.into(),
    }
}
fn missing(p: &Path) -> bool {
    matches!(fs::symlink_metadata(p), Err(e) if e.kind() == io::ErrorKind::NotFound)
}
fn expected_at(p: &Path, expected: &ObjectIdentity) -> bool {
    capture_identity(p).is_ok_and(|v| identity_matches(expected, &v))
}
pub(crate) fn phase_matches_entry(
    value: &Evidence,
    entry: &crate::dat::rename_apply::TransactionEntry,
) -> bool {
    let allowed = match value.action {
        Action::Apply => matches!(
            entry.state,
            EntryState::Applying | EntryState::Applied | EntryState::ApplyFailed
        ),
        Action::Undo => matches!(
            entry.state,
            EntryState::RollingBack | EntryState::RolledBack | EntryState::RollbackFailed
        ),
    };
    let terminal = match value.action {
        Action::Apply => entry.state == EntryState::Applied,
        Action::Undo => entry.state == EntryState::RolledBack,
    };
    allowed && (value.phase == Phase::Complete) == terminal
}
pub(crate) fn verify_records(tx: &RenameTransaction) -> io::Result<()> {
    if tx.entries.iter().any(enrolled)
        && (tx.entries.iter().any(|entry| !enrolled(entry))
            || (tx.state == TransactionState::RolledBack
                && tx.entries.iter().any(|entry| {
                    !matches!(entry.state, EntryState::RolledBack | EntryState::Skipped)
                })))
    {
        return Err(io::Error::other(
            "FD enrollment or terminal state contradicts entries",
        ));
    }
    for (index, entry) in tx.entries.iter().enumerate() {
        if !enrolled(entry) {
            continue;
        }
        let runtime = evidence(tx, index)?;
        if runtime.is_none()
            && entry.unknown.get(anchor::ENROLLED) == Some(&serde_json::json!(true))
        {
            let root = Path::new(&tx.source_scan_root);
            Publisher::open(
                root,
                &anchor::path(root, tx, index)?,
                &entry.source_path,
                &entry.identity,
            )
            .map_err(|e| {
                io::Error::other(format!(
                    "enrollment anchor unavailable or inconsistent; operator review required: {e}"
                ))
            })?;
        }
        if let Some(value) = runtime {
            if !phase_matches_entry(&value, entry) {
                return Err(io::Error::other("FD receipt phase contradicts completion"));
            }
            if value.phase == Phase::Complete {
                let root = Path::new(&tx.source_scan_root);
                let (held, salvage) = paths(root, tx, index)?;
                let (destination, old) = match value.action {
                    Action::Apply => (&entry.destination_path, &entry.source_path),
                    Action::Undo => (&entry.source_path, &entry.destination_path),
                };
                let pinned = Publisher::open(root, destination, old, &entry.identity)?;
                pinned.dirs.verify_bound()?;
                if !missing(old)
                    || !missing(&held)
                    || !missing(&salvage)
                    || (value.action == Action::Undo && !missing(&anchor::path(root, tx, index)?))
                    || (value.action == Action::Apply
                        && !expected_at(&anchor::path(root, tx, index)?, &entry.identity))
                {
                    return Err(io::Error::other(
                        "FD completion conflicts with current names or holding evidence; review required",
                    ));
                }
            }
        }
    }
    Ok(())
}
fn unlink_private(p: &Publisher) -> io::Result<()> {
    // The containing private directory is trusted under Contract 1-I. This
    // syscall is NOT inode-conditional and must never target a library slot.
    let entry = p.dirs.source_entry();
    let dir = File::open(entry.parent().unwrap())?;
    let name = basename(&entry)?;
    let rc = unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    dir.sync_all()
}

/// One entry of an approved Undo. This is not a public production entry point.
/// Resume requires an admission/approval check by the existing bridge BEFORE
/// invoking this function. It never obtains authority from a receipt alone.
pub(crate) fn undo_entry(
    root: &Path,
    tx: &mut RenameTransaction,
    index: usize,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
    hook: &mut impl FnMut(Edge) -> io::Result<()>,
    fault: Fault,
) -> io::Result<()> {
    operate_entry(root, tx, index, persist, hook, fault, Action::Undo)
}

pub(crate) fn apply_entry(
    root: &Path,
    tx: &mut RenameTransaction,
    index: usize,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
    hook: &mut impl FnMut(Edge) -> io::Result<()>,
) -> io::Result<()> {
    operate_entry(root, tx, index, persist, hook, Fault::None, Action::Apply)
}
fn operate_entry(
    root: &Path,
    tx: &mut RenameTransaction,
    index: usize,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
    hook: &mut impl FnMut(Edge) -> io::Result<()>,
    _fault: Fault,
    action: Action,
) -> io::Result<()> {
    let e = tx.entries[index].clone();
    if !enrolled(&e) {
        return Err(io::Error::other("explicit FD enrollment required"));
    }
    let (old, destination) = match action {
        Action::Apply => (e.source_path, e.destination_path),
        Action::Undo => (e.destination_path, e.source_path),
    };
    let expected = e.identity;
    let (holding, salvage) = paths(root, tx, index)?;
    let previous = match evidence(tx, index)? {
        Some(v) if v.action == action => Some(v),
        Some(v)
            if action == Action::Undo
                && v.action == Action::Apply
                && v.phase == Phase::Complete =>
        {
            None
        }
        Some(_) => {
            return Err(io::Error::other(
                "different interrupted FD action requires operator review",
            ));
        }
        None => None,
    };
    let anchor_path = anchor::path(root, tx, index)?;
    let enrolled_anchor = e.unknown.get(anchor::ENROLLED) == Some(&serde_json::json!(true));
    // A trusted surviving name prevents inode reuse. Legacy inode/hash/mtime
    // evidence alone cannot establish continuity after loss of a live FD.
    let anchor_pin = if enrolled_anchor && !missing(&anchor_path) {
        Some(Publisher::open(
            root,
            &anchor_path,
            &destination,
            &expected,
        )?)
    } else {
        None
    };
    if enrolled_anchor && previous.is_none() && anchor_pin.is_none() {
        return Err(io::Error::other(
            "enrollment anchor missing; operator review required",
        ));
    }
    // A device number alone does not prove same-mount link compatibility.
    let old_parent = File::open(
        old.parent()
            .ok_or_else(|| io::Error::other("missing old parent"))?,
    )?;
    let destination_parent = File::open(
        destination
            .parent()
            .ok_or_else(|| io::Error::other("missing destination parent"))?,
    )?;
    capability::same_mount(&old_parent, &destination_parent)?;
    capability::same_mount(&old_parent, &File::open(root)?)?;

    if let Some(ref v) = previous {
        if v.phase == Phase::Complete {
            if expected_at(&destination, &expected)
                && missing(&old)
                && missing(&holding)
                && missing(&salvage)
                && (action == Action::Apply || missing(&anchor_path))
            {
                return Ok(());
            }
            return Err(io::Error::other(
                "completed receipt contradicts current objects",
            ));
        }
        if !matches!(
            v.phase,
            Phase::CapturePending | Phase::Captured | Phase::CleanupPending
        ) {
            return Err(io::Error::other(
                "Held for review / Undo incomplete: operator review required",
            ));
        }
    }
    private_dir(&root.join(".emuwiz-r2-holding"), previous.is_none())?;
    private_dir(holding.parent().unwrap(), previous.is_none())?;
    if File::open(root)?.metadata()?.dev()
        != File::open(holding.parent().unwrap())?.metadata()?.dev()
    {
        return Err(io::Error::other(
            "holding must be on the collection filesystem",
        ));
    }
    let _initial_pin;
    if previous.is_none() {
        // Verify/pin A before touching the contested old name. The descriptor
        // survives replacement; capture may still move B and must inspect it.
        _initial_pin = Some(Publisher::open(root, &old, &destination, &expected)?);
        if !missing(&destination) || !missing(&holding) || !missing(&salvage) {
            return Err(io::Error::other("occupied destination or recovery slot"));
        }
        checkpoint(
            tx,
            index,
            value(
                Phase::CapturePending,
                None,
                "Capture pending / Undo incomplete",
            ),
            action,
            persist,
        )?;
        hook(Edge::Intent)?;
    } else {
        _initial_pin = None;
    }
    if missing(&holding) {
        if previous
            .as_ref()
            .is_some_and(|v| v.phase != Phase::CapturePending || anchor_pin.is_none())
        {
            // A lost descriptor is never reconstructed from names or matching
            // hashes. Missing trusted anchor after cleanup is ambiguous.
            checkpoint(
                tx,
                index,
                value(
                    Phase::PossiblyUnrecoverable,
                    None,
                    "Original file may be unrecoverable; trusted holding entry missing / Undo incomplete",
                ),
                action,
                persist,
            )?;
            return Ok(());
        }
        let _verified = Publisher::open(root, &old, &destination, &expected)?;
        hook(Edge::BeforeCapture)?;
        let dirs = RenameDirs::open_in(&old, &holding, &TrustedRoots::from_paths([root]))?;
        dirs.rename_noreplace(false).map_err(io::Error::other)?;
        hook(Edge::AfterCapture)?;
        dirs.sync_and_verify(false)?;
    }
    let captured = capture_identity(&holding)?;
    if !identity_matches(&expected, &captured) {
        checkpoint(
            tx,
            index,
            value(
                Phase::ForeignHeld,
                Some(captured),
                "Held for review / Undo incomplete: foreign object captured from the original location; automatic return refused under G3",
            ),
            action,
            persist,
        )?;
        return Ok(());
    }
    let publisher = Publisher::open(root, &holding, &destination, &expected)?;
    if previous
        .as_ref()
        .is_none_or(|v| v.phase != Phase::CleanupPending)
    {
        checkpoint(
            tx,
            index,
            value(
                Phase::Captured,
                Some(captured.clone()),
                "Original held / Undo incomplete",
            ),
            action,
            persist,
        )?;
        hook(Edge::CapturedRecorded)?;
        if !missing(&destination) {
            // A crash after link but before its checkpoint is ambiguous. Do not
            // infer publication history from a matching hardlink alone.
            checkpoint(
                tx,
                index,
                value(
                    Phase::DestinationOccupied,
                    Some(captured),
                    "Destination occupied or publication unrecorded / Undo incomplete",
                ),
                action,
                persist,
            )?;
            return Ok(());
        }
        hook(Edge::BeforePublish)?;
        match publisher.publish(|| {})? {
            Publication::LinkedVerified => (),
            other => {
                checkpoint(
                    tx,
                    index,
                    value(
                        Phase::Incomplete,
                        Some(captured),
                        &format!(
                            "Publication requires review: {} / Undo incomplete",
                            other.detail()
                        ),
                    ),
                    action,
                    persist,
                )?;
                return Ok(());
            }
        }
        hook(Edge::AfterPublish)?;
        checkpoint(
            tx,
            index,
            value(
                Phase::CleanupPending,
                Some(captured.clone()),
                "Correct destination published; holding cleanup pending / Undo incomplete",
            ),
            action,
            persist,
        )?;
    }
    hook(Edge::CleanupRecorded)?;
    // Reopen with openat/O_NOFOLLOW and verify the actual trusted holding entry.
    let pinned = Publisher::open(root, &holding, &destination, &expected)?;
    if pinned.file.metadata()?.nlink() <= 1 || !expected_at(&destination, &expected) {
        checkpoint(
            tx,
            index,
            value(
                Phase::Incomplete,
                Some(captured),
                "Destination no longer verified; original retained in holding / Undo incomplete",
            ),
            action,
            persist,
        )?;
        return Ok(());
    }
    if action == Action::Undo
        && let Some(anchor_pin) = anchor_pin.as_ref()
    {
        hook(Edge::BeforeAnchorUnlink)?;
        unlink_private(anchor_pin)?;
        hook(Edge::AfterAnchorUnlink)?;
    }
    hook(Edge::BeforeUnlink)?;
    unlink_private(&pinned)?;
    hook(Edge::AfterUnlink)?;
    if pinned.file.metadata()?.nlink() == 0 {
        checkpoint(
            tx,
            index,
            value(
                Phase::SalvagePending,
                Some(captured.clone()),
                "Original has no observed names; best-effort salvage pending / Undo incomplete",
            ),
            action,
            persist,
        )?;
        hook(Edge::BeforeSalvage)?;
        let copy_result = (|| -> io::Result<ObjectIdentity> {
            let mut output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&salvage)?;
            let mut input = pinned
                .file
                .try_clone()?
                .take(expected.size_bytes.saturating_add(1));
            let mut buffer = [0u8; 64 * 1024];
            #[cfg(test)]
            let mut copied = 0usize;
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                #[cfg(test)]
                if let Fault::CopyAfter(limit, errno) = _fault {
                    let amount = count.min(limit.saturating_sub(copied));
                    output.write_all(&buffer[..amount])?;
                    return Err(io::Error::from_raw_os_error(errno));
                }
                output.write_all(&buffer[..count])?;
                #[cfg(test)]
                {
                    copied += count;
                }
            }
            #[cfg(test)]
            if matches!(_fault, Fault::Sync) {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            output.sync_all()?;
            File::open(salvage.parent().unwrap())?.sync_all()?;
            let copy = capture_identity(&salvage)?;
            if !crate::dat::rename_apply::identity::content_identity_matches(&expected, &copy) {
                return Err(io::Error::other(
                    "salvage bytes differ from approved content",
                ));
            }
            Ok(copy)
        })();
        let mut v = value(
            Phase::PossiblyUnrecoverable,
            Some(captured),
            "Original file may be unrecoverable; salvage incomplete / Undo incomplete",
        );
        match copy_result {
            Ok(id) => {
                v.phase = Phase::Salvaged;
                v.salvage_identity = Some(id);
                v.detail = "Salvaged bytes into a NEW inode; original may be unrecoverable / Undo incomplete".into();
            }
            Err(error) => {
                v.detail = format!(
                    "Original file may be unrecoverable; salvage failed: {error} / Undo incomplete"
                )
            }
        }
        checkpoint(tx, index, v, action, persist)?;
        hook(Edge::AfterSalvage)?;
        return Ok(());
    }
    hook(Edge::BeforeComplete)?;
    if !expected_at(&destination, &expected)
        || !missing(&old)
        || !missing(&holding)
        || !missing(&salvage)
        || (action == Action::Undo && !missing(&anchor_path))
    {
        checkpoint(
            tx,
            index,
            value(
                Phase::Incomplete,
                Some(captured),
                "Links were observed after cleanup, but current names are unresolved; original may be unrecoverable / Undo incomplete",
            ),
            action,
            persist,
        )?;
        return Ok(());
    }
    checkpoint(
        tx,
        index,
        value(
            Phase::Complete,
            None,
            if action == Action::Apply {
                "Apply completed"
            } else {
                "Undo completed"
            },
        ),
        action,
        persist,
    )?;
    hook(Edge::CompletionRecorded)?;
    verify_records(tx)
}

#[path = "fd_contract_capability.rs"]
pub(crate) mod capability;

#[path = "fd_contract_anchor.rs"]
pub(crate) mod anchor;
