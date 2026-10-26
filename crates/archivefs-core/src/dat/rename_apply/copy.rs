//! Shared copy staging. Publication/Undo use the existing descriptor-bound
//! no-clobber rename. Copies are never written through a collection pathname.
//! Undo retains the created copy in its private staging slot; it never unlinks
//! an entry that another writer could replace after verification.
use super::{
    dirsync::RenameDirs,
    identity::{capture_identity, content_identity_matches, identity_matches},
    model::{EntryState, ObjectIdentity, TransactionEntry, TransactionOperation},
};
use crate::safe_read::TrustedRoots;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
const KEY: &str = "created_copy_v1";
#[derive(Serialize, Deserialize)]
struct Receipt {
    staging: PathBuf,
    directory_device: u64,
    directory_inode: u64,
    identity: ObjectIdentity,
}

pub(crate) struct Prepared {
    dirs: RenameDirs,
    _file: File,
    identity: ObjectIdentity,
}

#[cfg(target_os = "linux")]
pub(crate) fn prepare(entry: &mut TransactionEntry) -> Result<Option<Prepared>, String> {
    use std::os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    };
    let TransactionOperation::CreateCopy {
        expected_source,
        destination_root,
    } = &entry.operation
    else {
        return Ok(None);
    };
    if expected_source != &entry.source_path
        || !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
        || entry.unknown.contains_key(KEY)
    {
        return Err("copy authority is invalid or already staged; review recovery".into());
    }
    let roots = TrustedRoots::from_paths([destination_root]);
    let bound = RenameDirs::open_in(&entry.source_path, &entry.destination_path, &roots)
        .map_err(|e| e.to_string())?;
    let source_path = bound.source_entry();
    let source = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&source_path)
        .map_err(|e| e.to_string())?;
    if !source.metadata().map_err(|e| e.to_string())?.is_file()
        || !identity_matches(
            &entry.identity,
            &capture_identity(&source_path).map_err(|e| e.to_string())?,
        )
    {
        return Err("copy source changed since review".into());
    }
    // The descriptor path is for our OWN retained open file, not a user path.
    let pinned_source = PathBuf::from(format!("/proc/self/fd/{}", source.as_raw_fd()));
    let mut input = File::open(pinned_source).map_err(|e| e.to_string())?;
    let pinned_parent = bound.destination_entry().parent().unwrap().to_path_buf();
    let staging_dir = tempfile::Builder::new()
        .prefix(".emuwiz-copy-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(&pinned_parent)
        .map_err(|e| e.to_string())?;
    // Keep failed/in-flight data for recovery, never auto-delete a substituted
    // pathname from a destructor. The directory is exclusive and mode 0700.
    let retained = staging_dir.keep();
    let actual_dir = entry
        .destination_path
        .parent()
        .unwrap()
        .join(retained.file_name().unwrap());
    let meta = fs::symlink_metadata(&retained).map_err(|e| e.to_string())?;
    let staging = actual_dir.join("copy");
    let staged_bound = RenameDirs::open_in(&staging, &entry.destination_path, &roots)
        .map_err(|e| e.to_string())?;
    edge(Edge::BeforeCreate, &staged_bound.source_entry()).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .mode(0o600)
        .open(staged_bound.source_entry())
        .map_err(|e| e.to_string())?;
    let copied = io::copy(
        &mut Read::by_ref(&mut input).take(entry.identity.size_bytes.saturating_add(1)),
        &mut file,
    )
    .map_err(|e| e.to_string())?;
    file.flush()
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    edge(Edge::BeforeCapture, &staged_bound.source_entry()).map_err(|e| e.to_string())?;
    let identity = capture_identity(&staged_bound.source_entry()).map_err(|e| e.to_string())?;
    let held = file.metadata().map_err(|e| e.to_string())?;
    if held.dev() != identity.dev || held.ino() != identity.ino {
        return Err("staged pathname no longer names the held copy".into());
    }
    if copied != entry.identity.size_bytes
        || !content_identity_matches(&entry.identity, &identity)
        || !identity_matches(
            &entry.identity,
            &capture_identity(&source_path).map_err(|e| e.to_string())?,
        )
    {
        return Err(format!(
            "copy staging verification failed; retained at {}",
            staging.display()
        ));
    }
    bound
        .verify_bound()
        .and_then(|_| staged_bound.sync_and_verify(false))
        .map_err(|e| e.to_string())?;
    let receipt = Receipt {
        staging,
        directory_device: meta.dev(),
        directory_inode: meta.ino(),
        identity: identity.clone(),
    };
    verify_directory(&receipt, destination_root)?;
    entry.unknown.insert(
        KEY.into(),
        serde_json::to_value(receipt).map_err(|e| e.to_string())?,
    );
    Ok(Some(Prepared {
        dirs: staged_bound,
        _file: file,
        identity,
    }))
}
#[cfg(not(target_os = "linux"))]
pub(crate) fn prepare(entry: &mut TransactionEntry) -> Result<Option<Prepared>, String> {
    if matches!(entry.operation, TransactionOperation::CreateCopy { .. }) {
        Err("descriptor-bound copy requires Linux".into())
    } else {
        Ok(None)
    }
}

