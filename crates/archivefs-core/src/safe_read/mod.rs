//! The one bounded, read-only file-open policy this build has.
//!
//! Two subsystems need to read a handful of bytes from a game file to identify
//! it: platform signature detection ([`crate::platform::detect`]) and disc
//! identity ([`crate::game_identity`]). They used to carry *different* symlink
//! policies, and neither was right:
//!
//! - `game_identity` walked every path component and refused if any of them was
//!   a symlink. Strict and safe, but it meant a symlinked library - which is how
//!   real libraries are usually arranged - could never be identified at all.
//! - platform detection only checked the final component, then used
//!   `File::open`, which happily follows symlinks in *parent* directories. So it
//!   was simultaneously more permissive and less deliberate.
//!
//! This module replaces both with one policy, built on the stricter of the two
//! and extended with an explicit, opt-in way to follow a symlink safely.
//!
//! # The policy
//!
//! A read is permitted only when every one of these holds:
//!
//! 1. The path is absolute and contains no `.` or `..` component.
//! 2. If the path is **not** a symlink: no component of it may be a symlink
//!    either, exactly as before. This is the unchanged, default behaviour.
//! 3. If the path **is** a symlink, it may be followed only when:
//!    a. at least one trusted root is configured (absent roots means refuse -
//!    see [`TrustedRoots::none`]);
//!    b. the symlink itself lies inside a trusted root;
//!    c. the target canonicalises successfully - which is what rules out a
//!    broken link and a symlink loop, since `canonicalize` reports both as
//!    errors;
//!    d. the canonical target also lies inside a trusted root;
//!    e. the canonical target is a regular file - so a directory, FIFO, socket,
//!    device or anything else is refused.
//! 4. The file is opened by walking the path one component at a time from
//!    `/` with `openat(O_DIRECTORY | O_NOFOLLOW)`, and the leaf with
//!    `openat(O_NOFOLLOW | O_NONBLOCK | O_NOCTTY)` on the directory
//!    descriptor just obtained. Every step is a kernel-level "no symlink
//!    here" check on the descriptor actually in hand, so a directory swapped
//!    for a symlink *between* a check and the open (Night Watch S01-02) is
//!    refused rather than followed, and the file type is taken from the open
//!    descriptor itself. A path-based pre-check cannot promise either.
//!
//! # Fail-closed
//!
//! [`TrustedRoots::none`] refuses every symlink. Every caller that does not
//! deliberately supply roots therefore behaves exactly as this build did
//! before, and gaining the capability requires passing the roots in.
//!
//! # What this module never does
//!
//! No write, no create, no permission or timestamp change, no rename, no
//! delete, no hashing, no mounting, no extraction, no process, no network.
//! Reads are positional and bounded by the caller's own limit. A resolved
//! symlink target is never returned as a write destination - the only thing
//! that leaves here is a read-only [`SafeFile`].

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

/// The configured source roots a symlink target is allowed to resolve into.
///
/// Canonicalised once at construction, because comparing a canonical target
/// against a non-canonical root would be the obvious way to get containment
/// wrong. A root that cannot be canonicalised (it does not exist, or is not
/// readable) is dropped rather than trusted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedRoots {
    roots: Vec<PathBuf>,
}

impl TrustedRoots {
    /// No trusted roots: every symlink is refused. This is the default and the
    /// behaviour every pre-existing caller keeps.
    pub fn none() -> Self {
        Self { roots: Vec::new() }
    }

    /// Builds a trusted set from configured source roots.
    ///
    /// Only absolute, existing, canonicalisable directories are kept. A
    /// relative or missing root is silently dropped: trusting a path that
    /// cannot be resolved would make containment unprovable.
    pub fn from_paths<I, P>(paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut roots: Vec<PathBuf> = paths
            .into_iter()
            .filter_map(|path| {
                let path = path.as_ref();
                if !path.is_absolute() {
                    return None;
                }
                let canonical = std::fs::canonicalize(path).ok()?;
                std::fs::symlink_metadata(&canonical)
                    .ok()
                    .filter(|metadata| metadata.is_dir())
                    .map(|_| canonical)
            })
            .collect();
        roots.sort();
        roots.dedup();
        Self { roots }
    }

