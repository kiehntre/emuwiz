//! Directory-anchored file operations for installers that publish into an
//! emulator's own folders (Night Watch D7-03).
//!
//! Every path-based check an installer makes (`assess_destination`,
//! `symlink_metadata`, `create_dir`) is a statement about the path *at that
//! moment*. A parent directory swapped for a symlink between the check and
//! the write redirects the write - and the later cleanup - outside the
//! approved root, while the installer still reports success. The only way
//! to close that window is to stop naming the parent by path at all:
//!
//! 1. open the approved parent directory **once**, with
//!    `O_DIRECTORY | O_NOFOLLOW`, and compare its `(device, inode)` with the
//!    directory the read-only assessment looked at;
//! 2. perform every create, verify, rename, exchange, unlink and fsync
//!    *relative to that open descriptor* (`openat`, `renameat2`,
//!    `unlinkat`, `fstatat`), with single-component names only.
//!
//! After step 1 there is no path string left for a swap to redirect: the
//! descriptor *is* the directory, whatever its name now points at.
//!
//! This module is deliberately small and self-contained: it owns the
//! `unsafe` libc calls and nothing else. It holds no policy (what may be
//! written, when, with which approval) - callers keep that. The shared
//! transaction executor has its own private descriptor helpers for
//! directory bootstrap; this module is the file-level counterpart for the
//! cheat installer and rollback, and is Linux/Unix only like the
//! guarantees it provides.

use std::ffi::{CString, OsStr};
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// `(device, inode)` of a directory, read from an open descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryIdentity {
    pub device: u64,
    pub inode: u64,
}

impl DirectoryIdentity {
    /// The identity a path-based `symlink_metadata` observed. Compare it
    /// with [`DirectoryHandle::identity`] to prove the handle is the very
    /// directory that was assessed.
    pub fn of_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

/// An open directory every operation is resolved against.
#[derive(Debug)]
pub struct DirectoryHandle {
    fd: OwnedFd,
}

/// Rejects anything that is not one plain name inside a directory: empty,
/// `.`/`..`, a separator, or a NUL byte. The caller decides what a *safe*
/// name is (see `cheat_catalogue::cheat_destination_file_name`); this only
/// guarantees the kernel resolves the name inside the handle's directory.
fn component(name: &OsStr) -> io::Result<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "name is not a single path component",
        ));
    }
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

