//! Descriptor-relative reads recovered from aa9eedc4. No named staging,
//! pathname publication, unlink, recursive cleanup or named capability probes.
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
fn name_c(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))
}

fn open_at(dir: &File, name: &OsStr, flags: libc::c_int, mode: libc::mode_t) -> io::Result<File> {
    let name = name_c(name)?;
    loop {
        // SAFETY: `name` is a valid NUL-terminated string that outlives the call,
        // `dir` is a live descriptor, and the returned descriptor is wrapped
        // exactly once below.
        let descriptor = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                mode as libc::c_uint,
            )
        };
        if descriptor >= 0 {
            // SAFETY: `descriptor` is a fresh, owned, valid descriptor.
            return Ok(unsafe { File::from_raw_fd(descriptor) });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

pub(super) fn open_file_at(dir: &File, name: &OsStr) -> io::Result<File> {
    open_at(dir, name, libc::O_RDONLY | libc::O_NONBLOCK, 0)
}

/// What a directory entry is, judged without following links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Directory,
    File,
    Symlink,
    Other,
}

pub(super) fn kind_at(dir: &File, name: &OsStr) -> io::Result<Kind> {
    let name = name_c(name)?;
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: valid descriptor and name; `status` is written by a successful call.
    let result = unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            status.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: initialised by the successful `fstatat` above.
    let mode = unsafe { status.assume_init() }.st_mode & libc::S_IFMT;
    Ok(match mode {
        libc::S_IFDIR => Kind::Directory,
        libc::S_IFREG => Kind::File,
        libc::S_IFLNK => Kind::Symlink,
        _ => Kind::Other,
    })
}

/// Names in a directory (never `.` or `..`), read through the descriptor.
pub(super) fn list_dir(dir: &File) -> io::Result<Vec<OsString>> {
    // SAFETY: duplicating a live descriptor; ownership of the copy passes to
    // `fdopendir` on success and is closed by us on failure.
    let copy = unsafe { libc::dup(dir.as_raw_fd()) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `copy` is a valid directory descriptor owned by us.
    let stream = unsafe { libc::fdopendir(copy) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: `fdopendir` failed, so `copy` is still ours to close.
        unsafe { libc::close(copy) };
        return Err(error);
    }
    // The duplicate shares its offset with `dir`; always list from the start.
    // SAFETY: `stream` is a valid, open directory stream until `closedir`.
    unsafe { libc::rewinddir(stream) };
    let mut names = Vec::new();
    let outcome = loop {
        // SAFETY: errno is thread-local; reset so end-of-directory is distinguishable.
        unsafe { *libc::__errno_location() = 0 };
        // SAFETY: `stream` is valid; the entry is copied before the next call.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            break if error.raw_os_error() == Some(0) {
                Ok(())
            } else {
                Err(error)
            };
        }
        // SAFETY: `d_name` is NUL-terminated within the entry.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            if names.len() >= super::reviewed::MAX_ENTRIES {
                break Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory entry limit",
                ));
            }
            names.push(OsString::from_vec(name.to_vec()));
        }
    };
    // SAFETY: closes the stream and the duplicated descriptor it owns.
    unsafe { libc::closedir(stream) };
    outcome.map(|()| names)
}

fn filesystem(file: &File) -> io::Result<libc::c_long> {
    let mut result = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: live descriptor and valid writable statfs storage.
    if unsafe { libc::fstatfs(file.as_raw_fd(), result.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: initialized by successful fstatfs.
    Ok(unsafe { result.assume_init() }.f_type)
}
/// No filesystem type inference from path/label. Local ext4 and kernel procfs
/// are the tested subset. Unsupported roots fail before staging is created.
pub(super) fn eligible(parent: &File) -> io::Result<()> {
    if filesystem(parent)? != 0xef53 || filesystem(&File::open("/proc/self/fd")?)? != 0x9fa0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "local ext4 and procfs required",
        ));
    }
    Ok(())
}
pub(super) fn anonymous(parent: &File) -> io::Result<File> {
    // O_EXCL must NOT be used with O_TMPFILE: it would prohibit publication.
    // Drop closes the anonymous inode; there is no mutable name to unlink.
    open_at(
        parent,
        OsStr::new("."),
        libc::O_TMPFILE | libc::O_RDWR,
        0o600,
    )
}
pub(super) fn publish(parent: &File, file: &File, destination: &OsStr) -> io::Result<()> {
    let source = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap();
    let name = name_c(destination)?;
    // SAFETY: owned, live file/parent FDs; valid names. The source resolves to
    // the pinned anonymous inode, not a replaceable staging pathname. linkat
    // atomically refuses every occupied destination, including dangling links.
    if unsafe {
        libc::linkat(
            libc::AT_FDCWD,
            source.as_ptr(),
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::AT_SYMLINK_FOLLOW,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