    /// The configured source folders of `config`, as a trusted set.
    pub fn from_config(config: &crate::Config) -> Self {
        Self::from_paths(&config.source_folders)
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Whether `candidate` - which must already be canonical - lies inside one
    /// of the trusted roots.
    ///
    /// Compares whole path components via `starts_with`, so `/mnt/roms-backup`
    /// is not treated as being inside `/mnt/roms`.
    pub fn contains_canonical(&self, candidate: &Path) -> bool {
        self.roots.iter().any(|root| candidate.starts_with(root))
    }
}

/// Why a read was refused. Each variant is a distinct, explainable reason
/// rather than one opaque failure, because "the target left your library" and
/// "the target is a directory" call for different responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafeReadRefusal {
    /// The path was relative, so containment could not be established.
    NotAbsolute,
    /// The path contained a `.` or `..` component.
    NonNormalComponent,
    /// A component of a non-symlink path was itself a symlink.
    SymlinkInPath(PathBuf),
    /// The path, or a component of it, could not be stat'ed.
    Unreadable(String),
    /// The target is not a regular file: a directory, FIFO, socket, device,
    /// or anything else.
    NotRegularFile,
    /// A symlink was found but no trusted root is configured, so following it
    /// could not be justified.
    NoTrustedRoots,
    /// The symlink itself is outside every trusted root.
    SymlinkOutsideTrustedRoots,
    /// The symlink resolved to something outside every trusted root.
    TargetOutsideTrustedRoots,
    /// The target could not be canonicalised: it is missing, it forms a loop,
    /// or it is not permitted. `canonicalize` reports all three the same way,
    /// so the underlying message is carried through verbatim.
    UnresolvableTarget(String),
    /// The file changed between validation and opening.
    ChangedWhileOpening,
    /// The file could not be opened.
    OpenFailed(String),
}

impl SafeReadRefusal {
    /// A short explanation suitable for evidence or a diagnostic.
    pub fn detail(&self) -> String {
        match self {
            Self::NotAbsolute => "the path is not absolute".to_string(),
            Self::NonNormalComponent => "the path contains a `.` or `..` component".to_string(),
            Self::SymlinkInPath(component) => {
                format!("symlink refused: {}", component.display())
            }
            Self::Unreadable(detail) => detail.clone(),
            Self::NotRegularFile => "the target is not a regular file".to_string(),
            // These four keep the historical "symlink refused" wording so a
            // diagnostic that already looked for it still reads correctly, and
            // each adds the reason rather than leaving it unexplained.
            Self::NoTrustedRoots => {
                "symlink refused: no trusted source root is configured".to_string()
            }
            Self::SymlinkOutsideTrustedRoots => {
                "symlink refused: the link is outside every configured source root".to_string()
            }
            Self::TargetOutsideTrustedRoots => {
                "symlink refused: it resolves outside every configured source root".to_string()
            }
            Self::UnresolvableTarget(detail) => {
                format!("symlink refused: the target could not be resolved: {detail}")
            }
            Self::ChangedWhileOpening => "the file changed while it was being opened".to_string(),
            Self::OpenFailed(detail) => detail.clone(),
        }
    }

    /// A stable, machine-readable reason, for audit counting.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAbsolute => "not_absolute",
            Self::NonNormalComponent => "non_normal_component",
            Self::SymlinkInPath(_) => "symlink_in_path",
            Self::Unreadable(_) => "unreadable",
            Self::NotRegularFile => "not_regular_file",
            Self::NoTrustedRoots => "no_trusted_roots",
            Self::SymlinkOutsideTrustedRoots => "symlink_outside_trusted_roots",
            Self::TargetOutsideTrustedRoots => "target_outside_trusted_roots",
            Self::UnresolvableTarget(_) => "unresolvable_target",
            Self::ChangedWhileOpening => "changed_while_opening",
            Self::OpenFailed(_) => "open_failed",
        }
    }
}