impl DirectoryHandle {
    /// Opens `path` as a directory, refusing to follow a symlink at its
    /// final component. This is the one plain-pathname open; everything
    /// else on the handle is descriptor-relative.
    pub fn open(path: &Path) -> io::Result<Self> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
        // SAFETY: `c_path` is a valid NUL-terminated string for the call;
        // `open` returns an owned descriptor (>= 0) or -1 with errno set.
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_RDONLY,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` was just returned by `open` and is owned by nobody else.
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    /// Opens `path` and proves it is the directory `assessed` describes:
    /// the same `(device, inode)` a read-only check observed. Any swap
    /// between that check and this open is refused here.
    pub fn open_verified(path: &Path, assessed: DirectoryIdentity) -> io::Result<Self> {
        let handle = Self::open(path)?;
        if handle.identity()? != assessed {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "directory is not the one that was assessed (replaced since the check)",
            ));
        }
        Ok(handle)
    }

    fn raw(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// `(device, inode)` of this very directory, via `fstat` on the
    /// descriptor - never re-resolved from a name.
    pub fn identity(&self) -> io::Result<DirectoryIdentity> {
        // SAFETY: zeroed `stat` is a valid out-parameter; the descriptor is live.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        check(unsafe { libc::fstat(self.raw(), &mut stat) })?;
        if (stat.st_mode & libc::S_IFMT) != libc::S_IFDIR {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            ));
        }
        Ok(DirectoryIdentity {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
        })
    }

    /// Opens the child directory `name`, refusing a symlink.
    pub fn open_child_dir(&self, name: &OsStr) -> io::Result<Self> {
        let c_name = component(name)?;
        // SAFETY: live descriptor, valid C string; `openat` returns an
        // owned fd or -1.
        let fd = unsafe {
            libc::openat(
                self.raw(),
                c_name.as_ptr(),
                libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_RDONLY,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh owned descriptor.
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    /// Creates the child directory `name` (mode 0o755 before umask) and
    /// opens it, refusing a symlink. `AlreadyExists` is returned unchanged
    /// so a caller can decide whether an existing child is acceptable.
    pub fn create_child_dir(&self, name: &OsStr) -> io::Result<Self> {
        let c_name = component(name)?;
        // SAFETY: live descriptor, valid C string.
        check(unsafe { libc::mkdirat(self.raw(), c_name.as_ptr(), 0o755) })?;
        self.open_child_dir(name)
    }

    /// Opens or creates the child directory `name`, proving in either case
    /// that what was opened is a real directory (never a symlink). Returns
    /// whether it was created.
    pub fn open_or_create_child_dir(&self, name: &OsStr) -> io::Result<(Self, bool)> {
        match self.create_child_dir(name) {
            Ok(dir) => Ok((dir, true)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.open_child_dir(name).map(|dir| (dir, false))
            }
            Err(error) => Err(error),
        }
    }

    /// `lstat` of `name` inside this directory (never following a symlink),
    /// or `None` when it does not exist.
    pub fn lstat(&self, name: &OsStr) -> io::Result<Option<EntryKind>> {
        let c_name = component(name)?;
        // SAFETY: zeroed `stat` out-parameter, live descriptor, valid C string.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::fstatat(
                self.raw(),
                c_name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            };
        }
        Ok(Some(match stat.st_mode & libc::S_IFMT {
            libc::S_IFREG => EntryKind::RegularFile {
                len: stat.st_size as u64,
            },
            libc::S_IFDIR => EntryKind::Directory,
            libc::S_IFLNK => EntryKind::Symlink,
            _ => EntryKind::Other,
        }))
    }

    /// Creates a brand-new regular file `name` (mode 0o644 before umask),
    /// failing if anything already has that name. Writes go through the
    /// returned [`File`]; the caller flushes and verifies it before
    /// publishing it under its final name.
    pub fn create_new_file(&self, name: &OsStr) -> io::Result<File> {
        let c_name = component(name)?;
        // SAFETY: live descriptor, valid C string; `openat` with O_CREAT
        // takes the mode as a third argument.
        let fd = unsafe {
            libc::openat(
                self.raw(),
                c_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o644 as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// Opens the regular file `name` for reading, refusing a symlink. The
    /// entry is checked with `lstat` first so a planted FIFO or device
    /// never blocks the open.
    pub fn open_file(&self, name: &OsStr) -> io::Result<File> {
        match self.lstat(name)? {
            Some(EntryKind::RegularFile { .. }) => {}
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "not a regular file",
                ));
            }
            None => return Err(io::Error::from(io::ErrorKind::NotFound)),
        }
        let c_name = component(name)?;
        // SAFETY: live descriptor, valid C string.
        let fd = unsafe {
            libc::openat(
                self.raw(),
                c_name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(file)
    }

    fn renameat2(&self, from: &OsStr, to: &OsStr, flags: libc::c_uint) -> io::Result<()> {
        let c_from = component(from)?;
        let c_to = component(to)?;
        // SAFETY: both names are valid C strings resolved against this live
        // descriptor; `renameat2` returns 0 or -1 with errno set.
        check(unsafe {
            libc::renameat2(
                self.raw(),
                c_from.as_ptr(),
                self.raw(),
                c_to.as_ptr(),
                flags,
            )
        })
    }

    /// Atomically renames `from` to `to` inside this directory, refusing to
    /// clobber anything already at `to` (`RENAME_NOREPLACE`).
    pub fn rename_no_replace(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        self.renameat2(from, to, libc::RENAME_NOREPLACE)
    }

    /// Atomically exchanges the two names (`RENAME_EXCHANGE`); both must
    /// exist. The displaced file keeps existing under the other name, so
    /// it can be verified afterwards and the exchange reversed if it is
    /// not what was expected.
    pub fn exchange(&self, left: &OsStr, right: &OsStr) -> io::Result<()> {
        self.renameat2(left, right, libc::RENAME_EXCHANGE)
    }

    /// Removes the non-directory entry `name` from this directory.
    pub fn unlink(&self, name: &OsStr) -> io::Result<()> {
        let c_name = component(name)?;
        // SAFETY: live descriptor, valid C string.
        check(unsafe { libc::unlinkat(self.raw(), c_name.as_ptr(), 0) })
    }

    /// Flushes this directory's own entries to stable storage, so a rename
    /// published here survives a crash.
    pub fn sync(&self) -> io::Result<()> {
        // SAFETY: live descriptor.
        check(unsafe { libc::fsync(self.raw()) })
    }
}

/// What [`DirectoryHandle::lstat`] found under a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    RegularFile { len: u64 },
    Directory,
    Symlink,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::unix::fs::symlink;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn name(value: &str) -> &OsStr {
        OsStr::new(value)
    }

    #[test]
    fn open_refuses_a_symlink_parent_and_verified_open_refuses_a_replaced_parent() {
        let temp = temp();
        let real = temp.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = temp.path().join("link");
        symlink(&real, &link).unwrap();
        assert!(
            DirectoryHandle::open(&link).is_err(),
            "symlink parent is never followed"
        );

        let assessed = DirectoryIdentity::of_metadata(&fs::symlink_metadata(&real).unwrap());
        assert!(DirectoryHandle::open_verified(&real, assessed).is_ok());
        // Swap the assessed directory for another one with the same name.
        fs::rename(&real, temp.path().join("moved")).unwrap();
        fs::create_dir(&real).unwrap();
        let error = DirectoryHandle::open_verified(&real, assessed).unwrap_err();
        assert!(error.to_string().contains("replaced"), "{error}");
    }

    /// D7-03: after the handle is open, swapping the parent directory for a
    /// symlink to somewhere else redirects nothing - every write, verify,
    /// rename and unlink still lands in the original directory.
    #[test]
    fn operations_stay_in_the_opened_directory_after_the_parent_is_swapped() {
        let temp = temp();
        let platform = temp.path().join("Atari2600");
        fs::create_dir(&platform).unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let handle = DirectoryHandle::open(&platform).unwrap();

        // The race: the platform directory becomes a symlink to `outside`.
        let moved = temp.path().join("Atari2600.moved");
        fs::rename(&platform, &moved).unwrap();
        symlink(&outside, &platform).unwrap();

        let mut file = handle.create_new_file(name(".tmp-1")).unwrap();
        file.write_all(b"cheats = 0\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        handle
            .rename_no_replace(name(".tmp-1"), name("Frogger.cht"))
            .unwrap();
        handle.sync().unwrap();

        assert_eq!(
            fs::read(moved.join("Frogger.cht")).unwrap(),
            b"cheats = 0\n"
        );
        assert!(
            fs::read_dir(&outside).unwrap().next().is_none(),
            "nothing may land where the symlink points"
        );
        let mut read_back = String::new();
        handle
            .open_file(name("Frogger.cht"))
            .unwrap()
            .read_to_string(&mut read_back)
            .unwrap();
        assert_eq!(read_back, "cheats = 0\n");
        assert_eq!(
            handle.lstat(name("Frogger.cht")).unwrap(),
            Some(EntryKind::RegularFile { len: 11 })
        );
        handle.unlink(name("Frogger.cht")).unwrap();
        assert!(!moved.join("Frogger.cht").exists());
        assert_eq!(handle.lstat(name("Frogger.cht")).unwrap(), None);
    }

    #[test]
    fn create_new_and_rename_no_replace_never_clobber() {
        let temp = temp();
        let handle = DirectoryHandle::open(temp.path()).unwrap();
        fs::write(temp.path().join("taken"), b"user file").unwrap();
        let error = handle.create_new_file(name("taken")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        let mut file = handle.create_new_file(name("new")).unwrap();
        file.write_all(b"ours").unwrap();
        drop(file);
        let error = handle
            .rename_no_replace(name("new"), name("taken"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(temp.path().join("taken")).unwrap(), b"user file");
        assert_eq!(fs::read(temp.path().join("new")).unwrap(), b"ours");
    }

    #[test]
    fn exchange_swaps_both_names_atomically_and_can_be_reversed() {
        let temp = temp();
        let handle = DirectoryHandle::open(temp.path()).unwrap();
        fs::write(temp.path().join("live"), b"old").unwrap();
        fs::write(temp.path().join("staged"), b"new").unwrap();
        handle.exchange(name("staged"), name("live")).unwrap();
        assert_eq!(fs::read(temp.path().join("live")).unwrap(), b"new");
        assert_eq!(fs::read(temp.path().join("staged")).unwrap(), b"old");
        handle.exchange(name("staged"), name("live")).unwrap();
        assert_eq!(fs::read(temp.path().join("live")).unwrap(), b"old");
        assert!(handle.exchange(name("live"), name("missing")).is_err());
    }

    #[test]
    fn child_directory_helpers_refuse_symlinks_and_report_creation() {
        let temp = temp();
        let handle = DirectoryHandle::open(temp.path()).unwrap();
        let (created, was_created) = handle.open_or_create_child_dir(name("Atari2600")).unwrap();
        assert!(was_created);
        assert!(created.identity().is_ok());
        let (_again, was_created) = handle.open_or_create_child_dir(name("Atari2600")).unwrap();
        assert!(!was_created);
        symlink(temp.path(), temp.path().join("loop")).unwrap();
        assert!(handle.open_child_dir(name("loop")).is_err());
        assert!(handle.open_or_create_child_dir(name("loop")).is_err());
        assert_eq!(
            handle.lstat(name("loop")).unwrap(),
            Some(EntryKind::Symlink)
        );
        fs::write(temp.path().join("plain"), b"").unwrap();
        assert!(handle.open_child_dir(name("plain")).is_err());
        assert!(
            handle.open_file(name("Atari2600")).is_err(),
            "a directory is not a file"
        );
    }

    #[test]
    fn names_must_be_single_components() {
        let temp = temp();
        let handle = DirectoryHandle::open(temp.path()).unwrap();
        for bad in ["", ".", "..", "a/b", "../escape"] {
            assert!(handle.create_new_file(name(bad)).is_err(), "{bad:?}");
            assert!(handle.lstat(name(bad)).is_err(), "{bad:?}");
            assert!(handle.unlink(name(bad)).is_err(), "{bad:?}");
        }
    }
}
