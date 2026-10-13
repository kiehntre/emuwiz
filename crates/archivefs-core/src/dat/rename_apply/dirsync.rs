//! Directory-entry durability for completed renames.
//!
//! `renameat2` changes directory entries; the journal's own `fsync` does not
//! cover them. After a rename succeeds, the parent directory (or directories)
//! whose entries changed must be synchronised **before** a successful
//! completion is recorded. This makes the directory entry durable; it says
//! nothing about the file's *data* — content durability is a separate property
//! of whoever wrote the file, and this module does not provide or prove it.
//!
//! KNOWN LIMITS (documented, not fixed here; this module does not claim power-loss
//! safety):
//!   * Creating destination directories (`create_dir` in plan apply and quarantine) and
//!     removing created directories during rollback (`remove_dir`) do not fsync their
//!     parent directory, so those directory-entry changes are not proven durable.
//!   * Ancestors above a rename's parents are resolved by the kernel at `open`; the
//!     descriptor binding and the post-operation identity comparison cover the parents
//!     themselves, not a swap of a higher ancestor between the caller's confinement
//!     checks and this open.
//!   * Directory fsync says nothing about file DATA durability or device write caches.
//!
//! A failure here means the rename may already have happened. Callers must
//! not repeat the rename, must not record success, and must not describe the
//! original state as unchanged.

use std::io;
use std::path::Path;

#[cfg(test)]
pub(crate) use hook::{DirSyncEvent, set_test_hook};

/// The two parent directories of a rename, opened and identity-checked BEFORE
/// the rename and retained until completion is recorded.
///
/// Why descriptors: a path opened after the rename can resolve to a different
/// directory if an ancestor or the parent itself was replaced in between, so
/// "fsync the parent path" can succeed on the wrong directory. Here the rename
/// is issued with `renameat2(source_dirfd, name, dest_dirfd, name)`, so it acts
/// on exactly the directories that were verified, and the same descriptors are
/// the ones fsynced. `O_NOFOLLOW` only rejects a symlink as the FINAL component,
/// so after opening, the descriptor's (dev, ino) is compared with the path's
/// own `lstat`; and after the operation the paths are compared with the
/// descriptors again. Any mismatch means the path no longer names the directory
/// that was mutated: the caller must NOT record completion.
///
/// Limits: ancestors above the parent are resolved by the kernel during
/// `open`; a swap of an ancestor between the caller's earlier confinement
/// checks and this open is not detected here beyond the final identity
/// comparison. Directory fsync makes directory entries durable, not file data.
#[cfg(unix)]
pub(crate) struct RenameDirs {
    source_dir: std::fs::File,
    destination_dir: std::fs::File,
    source_parent: std::path::PathBuf,
    destination_parent: std::path::PathBuf,
    source_name: std::ffi::OsString,
    destination_name: std::ffi::OsString,
    source_id: (u64, u64),
    destination_id: (u64, u64),
}