/// A validated, read-only handle. The only thing this module hands out.
#[derive(Debug)]
pub struct SafeFile {
    file: File,
    length: u64,
    resolved_via_symlink: bool,
    /// Test-only: lets the read fault hook recognise the file.
    #[cfg(test)]
    path: PathBuf,
}

#[cfg(test)]
thread_local! {
    /// Test-only fault injection: reads through [`SafeFile::read_exact_at`]
    /// of a file whose path ends with the given suffix fail once they reach
    /// at or beyond the given byte offset, so a test can stage "the first
    /// bytes read fine, then the device failed" deterministically.
    pub(crate) static FAIL_READS_AT_OR_BEYOND: std::cell::RefCell<Option<(String, u64)>> =
        const { std::cell::RefCell::new(None) };
}

impl SafeFile {
    /// The file's length, as observed during validation.
    pub fn len(&self) -> u64 {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Whether the path given was a symlink that resolved safely. Used to say
    /// so in evidence, without exposing where it pointed.
    pub fn resolved_via_symlink(&self) -> bool {
        self.resolved_via_symlink
    }

    /// The underlying handle, for callers that do their own bounded reading
    /// (`game_identity` reads structured records, not fixed-offset magic).
    /// Read-only: the handle was opened without write access.
    pub fn into_file(self) -> File {
        self.file
    }

    /// Reads exactly `length` bytes at `offset`.
    ///
    /// Returns `None` rather than reading anything when `length` exceeds
    /// `max_length`, or when the range would run past the end of the file, so
    /// a bound is enforced before any I/O happens.
    pub fn read_exact_at(
        &mut self,
        offset: u64,
        length: usize,
        max_length: usize,
    ) -> Option<Vec<u8>> {
        if length == 0 || length > max_length {
            return None;
        }
        if offset.saturating_add(length as u64) > self.length {
            return None;
        }
        #[cfg(test)]
        {
            let injected = FAIL_READS_AT_OR_BEYOND.with(|fault| {
                fault.borrow().as_ref().is_some_and(|(suffix, at)| {
                    self.path.to_string_lossy().ends_with(suffix.as_str())
                        && offset.saturating_add(length as u64) > *at
                })
            });
            if injected {
                return None;
            }
        }
        self.file.seek(SeekFrom::Start(offset)).ok()?;
        let mut buffer = vec![0_u8; length];
        self.file.read_exact(&mut buffer).ok()?;
        Some(buffer)
    }
}

/// Opens `path` for bounded reading under the policy documented on this module.
///
/// `trusted` decides whether a symlink may be followed at all. Pass
/// [`TrustedRoots::none`] to keep the historical behaviour of refusing every
/// symlink.
pub fn open_bounded_read(path: &Path, trusted: &TrustedRoots) -> Result<SafeFile, SafeReadRefusal> {
    if !path.is_absolute() {
        return Err(SafeReadRefusal::NotAbsolute);
    }
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => {}
            _ => return Err(SafeReadRefusal::NonNormalComponent),
        }
    }

    let link_metadata = std::fs::symlink_metadata(path)
        .map_err(|error| SafeReadRefusal::Unreadable(format!("{}: {error}", path.display())))?;

    let target = if link_metadata.file_type().is_symlink() {
        resolve_trusted_symlink(path, trusted)?
    } else {
        // Unchanged behaviour: every component must be a real directory, so
        // nothing was reached through a link the caller did not see.
        refuse_symlinked_components(path)?;
        if !link_metadata.is_file() {
            return Err(SafeReadRefusal::NotRegularFile);
        }
        path.to_path_buf()
    };

    open_validated(&target, link_metadata.file_type().is_symlink())
}

