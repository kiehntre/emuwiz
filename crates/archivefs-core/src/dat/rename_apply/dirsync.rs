//! Directory-entry durability for completed renames.
//!
//! `renameat2` changes directory entries; the journal's own `fsync` does not
//! cover them. After a rename succeeds, the parent directory (or directories)
//! whose entries changed must be synchronised **before** a successful
//! completion is recorded. This makes the directory entry durable; it says
//! nothing about the file's *data* — content durability is a separate property
//! of whoever wrote the file, and this module does not provide or prove it.
//!
//! KNOWN LIMITS (documented; this module does not claim power-loss safety):
//!   * Apply, undo and copy-publish resolve the rename's parents FROM the transaction's
//!     trusted roots (`RenameDirs::open_in`: root once, then `openat(O_NOFOLLOW)` per
//!     component), so a substituted ancestor is refused rather than followed. With NO
//!     trusted roots (legacy callers/tests) only the final component is protected.
//!   * Recovery (`reconcile_recovery`) anchors at the journalled collection root
//!     (`source_scan_root`) when both parents lie below it, using the same per-component
//!     `O_NOFOLLOW` walk; otherwise the parents must be their own real paths. A redirected,
//!     moved or symlink-substituted ancestor cannot bind, so the entry stays unresolved and
//!     is never promoted to Applied on file identity alone.
//!   * CONCURRENT EXTERNAL WRITERS: verification observes a point in time, not
//!     exclusion. Recovery retains bindings through publication and makes one
//!     unresolved correction attempt on a detected late swap. A later swap or
//!     failed correction can still leave historical Applied bytes on disk.
//!     Reload and execution reject demonstrably redirected Applied paths; those
//!     records are not proof of a current location. Enforced writer exclusion
//!     is required for an atomic containment/publication guarantee.
//!   * Roots must be their own real paths, opened O_NOFOLLOW and identity-bound
//!     through verification. A journal cannot prove an old symlink-root identity.
//!     Configured symlink aliases can still supply a previously resolved real
//!     root, but journal aliases without such independent proof are refused.
//!   * Creating destination directories (`create_dir` in plan apply and quarantine) and
//!     removing created directories during rollback (`remove_dir`) do not fsync their
//!     parent directory, so those directory-entry changes are not proven durable.
//!   * Directory fsync says nothing about file DATA durability or device write caches;
//!     filesystems without reliable directory fsync (some FUSE/NFS) fail closed.
//!
//! A failure here means the rename may already have happened. Callers must
//! not repeat the rename, must not record success, and must not describe the
//! original state as unchanged.

use std::io;
use std::path::Path;

#[cfg(test)]
pub(crate) use hook::{DirSyncEvent, set_open_hook, set_test_hook};

#[cfg(unix)]
#[derive(Clone)]
struct RootBinding {
    path: std::path::PathBuf,
    identity: (u64, u64),
    // Retain the inode itself, not only an inode number that might be reused.
    _handle: std::sync::Arc<std::fs::File>,
}

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
    /// The trusted root each parent was resolved from (None: anchor-less bind, which
    /// requires the path to be its own real path).  Used to RE-PROVE containment.
    source_root: Option<RootBinding>,
    destination_root: Option<RootBinding>,
}

