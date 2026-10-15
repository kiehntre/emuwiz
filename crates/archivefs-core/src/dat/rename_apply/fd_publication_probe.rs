//! Bounded Linux experiment. Only compiled into core unit tests.
//! Publication creates an additional hardlink; it never completes an Undo.
//! No AT_EMPTY_PATH, pathname-source fallback, unlink, or automatic cleanup.

use super::{capture_identity, identity_matches, require_same_snapshot};
use crate::dat::rename_apply::{ObjectIdentity, ObjectKind, dirsync::RenameDirs};
use crate::safe_read::TrustedRoots;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

fn basename(path: &Path) -> io::Result<CString> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("missing basename"))?;
    CString::new(name.as_bytes()).map_err(io::Error::other)
}

/// The ONLY publication syscall. The caller owns both descriptors throughout
/// the call. /proc/self/fd/N resolves the open file description, not its old
/// directory entry. A last-link deletion can make this return ENOENT.
fn link_descriptor(file: &File, directory: &File, name: &CStr) -> io::Result<()> {
    let bytes = name.to_bytes();
    if bytes.is_empty() || bytes.contains(&b'/') || bytes == b"." || bytes == b".." {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let source = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap();
    // SAFETY: both FDs are owned, live File values; both C strings outlive the
    // syscall. No other thread is given authority to close/reuse these FDs.
    let rc = unsafe {
        libc::linkat(
            libc::AT_FDCWD,
            source.as_ptr(),
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::AT_SYMLINK_FOLLOW,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[derive(Debug)]
enum Publication {
    /// The linked object/content and directory bindings passed postchecks.
    /// This is a point-in-time observation, NOT complete rename or exclusion.
    LinkedVerified,
    /// The syscall returned an error. No blanket "unchanged" claim for I/O
    /// errors; tests establish specific EEXIST/ENOENT/EXDEV outcomes.
    LinkError(io::Error),
    /// linkat succeeded; a subsequent sync/binding/content check failed.
    LinkedNeedsReview(String),
}

struct Publisher {
    file: File,
    destination_dir: File,
    destination_name: CString,
    dirs: RenameDirs,
    expected: ObjectIdentity,
}

impl Publisher {
    fn open(
        root: &Path,
        source: &Path,
        destination: &Path,
        expected: &ObjectIdentity,
    ) -> io::Result<Self> {
        if expected.kind != ObjectKind::RegularFile {
            return Err(io::Error::other("only reviewed regular files"));
        }
        let dirs = RenameDirs::open_in(source, destination, &TrustedRoots::from_paths([root]))?;
        // These parents are /proc/self/fd/N paths supplied by RenameDirs,
        // whose root-relative O_NOFOLLOW walk already pinned the directories.
        let source_entry = dirs.source_entry();
        let source_dir = File::open(source_entry.parent().unwrap())?;
        let destination_dir = File::open(dirs.destination_entry().parent().unwrap())?;
        let source_name = basename(source)?;
        // SAFETY: live directory FD and NUL-terminated single basename.
        let raw = unsafe {
            libc::openat(
                source_dir.as_raw_fd(),
                source_name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat transferred this fresh FD to us.
        let file = unsafe { File::from_raw_fd(raw) };
        let before = file.metadata()?;
        if !before.is_file() || (before.dev(), before.ino()) != (expected.dev, expected.ino) {
            return Err(io::Error::other("opened object differs from approval"));
        }
        // Reuse the full existing SHA256/mtime/object proof. Its read must be
        // of the pinned inode, and that inode's snapshot must remain stable
        // across capture. The retained FD prevents inode reuse in this interval.
        let observed = capture_identity(&source_entry)?;
        require_same_snapshot(&before, &file.metadata()?)?;
        if !identity_matches(expected, &observed) {
            return Err(io::Error::other("source proof differs from approval"));
        }
        dirs.verify_bound()?;
        Ok(Self {
            file,
            destination_dir,
            destination_name: basename(destination)?,
            dirs,
            expected: expected.clone(),
        })
    }

    fn publish(&self, immediately_before_syscall: impl FnOnce()) -> io::Result<Publication> {
        self.dirs.verify_bound()?;
        immediately_before_syscall();
        if let Err(error) =
            link_descriptor(&self.file, &self.destination_dir, &self.destination_name)
        {
            return Ok(Publication::LinkError(error));
        }
        // Nothing after successful linkat may report that no mutation happened.
        let postcheck = || -> io::Result<()> {
            self.file.sync_all()?;
            self.dirs.sync_and_verify(false)?;
            if !identity_matches(
                &self.expected,
                &capture_identity(&self.dirs.destination_entry())?,
            ) {
                return Err(io::Error::other(
                    "published inode/content no longer matches approval",
                ));
            }
            Ok(())
        };
        Ok(match postcheck() {
            Ok(()) => Publication::LinkedVerified,
            Err(e) => Publication::LinkedNeedsReview(e.to_string()),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Retirement {
    SourceMissing,
    ExpectedAnchorRetained,
    ForeignLinkedBackNeedsReview,
    ManualReview,
}

/// Capture and inspect the old entry, but NEVER unlink it after verification:
/// unlinkat is still name-based and cannot consume an expected-inode condition.
/// Even the cooperative case retains an anchor and is not a complete Undo.
fn retain_old_name(
    root: &Path,
    old: &Path,
    holding: &Path,
    expected: &ObjectIdentity,
    after_capture: impl FnOnce(),
    before_return: impl FnOnce(),
) -> io::Result<Retirement> {
    let dirs = RenameDirs::open_in(old, holding, &TrustedRoots::from_paths([root]))?;
    if let Err(e) = dirs.rename_noreplace(false) {
        if matches!(std::fs::symlink_metadata(old), Err(ref e) if e.kind() == io::ErrorKind::NotFound)
        {
            return Ok(Retirement::SourceMissing);
        }
        return Err(io::Error::other(format!("capture refused: {e:?}")));
    }
    dirs.sync_and_verify(false)?;
    after_capture();
    let actual = capture_identity(&dirs.destination_entry())?;
    if identity_matches(expected, &actual) {
        return Ok(Retirement::ExpectedAnchorRetained);
    }
    // Return only via the captured object's pinned FD. A successful return
    // adds an alias; the holding entry remains for review, without any unlink.
    let returned = match Publisher::open(root, holding, old, &actual) {
        Ok(p) => p.publish(before_return)?,
        Err(_) => return Ok(Retirement::ManualReview),
    };
    Ok(match returned {
        Publication::LinkedVerified => Retirement::ForeignLinkedBackNeedsReview,
        _ => Retirement::ManualReview,
    })
}

#[path = "fd_publication_probe_tests.rs"]
mod tests;

#[path = "fd_contract.rs"]
pub(crate) mod contract;