/// Resolves a symlink under the trusted-root rules and returns the canonical
/// target. Every refusal here is deliberate and named.
fn resolve_trusted_symlink(
    path: &Path,
    trusted: &TrustedRoots,
) -> Result<PathBuf, SafeReadRefusal> {
    if trusted.is_empty() {
        return Err(SafeReadRefusal::NoTrustedRoots);
    }

    // The symlink itself must be inside the library. Its *parent* is
    // canonicalised rather than the link, because canonicalising the link
    // would resolve it and answer the wrong question.
    let parent = path.parent().ok_or(SafeReadRefusal::NotAbsolute)?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|error| SafeReadRefusal::Unreadable(format!("{}: {error}", parent.display())))?;
    if !trusted.contains_canonical(&canonical_parent) {
        return Err(SafeReadRefusal::SymlinkOutsideTrustedRoots);
    }

    // Canonicalising is what rules out a broken link and a symlink loop: the
    // operating system reports ENOENT and ELOOP respectively, and neither is
    // treated as a resolvable target.
    let canonical_target = std::fs::canonicalize(path)
        .map_err(|error| SafeReadRefusal::UnresolvableTarget(error.to_string()))?;

    if !trusted.contains_canonical(&canonical_target) {
        return Err(SafeReadRefusal::TargetOutsideTrustedRoots);
    }
    // A canonical path contains no symlinks by construction, so this checks
    // what the link actually pointed at.
    let target_metadata = std::fs::symlink_metadata(&canonical_target).map_err(|error| {
        SafeReadRefusal::Unreadable(format!("{}: {error}", canonical_target.display()))
    })?;
    if !target_metadata.is_file() {
        // Covers a directory, FIFO, socket, block or character device, and
        // anything else that is not a plain file.
        return Err(SafeReadRefusal::NotRegularFile);
    }
    Ok(canonical_target)
}

/// Legacy path-based pre-check, kept for the error wording tests and for
/// the non-Unix build: refuses when any component of `path` is itself a
/// symlink. On Unix the descriptor walk in [`open_validated`] is the check
/// that counts; this only provides the earlier, more specific refusal.
fn refuse_symlinked_components(path: &Path) -> Result<(), SafeReadRefusal> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => current.push(Path::new("/")),
            Component::Normal(component) => current.push(component),
            _ => return Err(SafeReadRefusal::NonNormalComponent),
        }
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            SafeReadRefusal::Unreadable(format!("{}: {error}", current.display()))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(SafeReadRefusal::SymlinkInPath(current));
        }
    }
    Ok(())
}

/// Opens the directory `path` without following a single symlink anywhere
/// in it, by walking from `/` one component at a time on directory
/// descriptors. `path` must be absolute with only normal components.
///
/// The returned handle is the directory itself, usable with `openat`-style
/// operations by callers that must create or publish *relative to a
/// directory they have already verified* (the archive stage publisher does
/// this), instead of re-resolving a path that may have changed.
#[cfg(unix)]
pub fn open_directory_no_symlinks(path: &Path) -> Result<File, SafeReadRefusal> {
    if !path.is_absolute() {
        return Err(SafeReadRefusal::NotAbsolute);
    }
    let mut current = PathBuf::from("/");
    let mut directory = open_root()?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                current.push(name);
                directory = open_child_directory(&directory, name, &current)?;
            }
            _ => return Err(SafeReadRefusal::NonNormalComponent),
        }
    }
    Ok(directory)
}

#[cfg(unix)]
fn open_root() -> Result<File, SafeReadRefusal> {
    let mut options = OpenOptions::new();
    // Rust requires an access mode; with `O_PATH` the kernel ignores it.
    options.read(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        // `O_PATH`: a position in the tree, not an opened directory. It needs
        // no read permission and raises no inotify open event, which matters
        // because the walk passes through directories (a library parent, a
        // search-only folder) that this code must not list or "open".
        options.custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC);
    }
    options
        .open("/")
        .map_err(|error| SafeReadRefusal::Unreadable(format!("/: {error}")))
}