#[cfg(unix)]
impl RenameDirs {
    /// Creates a symlink inside the verified destination parent. The kernel
    /// resolves the parent descriptor, so a later path swap cannot redirect
    /// this operation to an unrelated directory.
    pub(crate) fn create_link(&self, target: &Path) -> io::Result<super::model::ObjectIdentity> {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        self.verify_bound()?;
        let target_text = target.to_path_buf();
        let target = CString::new(target.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid link target"))?;
        let name = CString::new(self.destination_name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid link name"))?;
        // SAFETY: both C strings and the destination descriptor remain live.
        if unsafe {
            libc::symlinkat(
                target.as_ptr(),
                self.destination_dir.as_raw_fd(),
                name.as_ptr(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let proof = super::identity::capture_identity(&self.destination_entry())?;
        if proof.kind != super::model::ObjectKind::Symlink
            || std::fs::read_link(self.destination_entry())? != target_text
        {
            return Err(io::Error::other(
                "created link changed before it could be proved",
            ));
        }
        self.destination_dir.sync_all()?;
        self.verify_bound()?;
        Ok(proof)
    }

    /// Removes only the journal-proved link in the bound destination parent.
    /// A path substitution before binding or a different link object refuses.
    pub(crate) fn remove_recorded_link(
        &self,
        expected: &super::model::ObjectIdentity,
        target: &Path,
    ) -> io::Result<()> {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        self.verify_bound()?;
        let entry = self.destination_entry();
        let current = super::identity::capture_identity(&entry)?;
        if current.kind != super::model::ObjectKind::Symlink
            || !super::identity::identity_matches(expected, &current)
            || std::fs::read_link(&entry)? != target
        {
            return Err(io::Error::other(
                "destination is not the journal-proved link object",
            ));
        }
        let name = CString::new(self.destination_name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid destination name"))?;
        // SAFETY: the verified destination parent descriptor and NUL-terminated
        // filename remain live for the syscall; unlinkat never follows a link.
        if unsafe { libc::unlinkat(self.destination_dir.as_raw_fd(), name.as_ptr(), 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        self.destination_dir.sync_all()?;
        self.verify_bound()
    }

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
        // With no trusted root to anchor to, containment cannot be re-proved from
        // an anchor, so the journalled path must BE its own real path: no symlink
        // or redirected ancestor anywhere above the parent. (A legitimately
        // symlinked prefix is refused rather than guessed at; pass a trusted root
        // via `open_in` / `open_anchored` to allow it.)
        for parent in [&source_parent, &destination_parent] {
            require_own_real_path(parent)?;
        }
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
            source_root: None,
            destination_root: None,
        })
    }

    /// Binds for RECOVERY: when both parents are lexically below `anchor` (the
    /// journalled collection root) they are resolved from it with the
    /// per-component `O_NOFOLLOW` walk of [`Self::open_in`], so an ancestor
    /// replaced by a symlink or a moved subtree is refused; otherwise the
    /// anchor-less rule of [`Self::open`] applies (the path must be its own
    /// real path). Never "succeeds" on a path that resolves outside the anchor.
    pub(crate) fn open_anchored(
        source: &Path,
        destination: &Path,
        anchor: Option<&Path>,
    ) -> io::Result<Self> {
        if let Some(anchor) = anchor.filter(|a| a.is_absolute() && !a.as_os_str().is_empty()) {
            let below = |p: &Path| p.parent().is_some_and(|parent| parent.starts_with(anchor));
            if below(source) && below(destination) {
                // A journal path is not proof of an originally configured symlink.
                require_own_real_path(anchor)?;
                let roots = crate::safe_read::TrustedRoots::from_paths([anchor]);
                return Self::open_in(source, destination, &roots);
            }
        }
        Self::open(source, destination)
    }

    /// Like [`Self::open`], but with the parents resolved FROM a trusted root: the
    /// root is opened once and every component below it is opened with
    /// `openat(O_DIRECTORY|O_NOFOLLOW)` against the previous descriptor, so a
    /// symlink substituted for ANY ancestor between the caller's confinement
    /// checks and this open is refused instead of followed (it cannot redirect
    /// the rename outside the root), and the kernel never re-resolves a path
    /// string for the parents. An empty `trusted` set falls back to
    /// [`Self::open`]. A parent outside every trusted root is refused.
    pub(crate) fn open_in(
        source: &Path,
        destination: &Path,
        trusted: &crate::safe_read::TrustedRoots,
    ) -> io::Result<Self> {
        if trusted.is_empty() {
            return Self::open(source, destination);
        }
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
        let (source_dir, source_id, source_root) = open_under_root(&source_parent, trusted)?;
        let (destination_dir, destination_id, destination_root) =
            if source_parent == destination_parent {
                (source_dir.try_clone()?, source_id, source_root.clone())
            } else {
                open_under_root(&destination_parent, trusted)?
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
            source_root: Some(source_root),
            destination_root: Some(destination_root),
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
        for (path, id, stage, root) in [
            (
                &self.destination_parent,
                self.destination_id,
                "destination directory",
                &self.destination_root,
            ),
            (
                &self.source_parent,
                self.source_id,
                "source directory",
                &self.source_root,
            ),
        ] {
            // Containment first: matching device/inode alone is not enough - a whole
            // subtree can be moved elsewhere and reached through a substituted ancestor
            // with its identities unchanged.
            let contained = match root {
                Some(binding) => require_own_real_path(&binding.path).and_then(|()| {
                    let root = &binding.path;
                    if path_identity(root)? != binding.identity {
                        return Err(io::Error::other("trusted root identity changed"));
                    }
                    plain_below_root(root, path)
                }),
                None => require_own_real_path(path),
            };
            if let Err(error) = contained {
                return Err(io::Error::other(format!(
                    "{stage} {} is no longer contained where it was bound ({error}); the \
                     path was redirected or its ancestry replaced while the operation was \
                     in progress",
                    path.display()
                )));
            }
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

/// The path must equal its own canonical form (no symlinked or redirected component).
#[cfg(unix)]
fn require_own_real_path(path: &Path) -> io::Result<()> {
    let real = std::fs::canonicalize(path)?;
    if real != path {
        return Err(io::Error::other(format!(
            "{} is not its own real path (it resolves to {}): an ancestor is a symlink or was \
             redirected, so containment cannot be established",
            path.display(),
            real.display()
        )));
    }
    Ok(())
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

/// Opens `parent` by walking down from the most specific trusted root that
/// contains it: root once (its own ancestors are trusted by definition), then
/// one `openat(O_DIRECTORY|O_NOFOLLOW)` per component. Every intermediate
/// descriptor is retained only until the next is opened.
#[cfg(unix)]
fn open_under_root(
    parent: &Path,
    trusted: &crate::safe_read::TrustedRoots,
) -> io::Result<(std::fs::File, (u64, u64), RootBinding)> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Component;

    let root = trusted
        .roots()
        .iter()
        .filter(|root| parent.starts_with(root))
        .max_by_key(|root| root.components().count())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is outside every trusted root", parent.display()),
            )
        })?;
    require_own_real_path(root)?;
    let root_id = path_identity(root)?;
    let relative = parent.strip_prefix(root).map_err(io::Error::other)?;
    let mut current = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let opened_root = current.metadata()?;
    if (opened_root.dev(), opened_root.ino()) != root_id {
        return Err(io::Error::other("trusted root changed during open"));
    }
    let root_handle = std::sync::Arc::new(current.try_clone()?);
    let mut walked = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} is not a plain path below its trusted root",
                    parent.display()
                ),
            ));
        };
        walked.push(name);
        #[cfg(test)]
        hook::open_component(&walked);
        let c_name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path component"))?;
        // SAFETY: `current` is an open directory descriptor owned here; `c_name` is a
        // NUL-terminated component that outlives the call; the result is checked and
        // wrapped in an owning File immediately.
        let fd = unsafe {
            libc::openat(
                current.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "{} could not be opened as a plain directory below the trusted root ({error}); \
                     an ancestor may have been replaced",
                    walked.display()
                ),
            ));
        }
        // SAFETY: `fd` is a freshly opened descriptor not owned by anything else.
        current = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    let opened = current.metadata()?;
    let id = (opened.dev(), opened.ino());
    if path_identity(parent)? != id {
        return Err(io::Error::other(format!(
            "{} resolves to a different directory than the one opened below the trusted root",
            parent.display()
        )));
    }
    require_own_real_path(root)?;
    if path_identity(root)? != root_id {
        return Err(io::Error::other("trusted root changed during traversal"));
    }
    Ok((
        current,
        id,
        RootBinding {
            path: root.to_path_buf(),
            identity: root_id,
            _handle: root_handle,
        },
    ))
}