impl Prepared {
    pub(crate) fn publish(self) -> Result<(), (EntryState, String)> {
        let fail = |e: String| (EntryState::Applying, e);
        edge(Edge::BeforePublish, &self.dirs.source_entry()).map_err(|e| fail(e.to_string()))?;
        self.dirs.verify_bound().map_err(|e| fail(e.to_string()))?;
        if !identity_matches(
            &self.identity,
            &capture_identity(&self.dirs.source_entry()).map_err(|e| fail(e.to_string()))?,
        ) {
            return Err(fail(
                "staged copy identity changed; retained for recovery".into(),
            ));
        }
        self.dirs
            .rename_noreplace(false)
            .map_err(|e| fail(e.to_string()))?;
        edge(Edge::AfterPublish, &self.dirs.destination_entry())
            .map_err(|e| fail(e.to_string()))?;
        self.dirs
            .sync_and_verify(false)
            .map_err(|e| fail(e.to_string()))?;
        if !identity_matches(
            &self.identity,
            &capture_identity(&self.dirs.destination_entry()).map_err(|e| fail(e.to_string()))?,
        ) {
            return Err(fail(
                "published copy identity changed; review recovery".into(),
            ));
        }
        Ok(())
    }
}
fn receipt(entry: &TransactionEntry) -> Result<Receipt, String> {
    serde_json::from_value(
        entry
            .unknown
            .get(KEY)
            .cloned()
            .ok_or("copy has no created-object receipt; manual recovery required")?,
    )
    .map_err(|e| e.to_string())
}
fn verify_directory(receipt: &Receipt, root: &Path) -> Result<(), String> {
    if !receipt.staging.starts_with(root) || receipt.staging.file_name().is_none_or(|n| n != "copy")
    {
        return Err("copy staging authority invalid".into());
    }
    let parent = receipt
        .staging
        .parent()
        .ok_or("missing staging directory")?;
    if !parent
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with(".emuwiz-copy-"))
    {
        return Err("copy staging directory invalid".into());
    }
    let meta = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !meta.is_dir()
            || meta.dev() != receipt.directory_device
            || meta.ino() != receipt.directory_inode
            || meta.mode() & 0o077 != 0
        {
            return Err("private copy directory changed".into());
        }
    }
    Ok(())
}
/// Identity proof for History, Undo and interrupted Apply reconciliation.
pub fn destination_matches(entry: &TransactionEntry, actual: &ObjectIdentity) -> bool {
    receipt(entry).is_ok_and(|r| identity_matches(&r.identity, actual))
}
pub(crate) fn undo(entry: &TransactionEntry, root: &Path) -> Result<(), String> {
    let receipt = receipt(entry)?;
    verify_directory(&receipt, root)?;
    let roots = TrustedRoots::from_paths([root]);
    let dirs = RenameDirs::open_in(&entry.destination_path, &receipt.staging, &roots)
        .map_err(|e| e.to_string())?;
    if !identity_matches(
        &receipt.identity,
        &capture_identity(&dirs.source_entry()).map_err(|e| e.to_string())?,
    ) {
        return Err("copied destination identity changed; Undo refused".into());
    }
    dirs.rename_noreplace(false).map_err(|e| e.to_string())?;
    let in_flight = |reason: String| format!("{}{reason}", super::rollback::IN_FLIGHT_MARK);
    edge(Edge::AfterUndoMove, &dirs.destination_entry()).map_err(|e| in_flight(e.to_string()))?;
    dirs.sync_and_verify(false)
        .map_err(|e| in_flight(e.to_string()))?;
    if !identity_matches(
        &receipt.identity,
        &capture_identity(&dirs.destination_entry()).map_err(|e| in_flight(e.to_string()))?,
    ) {
        // Never delete a captured replacement. A no-clobber reversal is the
        // smallest safe correction; if it fails both names require review.
        let reversal = dirs
            .rename_noreplace(true)
            .map_err(|e| e.to_string())
            .and_then(|_| dirs.sync_and_verify(true).map_err(|e| e.to_string()));
        return Err(in_flight(format!(
            "Undo captured a changed object; restoration result {reversal:?}; review retained staging"
        )));
    }
    Ok(())
}

