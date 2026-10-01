//! Descriptor-relative operations for the private launch workspace. No pathname
//! recursive deletion, symlink traversal, hardlinking, or copy acceleration.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

fn name(value: &OsStr) -> io::Result<CString> {
    if value.is_empty() || value == "." || value == ".." || value.as_bytes().contains(&b'/') {
        return Err(io::Error::other("not a single safe path component"));
    }
    CString::new(value.as_bytes()).map_err(io::Error::other)
}

pub(super) fn child(parent: &File, value: &OsStr, flags: i32) -> io::Result<File> {
    let value = name(value)?;
    // SAFETY: parent remains live, name is NUL terminated, and the returned
    // descriptor is uniquely owned. O_NONBLOCK prevents a substituted FIFO
    // from hanging preparation; its type is checked before reading.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            value.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(super) fn directory(parent: &File, value: &OsStr) -> io::Result<File> {
    child(parent, value, libc::O_RDONLY | libc::O_DIRECTORY)
}

/// Linux UAPI openat2: RESOLVE_NO_XDEV includes same-device bind mounts,
/// unlike comparing st_dev alone. Refuse unsupported kernels/seccomp rather
/// than falling back to a potentially unsafe cleanup traversal.
pub(super) fn cleanup_directory(parent: &File, value: &OsStr) -> io::Result<File> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let value = name(value)?;
    let how = OpenHow {
        flags: (libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) as u64,
        mode: 0,
        // /usr/include/linux/openat2.h: NO_XDEV | NO_SYMLINKS | BENEATH.
        resolve: 0x01 | 0x04 | 0x08,
    };
    // SAFETY: struct matches Linux UAPI, pointers live through the syscall,
    // and success returns a newly owned file descriptor.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            parent.as_raw_fd(),
            value.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}

pub(super) fn absolute(path: &Path, is_directory: bool) -> io::Result<File> {
    if !path.is_absolute()
        || path
            .as_os_str()
            .as_bytes()
            .split(|b| *b == b'/')
            .any(|v| v == b"." || v == b"..")
    {
        return Err(io::Error::other("absolute, non-traversing path required"));
    }
    let components: Vec<_> = path.components().collect();
    let mut current = File::open("/")?;
    for (index, component) in components.iter().enumerate().skip(1) {
        let Component::Normal(value) = component else {
            return Err(io::Error::other("unsafe component"));
        };
        current = child(
            &current,
            value,
            libc::O_RDONLY
                | if is_directory || index + 1 != components.len() {
                    libc::O_DIRECTORY
                } else {
                    0
                },
        )?;
    }
    Ok(current)
}

pub(super) fn private(file: &File, is_directory: bool) -> io::Result<()> {
    let meta = file.metadata()?;
    // SAFETY: geteuid has no arguments or memory effects.
    if meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o777 != if is_directory { 0o700 } else { 0o600 }
        || if is_directory {
            !meta.is_dir()
        } else {
            !meta.is_file() || meta.nlink() != 1
        }
    {
        return Err(io::Error::other(
            "workspace ownership, type, link count or permissions invalid",
        ));
    }
    Ok(())
}

pub(super) fn mkdir(parent: &File, value: &OsStr, allow_existing: bool) -> io::Result<File> {
    let value_c = name(value)?;
    // SAFETY: fd and name live through call; mode is deliberately private.
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), value_c.as_ptr(), 0o700) };
    if result != 0 {
        let error = io::Error::last_os_error();
        if !allow_existing || error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    let file = directory(parent, value)?;
    private(&file, true)?;
    Ok(file)
}

pub(super) fn create(parent: &File, value: &OsStr) -> io::Result<File> {
    child(parent, value, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL)
}

pub(super) fn lock(file: &File) -> io::Result<()> {
    // SAFETY: advisory exclusive, nonblocking lock on a live owned fd.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn entries(dir: &File, limit: usize) -> io::Result<Vec<OsString>> {
    // A fresh open gives an independent directory offset (dup would share it).
    let dot = c".";
    // SAFETY: dir is live; fdopendir takes ownership only on success.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            dot.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        unsafe {
            libc::close(fd);
        }
        return Err(error);
    }
    let result = (|| {
        let mut values = Vec::new();
        loop {
            // SAFETY: stream is owned here and readdir's result lives until
            // the next call. Linux errno distinguishes EOF from failure.
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let errno = unsafe { *libc::__errno_location() };
                if errno != 0 {
                    return Err(io::Error::from_raw_os_error(errno));
                }
                break;
            }
            let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if values.len() == limit {
                return Err(io::Error::other("workspace entry bound exceeded"));
            }
            values.push(OsString::from_vec(bytes.to_vec()));
        }
        values.sort();
        Ok(values)
    })();
    unsafe {
        libc::closedir(stream);
    }
    result
}

pub(super) fn unlink(parent: &File, value: &OsStr, directory: bool) -> io::Result<()> {
    let value = name(value)?;
    // SAFETY: unlinkat operates on this one component, never follows a link.
    if unsafe {
        libc::unlinkat(
            parent.as_raw_fd(),
            value.as_ptr(),
            if directory { libc::AT_REMOVEDIR } else { 0 },
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn replace_marker(parent: &File, from: &OsStr, to: &OsStr) -> io::Result<()> {
    let (from, to) = (name(from)?, name(to)?);
    // SAFETY: both names are single components of the same owned directory.
    // Only the validated ownership record is replaced, never a user path.
    if unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    parent.sync_all()
}

/// Bounded cleanup below an already ownership-checked transaction fd. A
/// symlink created by an emulator is unlinked, never traversed. Cross-device
/// directories and excessive depth/entries are refused, not swept broadly.
pub(super) fn clear(dir: &File, device: u64, depth: usize, budget: &mut usize) -> io::Result<()> {
    if depth > 16 || dir.metadata()?.dev() != device {
        return Err(io::Error::other("cleanup boundary exceeded"));
    }
    for value in entries(dir, *budget)? {
        if depth == 0 && (value == ".emuwiz-owned" || value == ".lease") {
            continue;
        }
        if *budget == 0 {
            return Err(io::Error::other("cleanup entry bound exceeded"));
        }
        *budget -= 1;
        match cleanup_directory(dir, &value) {
            Ok(subdir) => {
                // Emulator-created directories need not use our 0700 mode;
                // the containing transaction remains private. Never traverse
                // a directory owned by another uid or on another device.
                if subdir.metadata()?.uid() != dir.metadata()?.uid() {
                    return Err(io::Error::other("foreign directory in workspace"));
                }
                clear(&subdir, device, depth + 1, budget)?;
                unlink(dir, &value, true)?;
            }
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) => {
                unlink(dir, &value, false)?
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
