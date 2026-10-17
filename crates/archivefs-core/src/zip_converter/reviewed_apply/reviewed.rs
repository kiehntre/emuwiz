//! Private evidence attached to the displayed preview. Apply may compare fresh
//! evidence with this record, but must never replace it with a newly made plan.
use super::*;
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) const MAX_ENTRIES: usize = 4096;
pub(super) const MAX_BYTES: u64 = 128 * 1024 * 1024;
pub(super) const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
    directory: bool,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Identity {
    fn of(meta: &fs::Metadata) -> Self {
        let directory = meta.is_dir();
        Self {
            device: meta.dev(),
            inode: meta.ino(),
            directory,
            // Directory timestamps change when private staging is created.
            size: if directory { 0 } else { meta.len() },
            modified: if directory {
                (0, 0)
            } else {
                (meta.mtime(), meta.mtime_nsec())
            },
            changed: if directory {
                (0, 0)
            } else {
                (meta.ctime(), meta.ctime_nsec())
            },
        }
    }
    fn at(path: &Path) -> Result<Self, ZipError> {
        let meta = fs::symlink_metadata(path).map_err(io_error)?;
        if meta.file_type().is_symlink() {
            return Err(ZipError::SourceSymlink);
        }
        if !meta.is_dir() && !meta.is_file() {
            return Err(ZipError::UnsupportedSource);
        }
        Ok(Self::of(&meta))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Manifest {
    Compress(BTreeMap<String, SourceEntry>),
    Extract(Vec<ExtractEntry>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reviewed {
    source: PathBuf,
    destination: PathBuf,
    canonical_source: PathBuf,
    canonical_parent: PathBuf,
    source_identity: Identity,
    parent_identity: Identity,
    identities: BTreeMap<String, Identity>,
    container: Option<(u64, [u8; 32])>,
    // The operation pins the converter's fixed ZIP options: default ZIP
    // creation options or safe extraction, with no overwrite in either mode.
    pub(super) manifest: Manifest,
    pub(super) options: SimpleFileOptions,
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<(), ZipError> {
    if cancel.load(Ordering::Acquire) {
        Err(ZipError::Cancelled)
    } else {
        Ok(())
    }
}

pub(super) fn stale(detail: impl Into<String>) -> ZipError {
    ZipError::StalePreview(detail.into())
}

/// Walk every component with openat/O_NOFOLLOW. Opening a final file alone
/// with O_NOFOLLOW would still follow a concurrently substituted parent link.
pub(super) fn open_checked(path: &Path, directory: bool) -> Result<File, ZipError> {
    let mut current = File::open("/").map_err(io_error)?;
    let components: Vec<_> = path.components().collect();
    if !path.is_absolute() {
        return Err(ZipError::UnsafePath);
    }
    for (index, component) in components.iter().enumerate() {
        if matches!(component, Component::RootDir) {
            continue;
        }
        let Component::Normal(name) = component else {
            return Err(ZipError::UnsafePath);
        };
        let name = CString::new(name.as_bytes()).map_err(|_| ZipError::InvalidName)?;
        let is_directory = index + 1 != components.len() || directory;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if is_directory { libc::O_DIRECTORY } else { 0 };
        let descriptor = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(io_error(io::Error::last_os_error()));
        }
        current = unsafe { File::from_raw_fd(descriptor) };
    }
    let meta = current.metadata().map_err(io_error)?;
    if (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
        return Err(ZipError::UnsupportedSource);
    }
    Ok(current)
}

pub(super) fn digest_reader(
    reader: &mut impl Read,
    cancel: &AtomicBool,
) -> Result<(u64, [u8; 32]), ZipError> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or(ZipError::LimitExceeded)?;
        hasher.update(&buffer[..count]);
    }
    Ok((size, hasher.finalize().into()))
}

pub(super) fn digest_path(path: &Path, cancel: &AtomicBool) -> Result<(u64, [u8; 32]), ZipError> {
    let mut file = open_checked(path, false)?;
    let before = Identity::of(&file.metadata().map_err(io_error)?);
    if before.size > MAX_BYTES {
        return Err(ZipError::LimitExceeded);
    }
    let digest = digest_reader(&mut file, cancel)?;
    if before != Identity::of(&file.metadata().map_err(io_error)?) {
        return Err(stale(format!(
            "source contents changed while reading {}",
            path.display()
        )));
    }
    Ok(digest)
}

pub(super) fn absolute(path: &Path) -> Result<PathBuf, ZipError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(io_error)?.join(path)
    };
    if absolute
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(ZipError::UnsafePath);
    }
    Ok(absolute)
}