/// Both possible copy slots are bound to the recorded private directory.
pub(crate) fn recovery_binding(entry: &TransactionEntry) -> Result<RenameDirs, String> {
    let TransactionOperation::CreateCopy {
        destination_root, ..
    } = &entry.operation
    else {
        return Err("not a copy".into());
    };
    let receipt = receipt(entry)?;
    verify_directory(&receipt, destination_root)?;
    RenameDirs::open_in(
        &receipt.staging,
        &entry.destination_path,
        &TrustedRoots::from_paths([destination_root]),
    )
    .map_err(|e| e.to_string())
}
pub(crate) fn classify(entry: &TransactionEntry, dirs: &RenameDirs) -> Result<bool, String> {
    let receipt = receipt(entry)?;
    let source = capture_identity(&entry.source_path).map_err(|e| e.to_string())?;
    if !identity_matches(&entry.identity, &source) {
        return Err("copy source changed".into());
    }
    let at_stage = capture_identity(&dirs.source_entry());
    let at_dest = capture_identity(&dirs.destination_entry());
    let absent = |result: &io::Result<ObjectIdentity>| {
        result
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
    };
    if at_dest
        .as_ref()
        .is_ok_and(|i| identity_matches(&receipt.identity, i))
        && absent(&at_stage)
    {
        Ok(true)
    } else if at_stage
        .as_ref()
        .is_ok_and(|i| identity_matches(&receipt.identity, i))
        && absent(&at_dest)
    {
        Ok(false)
    } else {
        Err("copy slots do not prove a unique created object; manual recovery required".into())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    BeforeCreate,
    BeforeCapture,
    BeforePublish,
    AfterPublish,
    AfterUndoMove,
}
#[cfg(not(test))]
fn edge(_: Edge, _: &Path) -> io::Result<()> {
    Ok(())
}
#[cfg(test)]
thread_local! { static HOOK: std::cell::RefCell<Option<(Edge, Box<dyn FnOnce(&Path) -> io::Result<()>>)>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
fn edge(at: Edge, path: &Path) -> io::Result<()> {
    let hook = HOOK.with(|h| {
        let mut h = h.borrow_mut();
        if h.as_ref().is_some_and(|(e, _)| *e == at) {
            h.take()
        } else {
            None
        }
    });
    if let Some((_, hook)) = hook {
        hook(path)
    } else {
        Ok(())
    }
}
#[cfg(all(test, target_os = "linux"))]
mod tests;
