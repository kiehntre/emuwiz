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
        resolve: NO_SYMLINKS | if owned { BENEATH | NO_XDEV } else { 0 },
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