/// One step of the walk: `name` inside `directory`, which must be a real
/// directory (not a symlink to one). With `O_NOFOLLOW | O_DIRECTORY` the
/// kernel refuses a symlink in that place (Linux reports `ENOTDIR`, some
/// systems `ELOOP`); the refusal is then named by asking the directory
/// descriptor what the entry is, so the caller can say "symlink" rather
/// than "unreadable" - the open itself never followed anything.
#[cfg(unix)]
fn open_child_directory(
    directory: &File,
    name: &std::ffi::OsStr,
    reached: &Path,
) -> Result<File, SafeReadRefusal> {
    // `O_PATH` for every intermediate directory: search permission (`x`) is
    // enough, no open event is generated, and `O_NOFOLLOW` still refuses a
    // symlink in the component's place. An `O_PATH` descriptor is valid as the
    // `dirfd` of `openat`, `fstatat` and `renameat2`, which is all the walk and
    // its callers use it for.
    openat(
        directory,
        name,
        libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    )
    .map_err(|error| {
        if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR))
            && entry_is_symlink(directory, name)
        {
            return SafeReadRefusal::SymlinkInPath(reached.to_path_buf());
        }
        SafeReadRefusal::Unreadable(format!("{}: {error}", reached.display()))
    })
}

/// Whether `name` inside the open `directory` is a symlink right now
/// (`fstatat` with `AT_SYMLINK_NOFOLLOW`). Diagnostic only: the refusal was
/// already decided by the kernel's open.
#[cfg(unix)]
fn entry_is_symlink(directory: &File, name: &std::ffi::OsStr) -> bool {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let Ok(name) = std::ffi::CString::new(name.as_bytes()) else {
        return false;
    };
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `directory` is a live descriptor, `name` a valid C string and
    // `stat` a writable, correctly sized struct.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    result == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFLNK
}

/// Opens the subdirectory `name` of an already-open directory handle without
/// following a symlink in its place (`ELOOP` becomes an error). `name` must
/// be one plain component. This is the one descriptor-relative primitive
/// handed to callers that stage and publish inside a directory they pinned
/// with [`open_directory_no_symlinks`].
#[cfg(unix)]
pub fn open_subdirectory_no_follow(
    directory: &File,
    name: &std::ffi::OsStr,
) -> std::io::Result<File> {
    openat(
        directory,
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    )
}

/// `openat(2)` of one single-component `name` relative to `directory`.
#[cfg(unix)]
fn openat(directory: &File, name: &std::ffi::OsStr, flags: libc::c_int) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.contains(&b'/') || bytes == b"." || bytes == b".." {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "openat name must be one plain path component",
        ));
    }
    let name = std::ffi::CString::new(bytes)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in name"))?;
    // SAFETY: `directory` is a live descriptor owned by the caller for the
    // duration of the call and `name` is a valid NUL-terminated string.
    let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by `openat` and is owned by nothing else.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Opens an already-validated, already-canonical path read-only, without
