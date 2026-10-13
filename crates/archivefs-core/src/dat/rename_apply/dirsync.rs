//! Directory-entry durability for completed renames.
//!
//! `renameat2` changes directory entries; the journal's own `fsync` does not
//! cover them. After a rename succeeds, the parent directory (or directories)
//! whose entries changed must be synchronised **before** a successful
//! completion is recorded. This makes the directory entry durable; it says
//! nothing about the file's *data* — content durability is a separate property
//! of whoever wrote the file, and this module does not provide or prove it.
//!
//! A failure here means the rename may already have happened. Callers must
//! not repeat the rename, must not record success, and must not describe the
//! original state as unchanged.

use std::io;
use std::path::Path;

#[cfg(test)]
pub(crate) use hook::{DirSyncEvent, set_test_hook};

/// Synchronises the parent directory of `destination`, then the parent of
/// `source` unless it is the same directory. Both are attempted even if the
/// first fails; the first error is returned.
pub(crate) fn sync_rename_parents(source: &Path, destination: &Path) -> io::Result<()> {
    let missing = |path: &Path| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("no parent directory to synchronise for {}", path.display()),
        )
    };
    let destination_parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| missing(destination))?;
    let source_parent = source
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| missing(source))?;
    let first = sync_directory(destination_parent);
    let second = if same_directory(source_parent, destination_parent) {
        Ok(())
    } else {
        sync_directory(source_parent)
    };
    first.and(second)
}

fn same_directory(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return (a.dev(), a.ino()) == (b.dev(), b.ino());
        }
    }
    false
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    #[cfg(test)]
    hook::before(dir)?;
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(dir)?;
    handle.sync_all()?;
    #[cfg(test)]
    hook::after(dir);
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory synchronisation is not available on this platform",
    ))
}

#[cfg(test)]
mod hook {
    use std::cell::RefCell;
    use std::io;
    use std::path::Path;

    /// What the synthetic fault injector observes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum DirSyncEvent<'a> {
        /// About to fsync this directory. Returning `Err` fails the sync
        /// without issuing it; panicking simulates a process death here.
        Before(&'a Path),
        /// The fsync of this directory returned success. Panicking simulates
        /// a process death between the sync and the completion checkpoint.
        After(&'a Path),
    }

    type Hook = Box<dyn FnMut(DirSyncEvent<'_>) -> io::Result<()>>;
    thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }

    pub(crate) fn set_test_hook(hook: Option<Hook>) {
        HOOK.with(|h| *h.borrow_mut() = hook);
    }
    pub(super) fn before(dir: &Path) -> io::Result<()> {
        call(DirSyncEvent::Before(dir))
    }
    pub(super) fn after(dir: &Path) {
        let _ = call(DirSyncEvent::After(dir));
    }
    fn call(event: DirSyncEvent<'_>) -> io::Result<()> {
        // Take the hook out while it runs so a panic cannot leave a borrow held.
        let taken = HOOK.with(|h| h.borrow_mut().take());
        let Some(mut hook) = taken else { return Ok(()) };
        let result = hook(event);
        HOOK.with(|h| {
            let mut slot = h.borrow_mut();
            if slot.is_none() {
                *slot = Some(hook);
            }
        });
        result
    }
}