#[cfg(unix)]
impl RenameDirs {
    /// Opens (O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC) and verifies both parents.
    pub(crate) fn open(source: &Path, destination: &Path) -> io::Result<Self> {
        let split = |path: &Path| -> io::Result<(std::path::PathBuf, std::ffi::OsString)> {
            match (path.parent(), path.file_name()) {
                (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
                    Ok((parent.to_path_buf(), name.to_os_string()))
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("no parent directory to bind for {}", path.display()),
                )),
            }
        };
        let (source_parent, source_name) = split(source)?;
        let (destination_parent, destination_name) = split(destination)?;
        let (source_dir, source_id) = open_verified(&source_parent)?;
        let (destination_dir, destination_id) = if source_parent == destination_parent {
            (source_dir.try_clone()?, source_id)
        } else {
            open_verified(&destination_parent)?
        };
        Ok(Self {
            source_dir,
            destination_dir,
            source_parent,
            destination_parent,
            source_name,
            destination_name,
            source_id,
            destination_id,
        })
    }

    /// `/proc/self/fd/N/<name>`: a path that resolves INSIDE the retained
    /// directory descriptor, whatever the original path now names. Used so the
    /// path-based identity helpers inspect the directory that was verified.
    pub(crate) fn source_entry(&self) -> std::path::PathBuf {
        fd_entry(&self.source_dir, &self.source_name)
    }

    pub(crate) fn destination_entry(&self) -> std::path::PathBuf {
        fd_entry(&self.destination_dir, &self.destination_name)
    }

    /// `renameat2(RENAME_NOREPLACE)` between the retained descriptors:
    /// `reverse == false` moves source -> destination, `true` the other way.
    pub(crate) fn rename_noreplace(
        &self,
        reverse: bool,
    ) -> Result<(), super::noclobber::NoClobberError> {
        use super::noclobber::NoClobberError;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let (from_fd, from_name, to_fd, to_name) = if reverse {
            (
                &self.destination_dir,
                &self.destination_name,
                &self.source_dir,
                &self.source_name,
            )
        } else {
            (
                &self.source_dir,
                &self.source_name,
                &self.destination_dir,
                &self.destination_name,
            )
        };
        let from = std::ffi::CString::new(from_name.as_bytes())
            .map_err(|_| NoClobberError::Io("name contains a NUL byte".into()))?;
        let to = std::ffi::CString::new(to_name.as_bytes())
            .map_err(|_| NoClobberError::Io("name contains a NUL byte".into()))?;
        // SAFETY: both descriptors are open directories owned by `self`; both names
        // are NUL-terminated and outlive the call; the syscall retains nothing.
        let rc = unsafe {
            libc::renameat2(
                from_fd.as_raw_fd(),
                from.as_ptr(),
                to_fd.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::AlreadyExists {
            Err(NoClobberError::DestinationExists)
        } else {
            Err(NoClobberError::Io(error.to_string()))
        }
    }

    /// Fsyncs the retained descriptors and then re-verifies that both parent
    /// PATHS still name those directories. The directory that received the
    /// entry is synced first (`reverse == false`: the destination directory;
    /// `true`: the source directory, the target of an undo), then the other
    /// unless it is the same directory. A replacement is an error even if the
    /// fsyncs succeeded: the path no longer addresses what was mutated.
    pub(crate) fn sync_and_verify(&self, reverse: bool) -> io::Result<()> {
        let (first_dir, first_path, second_dir, second_path) = if reverse {
            (
                &self.source_dir,
                &self.source_parent,
                &self.destination_dir,
                &self.destination_parent,
            )
        } else {
            (
                &self.destination_dir,
                &self.destination_parent,
                &self.source_dir,
                &self.source_parent,
            )
        };
        let first = sync_fd(first_dir, first_path)
            .map_err(|e| staged("destination directory", first_path, e));
        let second = if self.source_id == self.destination_id {
            Ok(())
        } else {
            sync_fd(second_dir, second_path).map_err(|e| staged("source directory", second_path, e))
        };
        first.and(second)?;
        self.verify_bound()
    }

    /// The parent paths must still resolve (without following a final symlink)
    /// to the directories this value holds descriptors for.
    pub(crate) fn verify_bound(&self) -> io::Result<()> {
        for (path, id, stage) in [
            (
                &self.destination_parent,
                self.destination_id,
                "destination directory",
            ),
            (&self.source_parent, self.source_id, "source directory"),
        ] {
            match path_identity(path) {
                Ok(current) if current == id => {}
                other => {
                    let detail = match other {
                        Ok(_) => "it now names a different directory".to_string(),
                        Err(error) => error.to_string(),
                    };
                    return Err(io::Error::other(format!(
                        "{stage} {} was replaced or substituted while the operation was in \
                         progress ({detail}); the descriptor-bound directory is not the one the \
                         path now names",
                        path.display()
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn fd_entry(dir: &std::fs::File, name: &std::ffi::OsStr) -> std::path::PathBuf {
    use std::os::fd::AsRawFd;
    std::path::PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(name)
}

#[cfg(unix)]
fn path_identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::other(format!(
            "{} is not a real directory (symlink or other object)",
            path.display()
        )));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
fn open_verified(path: &Path) -> io::Result<(std::fs::File, (u64, u64))> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let opened = handle.metadata()?;
    let id = (opened.dev(), opened.ino());
    if path_identity(path)? != id {
        return Err(io::Error::other(format!(
            "{} changed identity while it was being opened",
            path.display()
        )));
    }
    Ok((handle, id))
}

#[cfg(unix)]
fn sync_fd(handle: &std::fs::File, nominal: &Path) -> io::Result<()> {
    #[cfg(test)]
    hook::before(nominal)?;
    handle.sync_all()?;
    #[cfg(test)]
    hook::after(nominal);
    let _ = nominal;
    Ok(())
}

/// Synchronises the directories that hold the entries of a rename that the
/// FILESYSTEM shows already happened (recovery). Descriptor-bound: both parents
/// are opened and verified first, the expected name is proven present INSIDE the
/// opened destination (`at_destination`) or source directory, and only then are
/// those descriptors fsynced and re-verified.
#[cfg(unix)]
pub(crate) fn sync_observed_rename(
    source: &Path,
    destination: &Path,
    entry_at_destination: bool,
) -> io::Result<()> {
    let dirs = RenameDirs::open(source, destination)?;
    let observed = if entry_at_destination {
        dirs.destination_entry()
    } else {
        dirs.source_entry()
    };
    std::fs::symlink_metadata(&observed).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("the observed entry is not inside the verified directory: {error}"),
        )
    })?;
    dirs.sync_and_verify(!entry_at_destination)
}

#[cfg(not(unix))]
pub(crate) fn sync_observed_rename(
    _source: &Path,
    _destination: &Path,
    _entry_at_destination: bool,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory synchronisation is not available on this platform",
    ))
}

/// Marker the executor puts in an entry's `failure_reason` when the rename
/// completed but its directory entries could not be synchronised. Used only
/// for REPORTING; recovery decisions never depend on this text.
pub(crate) const SYNC_FAILURE_MARKER: &str = "could not be synchronised";

fn staged(stage: &str, dir: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{stage} {}: {error}", dir.display()))
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