impl Reviewed {
    pub(super) fn capture(
        source: &Path,
        destination: &Path,
        operation: ZipOperation,
        cancel: &AtomicBool,
    ) -> Result<Self, ZipError> {
        check_cancel(cancel)?;
        ensure_new_destination(destination)?;
        let source_identity = Identity::at(source)?;
        open_checked(source, source_identity.directory)?;
        open_checked(
            destination
                .parent()
                .ok_or(ZipError::DestinationParentMissing)?,
            true,
        )?;
        let canonical_source = fs::canonicalize(source).map_err(io_error)?;
        let canonical_parent = fs::canonicalize(
            destination
                .parent()
                .ok_or(ZipError::DestinationParentMissing)?,
        )
        .map_err(io_error)?;
        let parent_identity = Identity::at(&canonical_parent)?;
        if source_identity.directory && canonical_parent.starts_with(&canonical_source) {
            return Err(ZipError::UnsafeDestination);
        }
        let container = if operation == ZipOperation::Extract {
            Some(digest_path(&canonical_source, cancel)?)
        } else {
            None
        };
        let manifest = match operation {
            ZipOperation::Compress => {
                Manifest::Compress(collect_source(&canonical_source, cancel)?)
            }
            ZipOperation::Extract => Manifest::Extract(read_extract_manifest(
                &canonical_source,
                cancel,
                container.map(|(_, digest)| digest),
            )?),
        };
        let identities = match &manifest {
            Manifest::Compress(entries) => entries
                .iter()
                .map(|(name, (_, path, _, _))| Identity::at(path).map(|id| (name.clone(), id)))
                .collect::<Result<_, _>>()?,
            Manifest::Extract(_) => BTreeMap::new(),
        };
        let reviewed = Self {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            canonical_source,
            canonical_parent,
            source_identity,
            parent_identity,
            identities,
            container,
            manifest,
            options: SimpleFileOptions::default(),
        };
        reviewed.validate(cancel)?;
        Ok(reviewed)
    }

    pub(super) fn container_digest(&self) -> Option<[u8; 32]> {
        self.container.map(|(_, d)| d)
    }

    pub(super) fn open_source(&self) -> Result<File, ZipError> {
        let file = open_checked(&self.canonical_source, false)
            .map_err(|error| stale(format!("source identity changed: {error}")))?;
        if Identity::of(&file.metadata().map_err(io_error)?) != self.source_identity {
            return Err(stale("source identity changed"));
        }
        Ok(file)
    }

    pub(super) fn open_member(&self, name: &str, path: &Path) -> Result<File, ZipError> {
        let file = open_checked(path, false)
            .map_err(|error| stale(format!("source identity changed for {name}: {error}")))?;
        if self.identities.get(name) != Some(&Identity::of(&file.metadata().map_err(io_error)?)) {
            return Err(stale(format!("source identity changed for {name}")));
        }
        Ok(file)
    }

    pub(super) fn validate_source(&self, cancel: &AtomicBool) -> Result<(), ZipError> {
        check_cancel(cancel)?;
        if fs::canonicalize(&self.source).ok().as_ref() != Some(&self.canonical_source)
            || Identity::at(&self.source).ok().as_ref() != Some(&self.source_identity)
        {
            return Err(stale("source identity or contents changed"));
        }
        match &self.manifest {
            Manifest::Compress(approved) => {
                let current = collect_source(&self.canonical_source, cancel).map_err(|error| {
                    if error == ZipError::Cancelled {
                        error
                    } else {
                        stale(format!("source manifest could not be revalidated: {error}"))
                    }
                })?;
                if &current != approved {
                    return Err(stale("source file list, names, sizes or contents changed"));
                }
                for (name, (_, path, _, _)) in approved {
                    if Identity::at(path).ok().as_ref() != self.identities.get(name) {
                        return Err(stale(format!("source identity changed for {name}")));
                    }
                }
            }
            Manifest::Extract(_) => {
                let digest = digest_path(&self.canonical_source, cancel).map_err(|error| {
                    if error == ZipError::Cancelled {
                        error
                    } else {
                        stale(format!("source ZIP could not be revalidated: {error}"))
                    }
                })?;
                if Some(digest) != self.container {
                    return Err(stale("source ZIP contents changed"));
                }
            }
        }
        check_cancel(cancel)
    }
    pub(super) fn validate(&self, cancel: &AtomicBool) -> Result<(), ZipError> {
        self.validate_source(cancel)?;
        if fs::canonicalize(self.destination.parent().unwrap())
            .ok()
            .as_ref()
            != Some(&self.canonical_parent)
            || Identity::at(&self.canonical_parent).ok().as_ref() != Some(&self.parent_identity)
        {
            return Err(stale("destination folder identity changed"));
        }
        ensure_new_destination(&self.destination).map_err(|error| match error {
            ZipError::DestinationAlreadyExists => {
                stale("destination is now occupied; Destination already exists")
            }
            other => other,
        })?;
        check_cancel(cancel)
    }

    pub(super) fn parent(&self) -> Result<File, ZipError> {
        // A folder that was replaced, removed or turned into a link is a changed
        // review, not an I/O fault, so classify it before trying to open it.
        if Identity::at(&self.canonical_parent).ok().as_ref() != Some(&self.parent_identity) {
            return Err(stale("destination folder identity changed"));
        }
        let parent = open_checked(&self.canonical_parent, true)?;
        if Identity::of(&parent.metadata().map_err(io_error)?) != self.parent_identity {
            return Err(stale("destination folder identity changed"));
        }
        Ok(parent)
    }

    pub(super) fn matches_display(&self, preview: &ZipPreview) -> bool {
        let (entries, total_size) = self.display_entries();
        preview.source == self.source
            && preview.destination == self.destination
            && preview.entries == entries
            && preview.total_size == total_size
    }

    pub(super) fn display_entries(&self) -> (Vec<ZipPreviewEntry>, u64) {
        let entries: Vec<_> = match &self.manifest {
            Manifest::Compress(entries) => entries
                .iter()
                .map(|(name, (directory, _, size, _))| ZipPreviewEntry {
                    name: name.clone(),
                    directory: *directory,
                    size: *size,
                })
                .collect(),
            Manifest::Extract(entries) => entries
                .iter()
                .map(|entry| ZipPreviewEntry {
                    name: entry.name.clone(),
                    directory: entry.directory,
                    size: entry.size,
                })
                .collect(),
        };
        let total = entries.iter().map(|entry| entry.size).sum();
        (entries, total)
    }
}