/// Every component of `parent` below `root` is (still) a real directory, never a
/// symlink: containment under the root re-proved NOW, not just when it was opened.
#[cfg(unix)]
fn plain_below_root(root: &Path, parent: &Path) -> io::Result<()> {
    let relative = parent.strip_prefix(root).map_err(io::Error::other)?;
    let mut walked = root.to_path_buf();
    for component in relative.components() {
        walked.push(component);
        let metadata = std::fs::symlink_metadata(&walked)?;
        if !metadata.is_dir() {
            return Err(io::Error::other(format!(
                "{} is no longer a plain directory below the trusted root {}",
                walked.display(),
                root.display()
            )));
        }
    }
    Ok(())
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
/// FILESYSTEM shows already happened (recovery). Descriptor-bound end to end:
/// both parents are opened (below the trusted roots when given) and verified
/// first; the object at the observed name is then inspected THROUGH the opened
/// directory and must match the journalled identity (existence alone proves
/// nothing - a replacement directory can hold a file of the same name); only
/// then are those same descriptors fsynced and re-verified.
#[cfg(unix)]
pub(crate) fn sync_observed_rename(
    source: &Path,
    destination: &Path,
    entry_at_destination: bool,
    expected: &super::model::ObjectIdentity,
    trusted: &crate::safe_read::TrustedRoots,
) -> io::Result<()> {
    let dirs = RenameDirs::open_in(source, destination, trusted)?;
    let observed = if entry_at_destination {
        dirs.destination_entry()
    } else {
        dirs.source_entry()
    };
    let current = super::identity::capture_identity(&observed).map_err(|error| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("the observed entry is not inside the verified directory: {error}"),
        )
    })?;
    if !super::identity::identity_matches(expected, &current) {
        return Err(io::Error::other(
            "the object inside the verified directory is not the journalled object",
        ));
    }
    dirs.sync_and_verify(!entry_at_destination)
}

#[cfg(not(unix))]
pub(crate) fn sync_observed_rename(
    _source: &Path,
    _destination: &Path,
    _entry_at_destination: bool,
    _expected: &super::model::ObjectIdentity,
    _trusted: &crate::safe_read::TrustedRoots,
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

    type OpenHook = Box<dyn FnMut(&Path)>;
    thread_local! { static OPEN_HOOK: RefCell<Option<OpenHook>> = const { RefCell::new(None) }; }

    /// Runs just before each path component is opened below a trusted root:
    /// the deterministic point at which a concurrent replacer would strike.
    pub(crate) fn set_open_hook(hook: Option<OpenHook>) {
        OPEN_HOOK.with(|h| *h.borrow_mut() = hook);
    }
    pub(super) fn open_component(path: &Path) {
        let taken = OPEN_HOOK.with(|h| h.borrow_mut().take());
        let Some(mut hook) = taken else { return };
        hook(path);
        OPEN_HOOK.with(|h| {
            let mut slot = h.borrow_mut();
            if slot.is_none() {
                *slot = Some(hook);
            }
        });
    }

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
