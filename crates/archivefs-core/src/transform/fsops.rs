//! Descriptor-bound filesystem operations for the engine.
//!
//! Every directory is opened FROM its library root with the shared
//! per-component `O_NOFOLLOW` walk (`rename_apply::dirsync`), so a symlink
//! substituted for any ancestor is refused instead of followed. Entries are
//! then addressed relative to the retained descriptor (`*at` syscalls), and
//! identity helpers read them through `/proc/self/fd/N/<name>`, which always
//! resolves inside the directory that was verified.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::dat::rename_apply::dirsync::{entry_in, open_dir_beneath};
use crate::safe_read::TrustedRoots;

pub(crate) struct Dir {
    pub file: File,
    pub id: (u64, u64),
}

pub(crate) fn split(path: &Path) -> io::Result<(&Path, &OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => Ok((parent, name)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", path.display()),
        )),
    }
}

fn cname(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL byte in file name"))
}

/// Opens `dir` beneath `root`. Refuses when the root itself is gone.
pub(crate) fn open_dir(dir: &Path, root: &Path) -> io::Result<Dir> {
    let trusted = TrustedRoots::from_paths([root]);
    if trusted.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("library root {} is unavailable", root.display()),
        ));
    }
    let (file, id) = open_dir_beneath(dir, &trusted)?;
    Ok(Dir { file, id })
}

/// Opens the parent of `path` beneath `root` and returns it with the leaf.
pub(crate) fn open_parent<'a>(path: &'a Path, root: &Path) -> io::Result<(Dir, &'a OsStr)> {
    let (parent, name) = split(path)?;
    Ok((open_dir(parent, root)?, name))
}

fn check(rc: libc::c_int) -> io::Result<()> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Dir {
    pub fn entry(&self, name: &OsStr) -> PathBuf {
        entry_in(&self.file, name)
    }

    /// `fstatat(AT_SYMLINK_NOFOLLOW)`; `None` when the entry is absent.
    pub fn stat(&self, name: &OsStr) -> io::Result<Option<libc::stat>> {
        let c = cname(name)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: descriptor and C string are live; `st` is a valid out-pointer.
        let rc = unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                c.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc == 0 {
            return Ok(Some(st));
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error)
        }
    }

    pub fn mkdir_private(&self, name: &OsStr) -> io::Result<()> {
        let c = cname(name)?;
        // SAFETY: descriptor and C string are live.
        check(unsafe { libc::mkdirat(self.file.as_raw_fd(), c.as_ptr(), 0o700) })
    }

    pub fn rmdir(&self, name: &OsStr) -> io::Result<()> {
        let c = cname(name)?;
        // SAFETY: descriptor and C string are live.
        check(unsafe { libc::unlinkat(self.file.as_raw_fd(), c.as_ptr(), libc::AT_REMOVEDIR) })
    }

    pub fn unlink(&self, name: &OsStr) -> io::Result<()> {
        let c = cname(name)?;
        // SAFETY: descriptor and C string are live.
        check(unsafe { libc::unlinkat(self.file.as_raw_fd(), c.as_ptr(), 0) })
    }

    fn open_at(&self, name: &OsStr, flags: libc::c_int, mode: libc::c_uint) -> io::Result<File> {
        let c = cname(name)?;
        // SAFETY: descriptor and C string are live; the result is checked and
        // immediately owned.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                mode,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh descriptor owned by nothing else.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// Exclusive create of a new 0600 regular file.
    pub fn create_new(&self, name: &OsStr) -> io::Result<File> {
        self.open_at(name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)
    }

    pub fn open_read(&self, name: &OsStr) -> io::Result<File> {
        self.open_at(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)
    }

    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_all()
    }
}

/// True when `st` is a directory private to this user (mode 0700, owned).
pub(crate) fn is_private_dir(st: &libc::stat) -> bool {
    (st.st_mode & libc::S_IFMT) == libc::S_IFDIR
        && st.st_mode & 0o077 == 0
        && st.st_uid == unsafe { libc::geteuid() }
}

pub(crate) fn is_regular(st: &libc::stat) -> bool {
    (st.st_mode & libc::S_IFMT) == libc::S_IFREG
}

/// Drop cached pages for a synced file so the verification re-read is served
/// by the device rather than by pages this process just wrote. Advisory: a
/// kernel may keep pages, so this narrows, but does not close, the gap
/// between "verified" and "on the medium".
pub(crate) fn drop_cache(file: &File) {
    // SAFETY: valid descriptor; advisory call with no memory effects.
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
    }
}

pub(crate) fn not_found(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    }
}