/// following a symlink at any component, and takes the file's type and
/// length from the open descriptor itself.
#[cfg(unix)]
fn open_validated(target: &Path, resolved_via_symlink: bool) -> Result<SafeFile, SafeReadRefusal> {
    let before = std::fs::symlink_metadata(target)
        .map_err(|error| SafeReadRefusal::Unreadable(format!("{}: {error}", target.display())))?;
    if before.file_type().is_symlink() {
        // The caller passes a canonical or already-vetted path; a symlink in
        // its place now means it changed underneath them.
        return Err(SafeReadRefusal::SymlinkInPath(target.to_path_buf()));
    }
    if !before.is_file() {
        return Err(SafeReadRefusal::NotRegularFile);
    }
    let parent = target.parent().ok_or(SafeReadRefusal::NotAbsolute)?;
    let leaf = target.file_name().ok_or(SafeReadRefusal::NotAbsolute)?;
    let directory = open_directory_no_symlinks(parent)?;
    // `O_NOFOLLOW` refuses a leaf that became a symlink. `O_NONBLOCK` closes
    // the race between the regular-file check above and this open: a path
    // swapped for a FIFO opens at once (instead of waiting forever for a
    // writer) and is then refused below. It is cleared again once the file
    // is proven regular, so readers see ordinary blocking reads. `O_NOCTTY`
    // keeps a terminal swapped in during that window from becoming ours.
    let file = openat(
        &directory,
        leaf,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY,
    )
    .map_err(|error| {
        if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR))
            && entry_is_symlink(&directory, leaf)
        {
            return SafeReadRefusal::SymlinkInPath(target.to_path_buf());
        }
        SafeReadRefusal::OpenFailed(error.to_string())
    })?;

    use std::os::unix::fs::MetadataExt;
    let after = file
        .metadata()
        .map_err(|error| SafeReadRefusal::OpenFailed(error.to_string()))?;
    if !after.is_file() {
        return Err(SafeReadRefusal::NotRegularFile);
    }
    if before.dev() != after.dev() || before.ino() != after.ino() {
        return Err(SafeReadRefusal::ChangedWhileOpening);
    }
    clear_nonblocking(&file).map_err(|error| SafeReadRefusal::OpenFailed(error.to_string()))?;

    Ok(SafeFile {
        file,
        length: after.len(),
        resolved_via_symlink,
        #[cfg(test)]
        path: target.to_path_buf(),
    })
}

/// Non-Unix fallback: the path-based open this module had before the
/// descriptor walk. Kept only so the crate still builds there; the
/// guarantees documented above are Unix-only.
#[cfg(not(unix))]
fn open_validated(target: &Path, resolved_via_symlink: bool) -> Result<SafeFile, SafeReadRefusal> {
    let before = std::fs::symlink_metadata(target)
        .map_err(|error| SafeReadRefusal::Unreadable(format!("{}: {error}", target.display())))?;
    if !before.is_file() {
        return Err(SafeReadRefusal::NotRegularFile);
    }
    let file = OpenOptions::new()
        .read(true)
        .open(target)
        .map_err(|error| SafeReadRefusal::OpenFailed(error.to_string()))?;
    Ok(SafeFile {
        file,
        length: before.len(),
        resolved_via_symlink,
        #[cfg(test)]
        path: target.to_path_buf(),
    })
}

/// Opens `path` read-only for a bounded read of a file the caller already
/// trusts to *name* (its own config, a cache it wrote), while still refusing
/// to block on it: the open is `O_NONBLOCK | O_NOCTTY`, the descriptor is
/// then required to be a regular file (a FIFO, device, socket or directory
/// is refused instead of hanging the reader), and blocking mode is restored
/// before the handle is returned. Symlinks are followed when `follow` is
/// true (a config file kept in a dotfiles repository) and refused with
/// `O_NOFOLLOW` when it is false.
///
/// This is the minimal FIFO-safe open (Night Watch D7-04); it makes no
/// containment claim. Callers that need containment use
/// [`open_bounded_read`].
#[cfg(unix)]
pub fn open_regular_file_nonblocking(path: &Path, follow: bool) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.read(true);
    let mut flags = libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY;
    if !follow {
        flags |= libc::O_NOFOLLOW;
    }
    options.custom_flags(flags);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    clear_nonblocking(&file)?;
    Ok(file)
}

/// Restores blocking mode on a descriptor opened with `O_NONBLOCK`.
#[cfg(unix)]
pub(crate) fn clear_nonblocking(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is a live descriptor owned by `file` for this whole call;
    // F_GETFL/F_SETFL only read and change its status flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
