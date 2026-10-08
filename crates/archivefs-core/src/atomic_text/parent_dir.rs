//! Single-component operations relative to a retained parent directory.
//! Path identity observations diagnose rebinding; the descriptor supplies the
//! actual operation boundary even when a rebind happens after an observation.

use std::ffi::CString;
use std::fs::{self, File};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// An absolute path, slash or traversal component must never bypass dirfd.
pub(super) struct Leaf(CString);

impl Leaf {
    pub(super) fn from_path(path: &Path) -> io::Result<Self> {
        let bytes = path.as_os_str().as_bytes();
        let name = bytes
            .rsplit(|byte| *byte == b'/')
            .next()
            .unwrap_or_default();
        if name.is_empty() || name == b"." || name == b".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a file basename",
            ));
        }
        CString::new(name)
            .map(Self)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "file basename contains NUL"))
    }
}

pub(super) struct ParentDir {
    handle: File,
    path: PathBuf,
    identity: (u64, u64),
}

impl ParentDir {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let name = CString::new(path.as_os_str().as_bytes())?;
        // Linux O_PATH avoids introducing a directory-read requirement for
        // write/search-only directories. Other Unix targets retain a dirfd,
        // but the O_RDONLY fallback requires read permission (unverified).
        #[cfg(target_os = "linux")]
        let access = libc::O_PATH;
        #[cfg(not(target_os = "linux"))]
        let access = libc::O_RDONLY;
        // SAFETY: name is NUL-terminated and remains valid for this call.
        let fd = unsafe { libc::open(name.as_ptr(), access | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        let handle = owned_file(fd)?;
        let metadata = handle.metadata()?;
        Ok(Self {
            handle,
            path: path.to_owned(),
            identity: (metadata.dev(), metadata.ino()),
        })
    }

    pub(super) fn require_binding(&self) -> io::Result<()> {
        let current = fs::metadata(&self.path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "parent directory binding unavailable; pinned {:?}: {error}",
                    self.identity
                ),
            )
        })?;
        if !current.is_dir() || (current.dev(), current.ino()) != self.identity {
            return Err(io::Error::other(format!(
                "parent directory binding changed; pinned {:?}, observed {:?}",
                self.identity,
                (current.dev(), current.ino())
            )));
        }
        Ok(())
    }

    pub(super) fn create_stage(&self, leaf: &Leaf, mode: u32) -> io::Result<File> {
        // SAFETY: a live directory fd and a validated single-component C
        // string are supplied; O_EXCL never appropriates an existing object.
        let fd = unsafe {
            libc::openat(
                self.handle.as_raw_fd(),
                leaf.0.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode as libc::mode_t,
            )
        };
        owned_file(fd)
    }

    pub(super) fn metadata(&self, leaf: &Leaf) -> io::Result<libc::stat> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: stat is writable; the fd and string remain live. On success
        // fstatat initializes stat. Final symlinks are inspected, not followed.
        let result = unsafe {
            libc::fstatat(
                self.handle.as_raw_fd(),
                leaf.0.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        succeeded(result)?;
        // SAFETY: successful fstatat initialized the output above.
        Ok(unsafe { stat.assume_init() })
    }

    pub(super) fn replace(&self, stage: &Leaf, destination: &Leaf) -> io::Result<()> {
        // SAFETY: both validated basenames resolve under the same live fd.
        // renameat retains intentional replacement; NOREPLACE would change it.
        succeeded(unsafe {
            libc::renameat(
                self.handle.as_raw_fd(),
                stage.0.as_ptr(),
                self.handle.as_raw_fd(),
                destination.0.as_ptr(),
            )
        })
    }

    pub(super) fn remove_stage(&self, stage: &Leaf) -> io::Result<()> {
        // SAFETY: live fd and single basename; flags 0 cannot remove a directory.
        succeeded(unsafe { libc::unlinkat(self.handle.as_raw_fd(), stage.0.as_ptr(), 0) })
    }

    pub(super) fn sync_best_effort(&self) {
        // O_PATH itself cannot be synced. Reopen '.' under that descriptor so
        // the old best-effort policy syncs the original directory object.
        // SAFETY: live directory fd and a static NUL-terminated relative name.
        let fd = unsafe {
            libc::openat(
                self.handle.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if let Ok(handle) = owned_file(fd) {
            let _ = handle.sync_all();
        }
    }
}

fn owned_file(fd: libc::c_int) -> io::Result<File> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful open/openat returned a fresh owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn succeeded(result: libc::c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
