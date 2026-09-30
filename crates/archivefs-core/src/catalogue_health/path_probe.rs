//! Linux openat2 resolves all components atomically with no symlink traversal.
//! Owned probes are relative to a pinned source descriptor, forbid mount
//! crossings, and recheck the root pathname before returning any evidence.
use super::{FsProbe, PathObservation, SourceRootBinding};
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Component, Path, PathBuf};

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}
const NO_SYMLINKS: u64 = 0x04;
const BENEATH: u64 = 0x08;
const NO_XDEV: u64 = 0x01;

fn open(fd: i32, path: &Path, read: bool, owned: bool) -> io::Result<File> {
    open_resolving(
        fd,
        path,
        read,
        NO_SYMLINKS | if owned { BENEATH | NO_XDEV } else { 0 },
    )
}

fn open_resolving(fd: i32, path: &Path, read: bool, resolve: u64) -> io::Result<File> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let name =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| io::ErrorKind::InvalidInput)?;
    let how = OpenHow {
        flags: (if read {
            libc::O_RDONLY | libc::O_NONBLOCK
        } else {
            libc::O_PATH
        } | libc::O_NOFOLLOW
            | libc::O_CLOEXEC) as u64,
        mode: 0,
        resolve,
    };
    // SAFETY: arguments reference live NUL terminated/path and repr(C) storage;
    // on success File takes sole ownership. ENOSYS fails closed: no fallback.
    let result = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            fd,
            name.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(result as i32) };
    Ok(file)
}

fn binding(file: &File) -> io::Result<SourceRootBinding> {
    let m = file.metadata()?;
    if !m.is_dir() {
        return Err(io::ErrorKind::NotADirectory.into());
    }
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: a live descriptor and writable correctly sized statfs storage.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stats = unsafe { stats.assume_init() };
    // fsid_t is opaque. Preserve its initialized bytes, not a guessed layout.
    let fsid = unsafe {
        std::slice::from_raw_parts(
            (&stats.f_fsid as *const libc::fsid_t).cast::<u8>(),
            std::mem::size_of::<libc::fsid_t>(),
        )
    }
    .to_vec();
    Ok(SourceRootBinding {
        device: m.dev(),
        inode: m.ino(),
        filesystem_type: stats.f_type as i64,
        filesystem_id: fsid,
    })
}

fn observation(file: io::Result<File>, directory: bool) -> PathObservation {
    let mut result = unsafe_observation();
    match file.and_then(|f| f.metadata()) {
        Ok(m) => {
            result.probe = if m.file_type().is_symlink() {
                FsProbe::Symlink
            } else if directory && m.is_dir() {
                FsProbe::PresentDirectory
            } else if !directory && m.is_file() {
                FsProbe::PresentFile
            } else {
                FsProbe::WrongType
            };
            result.size = Some(m.len());
            result.modified = Some(m.mtime());
            result.modified_ns = Some(m.mtime_nsec());
            result.binding.push((m.dev(), m.ino()));
        }
        Err(e) => {
            result.probe = match e.raw_os_error() {
                Some(libc::ELOOP) => FsProbe::Symlink,
                Some(libc::ENOENT) => FsProbe::Missing,
                Some(libc::ENOTDIR) => FsProbe::WrongType,
                Some(libc::EACCES | libc::EPERM) => FsProbe::Inaccessible,
                _ => FsProbe::IoError,
            }
        }
    }
    result
}

pub(super) fn probe(path: &Path, directory: bool) -> PathObservation {
    if !path.is_absolute() {
        return unsafe_observation();
    }
    let first = observation(open(libc::AT_FDCWD, path, false, false), directory);
    let second = observation(open(libc::AT_FDCWD, path, false, false), directory);
    if first == second {
        first
    } else {
        unsafe_observation()
    }
}

/// Kernel mount identity of one directory. IDs are unique per mount instance
/// (`STATX_MNT_ID_UNIQUE`) where supported, so a bind mount on the same device
/// differs from the directory beneath which it is mounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MountObservation {
    pub(crate) mount_id: u64,
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NestedState {
    pub(crate) binding: SourceRootBinding,
    pub(crate) mount_id: u64,
    pub(crate) parent_mount_id: u64,
}
impl NestedState {
    pub(crate) fn is_mount_root(&self) -> bool {
        self.mount_id != self.parent_mount_id
    }
}

const STATX_INO: u32 = 0x100;
const STATX_MNT_ID: u32 = 0x1000;
const STATX_MNT_ID_UNIQUE: u32 = 0x4000;

