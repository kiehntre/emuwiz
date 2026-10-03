//! Domain-neutral, reviewed regular-file replacement. The existing journal's
//! source path holds the original after exchange; no second backup/journal.
use std::path::{Component, Path};

use super::identity::{capture_identity, identity_matches};
use super::model::{
    EntryState, ObjectIdentity, ObjectKind, TransactionEntry, TransactionOperation,
};

fn safe_regular(path: &Path) -> Result<ObjectIdentity, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
    {
        return Err("replacement requires an absolute path without traversal".into());
    }
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("replacement refuses symlink ancestors/members".into());
        }
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err("replacement refuses hardlinked files".into());
        }
    }
    if !metadata.is_file() {
        return Err("replacement requires regular files".into());
    }
    capture_identity(path).map_err(|e| e.to_string())
}

/// True: replacement published, exact original at staging. False: original
/// at target, replacement at staging. Any other state is refused, not guessed.
pub(super) fn published(entry: &TransactionEntry) -> Result<bool, String> {
    let TransactionOperation::ReplaceExisting {
        original_identity,
        destination_root,
    } = &entry.operation
    else {
        return Err("not a replacement operation".into());
    };
    if entry.source_path == entry.destination_path
        || !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
        || original_identity.kind != ObjectKind::RegularFile
        || entry.identity.kind != ObjectKind::RegularFile
        || original_identity.freshness.is_none()
        || entry.identity.freshness.is_none()
    {
        return Err("invalid replacement authority/evidence".into());
    }
    let source = safe_regular(&entry.source_path)?;
    let target = safe_regular(&entry.destination_path)?;
    if identity_matches(&entry.identity, &source) && identity_matches(original_identity, &target) {
        Ok(false)
    } else if identity_matches(original_identity, &source)
        && identity_matches(&entry.identity, &target)
    {
        Ok(true)
    } else {
        Err(
            "replacement source, preserved original or target changed; manual review required"
                .into(),
        )
    }
}

fn sync_files(entry: &TransactionEntry) -> Result<(), String> {
    for path in [&entry.source_path, &entry.destination_path] {
        std::fs::File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn sync_parents(entry: &TransactionEntry) -> Result<(), String> {
    for path in [&entry.source_path, &entry.destination_path] {
        std::fs::File::open(path.parent().ok_or("missing replacement parent")?)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn apply(entry: &TransactionEntry) -> Result<(), (EntryState, String)> {
    let before = |e| (EntryState::ApplyFailed, e);
    if published(entry).map_err(before)? {
        return Err(before("replacement already published".into()));
    }
    sync_files(entry).map_err(before)?;
    if published(entry).map_err(before)? {
        return Err(before("replacement changed before exchange".into()));
    }
    fault("before_exchange").map_err(before)?;
    super::noclobber::exchange(&entry.source_path, &entry.destination_path)
        .map_err(|e| before(e.to_string()))?;
    // Keep Applying on post-syscall failure so existing restart/undo reconciliation
    // can classify both objects. Never report an uncertain publication as success.
    let after = |e| (EntryState::Applying, e);
    fault("after_exchange").map_err(after)?;
    sync_parents(entry).map_err(after)?;
    if !published(entry).map_err(after)? {
        return Err(after("replacement exchange not confirmed".into()));
    }
    Ok(())
}

pub(super) fn undo(entry: &TransactionEntry) -> Result<(), String> {
    if !published(entry)? {
        return Err("replacement is not currently published".into());
    }
    sync_files(entry)?;
    if !published(entry)? {
        return Err("replacement changed before undo".into());
    }
    super::noclobber::exchange(&entry.source_path, &entry.destination_path)
        .map_err(|e| e.to_string())?;
    sync_parents(entry)?;
    if published(entry)? {
        return Err("replacement undo not confirmed".into());
    }
    Ok(())
}

#[cfg(test)]
thread_local! { static FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }
fn fault(_point: &str) -> Result<(), String> {
    #[cfg(test)]
    if FAULT.with(|f| f.get() == Some(_point)) {
        return Err(format!("injected replacement failure: {_point}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