fn statx_mount(fd: i32, path: &std::ffi::CStr, flags: i32) -> io::Result<MountObservation> {
    let mut out = std::mem::MaybeUninit::<libc::statx>::zeroed();
    // SAFETY: valid NUL-terminated path and writable statx storage.
    let rc = unsafe {
        libc::statx(
            fd,
            path.as_ptr(),
            flags,
            STATX_INO | STATX_MNT_ID | STATX_MNT_ID_UNIQUE,
            out.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let out = unsafe { out.assume_init() };
    // A kernel without mount IDs must fail closed, never guess "no boundary".
    if out.stx_mask & (STATX_MNT_ID | STATX_MNT_ID_UNIQUE) == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "kernel does not report mount IDs",
        ));
    }
    Ok(MountObservation {
        mount_id: out.stx_mnt_id,
        device: libc::makedev(out.stx_dev_major, out.stx_dev_minor),
        inode: out.stx_ino,
    })
}

fn mount_of(file: &File) -> io::Result<MountObservation> {
    statx_mount(
        file.as_raw_fd(),
        c"",
        libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
    )
}

/// Mount identity of a directory by pathname (no final symlink follow).
pub(crate) fn directory_mount(path: &Path) -> io::Result<MountObservation> {
    let name =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| io::ErrorKind::InvalidInput)?;
    statx_mount(libc::AT_FDCWD, &name, libc::AT_SYMLINK_NOFOLLOW)
}

pub(crate) struct BoundRoot {
    file: File,
    path: PathBuf,
    pub(crate) identity: (u64, u64),
    pub(crate) binding: SourceRootBinding,
}
impl BoundRoot {
    pub(crate) fn open(path: &Path) -> Option<Self> {
        if !path.is_absolute() {
            return None;
        }
        let file = open(libc::AT_FDCWD, path, false, false).ok()?;
        let binding = binding(&file).ok()?;
        let id = (binding.device, binding.inode);
        Some(Self {
            file,
            path: path.to_path_buf(),
            identity: id,
            binding,
        })
    }
    pub(crate) fn current(&self) -> bool {
        open(libc::AT_FDCWD, &self.path, false, false)
            .and_then(|f| binding(&f))
            .ok()
            == Some(self.binding.clone())
    }
    fn target(&self, path: &Path, read: bool) -> io::Result<File> {
        let relative = path
            .strip_prefix(&self.path)
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        // Empty relative path is the source root itself.
        open(
            self.file.as_raw_fd(),
            if relative.as_os_str().is_empty() {
                Path::new(".")
            } else {
                relative
            },
            read,
            true,
        )
    }
    pub(crate) fn probe(&self, path: &Path, directory: bool) -> PathObservation {
        let first = observation(self.target(path, false), directory);
        let second = observation(self.target(path, false), directory);
        if first == second && self.current() {
            first
        } else {
            unsafe_observation()
        }
    }
    /// Metadata of one atomically resolved, no-symlink target. A final
    /// symlink yields link metadata, never its target's.
    /// Identity of a directory beneath this root, resolved without symlinks but
    /// *allowing* mount crossings, plus whether it is a mount root of its own
    /// (its mount ID differs from its parent directory's). The mount ID is
    /// what recognises same-device bind mounts, which share `st_dev`.
    pub(crate) fn nested_state(&self, path: &Path) -> io::Result<NestedState> {
        let relative = path
            .strip_prefix(&self.path)
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        let resolve = NO_SYMLINKS | BENEATH;
        let file = open_resolving(self.file.as_raw_fd(), relative, false, resolve)?;
        let binding = binding(&file)?;
        let mount = mount_of(&file)?;
        let parent = match relative.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => mount_of(&open_resolving(
                self.file.as_raw_fd(),
                parent,
                false,
                resolve,
            )?)?,
            _ => mount_of(&self.file)?,
        };
        if self.current() {
            Ok(NestedState {
                binding,
                mount_id: mount.mount_id,
                parent_mount_id: parent.mount_id,
            })
        } else {
            Err(io::ErrorKind::Other.into())
        }
    }
    pub(crate) fn root_path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn metadata(&self, path: &Path) -> io::Result<std::fs::Metadata> {
        self.target(path, false)?.metadata()
    }
    pub(super) fn read_file(&self, path: &Path, expected: &PathObservation) -> Option<File> {
        let file = self.target(path, true).ok()?;
        let current = observation(file.try_clone().map_err(io::Error::other), false);
        (current == *expected && self.current()).then_some(file)
    }
}

pub(super) fn unsafe_observation() -> PathObservation {
    PathObservation {
        probe: FsProbe::IoError,
        size: None,
        modified: None,
        binding: Vec::new(),
        modified_ns: None,
    }
}
