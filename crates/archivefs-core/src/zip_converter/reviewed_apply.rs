//! Recovered reviewed ZIP workflow; anonymous regular-file publication only.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

#[cfg(target_os = "linux")]
mod fdio;
#[cfg(target_os = "linux")]
mod reviewed;
#[cfg(target_os = "linux")]
use reviewed::{
    MAX_BYTES, MAX_DEPTH, MAX_ENTRIES, Manifest, Reviewed, absolute, check_cancel, digest_path,
    digest_reader, open_checked, stale,
};
use std::path::{Component, Path, PathBuf};
use zip::write::SimpleFileOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZipOperation {
    Compress,
    Extract,
}

/// Where a running operation is. Reported by the worker itself, never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZipPhase {
    Waiting,
    /// Re-checking that the reviewed source and destination are unchanged.
    Checking,
    /// Copying reviewed content into a private staging object.
    Staging,
    /// Re-reading the staged output against the reviewed manifest.
    Verifying,
    /// The descriptor-bound hardlink commit; cancellation is no longer possible.
    Publishing,
    Done,
}

impl ZipPhase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Checking,
            2 => Self::Staging,
            3 => Self::Verifying,
            4 => Self::Publishing,
            5 => Self::Done,
            _ => Self::Waiting,
        }
    }
}

/// Lock-free progress a worker updates and another thread may read.
#[derive(Debug, Default)]
pub struct ZipProgress {
    phase: AtomicU8,
    files_done: AtomicU64,
    files_total: AtomicU64,
    bytes_done: AtomicU64,
    bytes_total: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZipProgressSnapshot {
    pub phase: ZipPhase,
    pub files_done: u64,
    pub files_total: u64,
    /// Source bytes copied into staging so far (not output bytes written).
    pub bytes_done: u64,
    pub bytes_total: u64,
}

impl ZipProgress {
    pub fn snapshot(&self) -> ZipProgressSnapshot {
        ZipProgressSnapshot {
            phase: ZipPhase::from_u8(self.phase.load(Ordering::Acquire)),
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
        }
    }
    fn begin(&self, files: u64, bytes: u64) {
        self.files_done.store(0, Ordering::Relaxed);
        self.bytes_done.store(0, Ordering::Relaxed);
        self.files_total.store(files, Ordering::Relaxed);
        self.bytes_total.store(bytes, Ordering::Relaxed);
    }
    fn set_phase(&self, phase: ZipPhase) {
        self.phase.store(phase as u8, Ordering::Release);
    }
    fn add_bytes(&self, count: u64) {
        self.bytes_done.fetch_add(count, Ordering::Relaxed);
    }
    fn file_done(&self) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
    }
}

/// Whether this system can publish ZIP output safely. Linux publishes with
/// `descriptor-bound linkat publication` against a pinned directory descriptor; no other
/// platform has a verified equivalent in this build, so it fails closed. Callers
/// use this to explain the restriction before any file is read.
pub fn publication_supported() -> Result<(), ZipError> {
    if cfg!(target_os = "linux") {
        Ok(())
    } else {
        Err(ZipError::UnsupportedPlatform)
    }
}

pub use super::{ZipOperationResult, ZipPreviewEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipPreview {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub entries: Vec<ZipPreviewEntry>,
    pub total_size: u64,
    #[cfg(target_os = "linux")]
    reviewed: Reviewed,
}

impl ZipPreview {
    fn layout_supported(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            match &self.reviewed.manifest {
                Manifest::Compress(_) => true,
                Manifest::Extract(entries) => {
                    entries.len() == 1
                        && !entries[0].directory
                        && Path::new(&entries[0].name).components().count() == 1
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
    pub fn apply_support(&self) -> Result<(), ZipError> {
        if !self.layout_supported() {
            return Err(ZipError::UnsupportedLayout);
        }
        #[cfg(target_os = "linux")]
        {
            let parent = self.reviewed.parent()?;
            fdio::eligible(&parent).map_err(|_| ZipError::UnsupportedPublication {
                directory: self.destination.parent().unwrap().into(),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(ZipError::UnsupportedPlatform)
        }
    }
    pub fn can_apply(&self) -> bool {
        self.apply_support().is_ok()
    }
    pub fn archive_sha256(&self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            self.reviewed
                .container_digest()
                .map(|d| d.iter().map(|b| format!("{b:02x}")).collect())
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }
}

#[cfg(target_os = "linux")]
type SourceEntry = (bool, PathBuf, u64, [u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZipError {
    SourceMissing,
    SourceSymlink,
    UnsupportedSource,
    DestinationAlreadyExists,
    DestinationParentMissing,
    UnsafePath,
    DuplicatePath,
    SymlinkMember,
    InvalidName,
    CorruptZip,
    StalePreview(String),
    Cancelled,
    LimitExceeded,
    UnsafeDestination,
    /// This system has no verified no-replace publication primitive.
    UnsupportedPlatform,
    /// The destination lacks supported anonymous-file publication.
    UnsupportedPublication {
        directory: PathBuf,
    },
    /// Writing the private output next to the destination failed. Always names
    /// the folder the user chose, never an internal staging name.
    StagingFailed {
        directory: PathBuf,
        detail: String,
    },
    UnsupportedLayout,
    UnsupportedZipStructure(String),
    PublicationUncertain {
        destination: PathBuf,
        detail: String,
    },
    VerificationFailed(String),
    Io(String),
}

impl std::fmt::Display for ZipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedZipStructure(detail) => write!(f, "Unsupported ZIP structure: {detail}. Reviewed Apply cannot process this archive."),
            Self::UnsupportedLayout => f.write_str("Extraction Apply supports exactly one top-level regular file with no directory entries. This layout is preview-only; no output was created."),
            Self::PublicationUncertain { destination, detail } => write!(f, "Output may have been published at {}: {detail}. Keep the source and inspect the destination. No automatic retry, deletion or Undo was performed.", destination.display()),
            Self::SourceMissing => write!(f, "The source could not be found."),
            Self::SourceSymlink => write!(f, "Symbolic-link sources are not supported."),
            Self::UnsupportedSource => write!(f, "Choose a regular file or folder."),
            Self::DestinationAlreadyExists => write!(f, "Destination already exists."),
            Self::DestinationParentMissing => write!(f, "The destination folder does not exist."),
            Self::UnsafePath => write!(f, "This ZIP contains an unsafe path."),
            Self::DuplicatePath => write!(f, "This ZIP contains a duplicate conflicting path."),
            Self::SymlinkMember => write!(f, "This ZIP contains an unsafe symbolic link."),
            Self::InvalidName => write!(f, "This ZIP contains an invalid name."),
            Self::StalePreview(detail) => {
                write!(f, "The reviewed {detail}. Preview again before applying.")
            }
            Self::Cancelled => write!(f, "ZIP operation cancelled; no output was published."),
            Self::LimitExceeded => write!(
                f,
                "The source exceeds the ZIP converter's bounded review limits."
            ),
            Self::UnsafeDestination => write!(f, "Choose a destination outside the source folder."),
            Self::UnsupportedPlatform => f.write_str(
                "ZIP conversion is unavailable on this system. EmuWiz publishes ZIP output with Linux anonymous-file and descriptor-bound no-replace publication and has no verified alternative, so it will not guess. Nothing was read or changed.",
            ),
            Self::UnsupportedPublication { directory } => write!(
                f,
                "The folder {} is on a filesystem without the supported local ext4 anonymous-file publication. Choose a destination on a different filesystem. Nothing was changed.",
                directory.display()
            ),
            Self::StagingFailed { directory, detail } => write!(
                f,
                "EmuWiz could not write the output in {}: {detail}. Nothing was published.",
                directory.display()
            ),
            Self::CorruptZip => write!(f, "This ZIP is corrupt or could not be read."),
            Self::VerificationFailed(detail) => write!(f, "Verification failed: {detail}"),
            Self::Io(detail) => write!(f, "The archive operation could not finish: {detail}"),
        }
    }
}

impl std::error::Error for ZipError {}

fn io_error(error: io::Error) -> ZipError {
    ZipError::Io(error.to_string())
}

fn ensure_new_destination(destination: &Path) -> Result<(), ZipError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err(ZipError::DestinationAlreadyExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    let Some(parent) = destination.parent() else {
        return Err(ZipError::DestinationParentMissing);
    };
    if !parent.is_dir() {
        return Err(ZipError::DestinationParentMissing);
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<PathBuf, ZipError> {
    let value = name.strip_suffix('/').unwrap_or(name);
    if value.is_empty()
        || value.len() > 4096
        || value
            .chars()
            .any(|c| c.is_control() || c == '\\' || c == ':')
        || value
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(ZipError::UnsafePath);
    }
    Ok(PathBuf::from(value))
}

fn member_name(path: &Path) -> Result<String, ZipError> {
    let name = path.to_str().ok_or(ZipError::InvalidName)?.to_owned();
    validate_name(&name)?;
    Ok(name)
}

#[cfg(target_os = "linux")]
fn collect_source(
    source: &Path,
    cancel: &AtomicBool,
) -> Result<BTreeMap<String, SourceEntry>, ZipError> {
    fn insert(
        result: &mut BTreeMap<String, SourceEntry>,
        total: &mut u64,
        name: String,
        value: SourceEntry,
    ) -> Result<(), ZipError> {
        if result.len() >= MAX_ENTRIES {
            return Err(ZipError::LimitExceeded);
        }
        *total = total
            .checked_add(value.2)
            .filter(|size| *size <= MAX_BYTES)
            .ok_or(ZipError::LimitExceeded)?;
        if result.keys().any(|n| n.eq_ignore_ascii_case(&name)) {
            return Err(ZipError::DuplicatePath);
        }
        if result.insert(name, value).is_some() {
            return Err(ZipError::DuplicatePath);
        }
        Ok(())
    }
    fn visit(
        root: &Path,
        current: &Path,
        depth: usize,
        result: &mut BTreeMap<String, SourceEntry>,
        total: &mut u64,
        cancel: &AtomicBool,
    ) -> Result<(), ZipError> {
        check_cancel(cancel)?;
        if depth > MAX_DEPTH {
            return Err(ZipError::LimitExceeded);
        }
        let directory = open_checked(current, true)?;
        // The descriptor pins the directory being listed, and each entry is
        // classified relative to it without following links. Content is then
        // re-opened component-by-component, never through a replacement link.
        for child in fdio::list_dir(&directory).map_err(io_error)? {
            check_cancel(cancel)?;
            let kind = fdio::kind_at(&directory, &child).map_err(io_error)?;
            let path = current.join(&child);
            if kind == fdio::Kind::Symlink {
                return Err(ZipError::SourceSymlink);
            }
            let name = member_name(path.strip_prefix(root).map_err(|_| ZipError::InvalidName)?)?;
            if kind == fdio::Kind::Directory {
                insert(result, total, name, (true, path.clone(), 0, [0; 32]))?;
                visit(root, &path, depth + 1, result, total, cancel)?;
            } else if kind == fdio::Kind::File {
                let (size, hash) = digest_path(&path, cancel)?;
                insert(result, total, name, (false, path, size, hash))?;
            } else {
                return Err(ZipError::UnsupportedSource);
            }
        }
        Ok(())
    }
    check_cancel(cancel)?;
    let metadata = fs::symlink_metadata(source).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            ZipError::SourceMissing
        } else {
            io_error(e)
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ZipError::SourceSymlink);
    }
    let mut result = BTreeMap::new();
    let mut total = 0;
    let name = member_name(Path::new(source.file_name().ok_or(ZipError::InvalidName)?))?;
    if metadata.is_file() {
        let (size, hash) = digest_path(source, cancel)?;
        insert(
            &mut result,
            &mut total,
            name,
            (false, source.to_path_buf(), size, hash),
        )?;
    } else if metadata.is_dir() {
        insert(
            &mut result,
            &mut total,
            name,
            (true, source.to_path_buf(), 0, [0; 32]),
        )?;
        visit(
            source.parent().unwrap(),
            source,
            0,
            &mut result,
            &mut total,
            cancel,
        )?;
    } else {
        return Err(ZipError::UnsupportedSource);
    }
    Ok(result)
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtractEntry {
    name: String,
    directory: bool,
    size: u64,
    hash: [u8; 32],
    index: usize,
}

#[cfg(target_os = "linux")]
fn read_extract_manifest(
    path: &Path,
    cancel: &AtomicBool,
    expected: Option<[u8; 32]>,
) -> Result<Vec<ExtractEntry>, ZipError> {
    read_extract_manifest_file(open_checked(path, false)?, cancel, expected)
}

#[cfg(target_os = "linux")]
fn open_archive(
    mut file: File,
    cancel: &AtomicBool,
    expected: Option<[u8; 32]>,
) -> Result<zip::ZipArchive<Cursor<Vec<u8>>>, ZipError> {
    use crate::dat::archive::limits::ArchiveLimits;
    use crate::dat::archive::zip_preflight::{ZipPreflightError, preflight_zip};
    let size = file.metadata().map_err(io_error)?.len();
    if size > MAX_BYTES {
        return Err(ZipError::LimitExceeded);
    }
    // Snapshot at a fixed bound before parsing: an outside writer must not
    // change metadata between preflight and the upstream ZIP index allocation.
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let n = file.read(&mut buffer).map_err(io_error)?;
        if n == 0 {
            break;
        }
        if (bytes.len() as u64)
            .checked_add(n as u64)
            .is_none_or(|n| n > MAX_BYTES)
        {
            return Err(ZipError::LimitExceeded);
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    let observed: [u8; 32] = Sha256::digest(&bytes).into();
    if expected.is_some_and(|hash| hash != observed) {
        return Err(stale("source ZIP snapshot differs from reviewed bytes"));
    }
    // Bind the upstream index parser to the same single ordinary end-record
    // selected by preflight. Upstream otherwise searches/falls back to other
    // signatures (including ones in comments) before we can compare indexes.
    // Refuse ambiguous marker-bearing archives rather than rewriting them.
    let unsupported = |message: &str| ZipError::UnsupportedZipStructure(message.into());
    let mut ends = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"PK\x05\x06");
    let (end_offset, _) = ends.next().ok_or(ZipError::CorruptZip)?;
    if ends.next().is_some()
        || bytes
            .windows(4)
            .any(|w| w == b"PK\x06\x06" || w == b"PK\x06\x07")
    {
        return Err(unsupported(
            "additional end-record markers or ZIP64 end-records are unsupported",
        ));
    }
    let end = bytes
        .get(end_offset..)
        .filter(|b| b.len() >= 22)
        .ok_or(ZipError::CorruptZip)?;
    if u16::from_le_bytes(end[10..12].try_into().unwrap()) == u16::MAX
        || u32::from_le_bytes(end[12..16].try_into().unwrap()) == u32::MAX
        || u32::from_le_bytes(end[16..20].try_into().unwrap()) == u32::MAX
    {
        return Err(unsupported("ZIP64 end-records are unsupported"));
    }
    let size = bytes.len() as u64;
    let mut snapshot = Cursor::new(bytes);
    let limits = ArchiveLimits {
        max_members: MAX_ENTRIES,
        ..Default::default()
    };
    // Reuse the bounded metadata probe before the upstream parser allocates
    // its central-directory index. Do not retain a second metadata index.
    let preflight =
        preflight_zip(&mut snapshot, size, &limits, cancel).map_err(|error| match error {
            ZipPreflightError::Cancelled => ZipError::Cancelled,
            ZipPreflightError::Refused(_) => ZipError::LimitExceeded,
            ZipPreflightError::Corrupt(_) => ZipError::CorruptZip,
        })?;
    let mut expanded = 0_u64;
    let mut names = std::collections::BTreeMap::new();
    for entry in &preflight.entries {
        if !matches!(entry.method, 0 | 8)
            || entry.flags & ((1 << 0) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 13)) != 0
        {
            return Err(ZipError::UnsupportedSource);
        }
        let name = validate_name(
            std::str::from_utf8(&entry.name_raw).map_err(|_| ZipError::InvalidName)?,
        )?;
        if names
            .insert(
                name.to_string_lossy().to_ascii_lowercase(),
                entry.is_directory,
            )
            .is_some()
        {
            return Err(ZipError::DuplicatePath);
        }
        let kind = (entry.external_attributes >> 16) & 0o170000;
        if !matches!(kind, 0 | 0o100000 | 0o040000)
            || (entry.is_directory && entry.logical_size != 0)
            || (kind == 0o040000 && !entry.is_directory)
        {
            return Err(ZipError::SymlinkMember);
        }
        expanded = expanded
            .checked_add(entry.logical_size)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or(ZipError::LimitExceeded)?;
    }
    for name in names.keys() {
        let mut parent = Path::new(name).parent();
        while let Some(path) = parent.filter(|p| !p.as_os_str().is_empty()) {
            if names.get(path.to_str().ok_or(ZipError::InvalidName)?) == Some(&false) {
                return Err(ZipError::DuplicatePath);
            }
            parent = path.parent();
        }
    }
    let archive = zip::ZipArchive::with_config(
        zip::read::Config {
            archive_offset: zip::read::ArchiveOffset::Known(0),
        },
        snapshot,
    )
    .map_err(|_| ZipError::CorruptZip)?;
    if archive.len() != preflight.entry_count {
        return Err(ZipError::CorruptZip);
    }
    Ok(archive)
}

#[cfg(target_os = "linux")]
fn read_extract_manifest_file(
    file: File,
    cancel: &AtomicBool,
    expected: Option<[u8; 32]>,
) -> Result<Vec<ExtractEntry>, ZipError> {
    let mut archive = open_archive(file, cancel, expected)?;
    if archive.len() > MAX_ENTRIES {
        return Err(ZipError::LimitExceeded);
    }
    let mut entries = Vec::new();
    let mut names = BTreeMap::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        check_cancel(cancel)?;
        let mut member = archive.by_index(index).map_err(|_| ZipError::CorruptZip)?;
        if member
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(ZipError::SymlinkMember);
        }
        let decoded =
            crate::zip_member_name::checked(member.name()).map_err(|_| ZipError::UnsafePath)?;
        let name = validate_name(&decoded)?.to_string_lossy().into_owned();
        if Path::new(&name).components().count() > MAX_DEPTH {
            return Err(ZipError::LimitExceeded);
        }
        let directory = member.is_dir();
        if names.insert(name.to_ascii_lowercase(), directory).is_some() {
            return Err(ZipError::DuplicatePath);
        }
        if member.size() > MAX_BYTES.saturating_sub(total) {
            return Err(ZipError::LimitExceeded);
        }
        let (size, hash) = if directory {
            (0, [0; 32])
        } else {
            digest_reader(&mut member, cancel).map_err(|error| match error {
                ZipError::Io(_) => ZipError::CorruptZip,
                other => other,
            })?
        };
        if size != member.size() {
            return Err(ZipError::CorruptZip);
        }
        total = total
            .checked_add(size)
            .filter(|size| *size <= MAX_BYTES)
            .ok_or(ZipError::LimitExceeded)?;
        entries.push(ExtractEntry {
            name,
            directory,
            size,
            hash,
            index,
        });
    }
    for entry in &entries {
        let mut parent = Path::new(&entry.name).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            if names.get(
                &path
                    .to_str()
                    .ok_or(ZipError::InvalidName)?
                    .to_ascii_lowercase(),
            ) == Some(&false)
            {
                return Err(ZipError::DuplicatePath);
            }
            parent = path.parent();
        }
    }
    Ok(entries)
}

/// Bounded, cancellable preview. The private review includes every member's
/// streamed SHA-256 and filesystem identity, not just displayed names/sizes.
/// On systems without verified no-replace publication this refuses first, before
/// any source or destination is read.
#[cfg(target_os = "linux")]
pub fn preview_with_cancel(
    source: &Path,
    destination: &Path,
    operation: ZipOperation,
    cancel: &AtomicBool,
) -> Result<ZipPreview, ZipError> {
    publication_supported()?;
    let source = absolute(source)?;
    let destination = absolute(destination)?;
    let reviewed = Reviewed::capture(&source, &destination, operation, cancel)?;
    let (entries, total_size) = reviewed.display_entries();
    Ok(ZipPreview {
        source,
        destination,
        entries,
        total_size,
        reviewed,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn preview_with_cancel(
    _source: &Path,
    _destination: &Path,
    _operation: ZipOperation,
    _cancel: &AtomicBool,
) -> Result<ZipPreview, ZipError> {
    publication_supported()?;
    Err(ZipError::UnsupportedPlatform)
}

pub fn preview_compress(source: &Path, destination: &Path) -> Result<ZipPreview, ZipError> {
    preview_with_cancel(
        source,
        destination,
        ZipOperation::Compress,
        &AtomicBool::new(false),
    )
}
pub fn preview_extract(source: &Path, destination: &Path) -> Result<ZipPreview, ZipError> {
    preview_with_cancel(
        source,
        destination,
        ZipOperation::Extract,
        &AtomicBool::new(false),
    )
}

/// Executes only the private plan attached to the reviewed preview. Fresh
/// collection is used solely to reject differences; it never replaces a plan.
pub fn apply_reviewed(
    preview: &ZipPreview,
    cancel: &AtomicBool,
) -> Result<ZipOperationResult, ZipError> {
    apply_reviewed_with_progress(preview, cancel, &ZipProgress::default())
}

/// As `apply_reviewed`, reporting phase and counts to `progress` as work happens.
/// Intended for a background worker; `cancel` is honoured at every safe
/// checkpoint up to the descriptor-bound commit.
#[cfg(target_os = "linux")]
pub fn apply_reviewed_with_progress(
    preview: &ZipPreview,
    cancel: &AtomicBool,
    progress: &ZipProgress,
) -> Result<ZipOperationResult, ZipError> {
    apply_with_progress(preview, cancel, progress, |_| Ok(()))
}

#[cfg(not(target_os = "linux"))]
pub fn apply_reviewed_with_progress(
    _preview: &ZipPreview,
    _cancel: &AtomicBool,
    _progress: &ZipProgress,
) -> Result<ZipOperationResult, ZipError> {
    Err(ZipError::UnsupportedPlatform)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagePhase {
    EntryStaged,
    BeforePublication,
    PublishReady,
    BeforeLink,
    AfterPublication,
}

#[cfg(target_os = "linux")]
fn copy_approved(
    input: &mut impl Read,
    output: &mut impl io::Write,
    size: u64,
    hash: &[u8; 32],
    name: &str,
    cancel: &AtomicBool,
) -> Result<(), ZipError> {
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = input
            .read(&mut buffer)
            .map_err(|error| stale(format!("source became unreadable for {name}: {error}")))?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(count as u64)
            .filter(|count| *count <= size)
            .ok_or_else(|| stale(format!("source contents changed for {name}")))?;
        hasher.update(&buffer[..count]);
        output.write_all(&buffer[..count]).map_err(io_error)?;
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if copied != size || &actual != hash {
        return Err(stale(format!("source contents changed for {name}")));
    }
    Ok(())
}

/// Counts source bytes as they are copied, so progress reflects real work.
#[cfg(target_os = "linux")]
struct CountingReader<'a, R> {
    inner: R,
    progress: &'a ZipProgress,
}

#[cfg(target_os = "linux")]
impl<R: Read> Read for CountingReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.progress.add_bytes(count as u64);
        Ok(count)
    }
}

/// Failures from here on concern the user's destination folder (source-side
/// problems are reported as stale-preview errors), so name that folder and keep
/// the private staging name out of the message.
#[cfg(target_os = "linux")]
fn name_directory(error: ZipError, directory: &Path) -> ZipError {
    match error {
        ZipError::Io(detail) => ZipError::StagingFailed {
            directory: directory.to_path_buf(),
            detail,
        },
        other => other,
    }
}

#[cfg(all(any(test, feature = "test-support"), target_os = "linux"))]
#[doc(hidden)]
pub fn apply_with_observer(
    preview: &ZipPreview,
    cancel: &AtomicBool,
    observe: impl FnMut(StagePhase) -> Result<(), ZipError>,
) -> Result<ZipOperationResult, ZipError> {
    apply_with_progress(preview, cancel, &ZipProgress::default(), observe)
}

#[cfg(target_os = "linux")]
fn apply_with_progress(
    preview: &ZipPreview,
    cancel: &AtomicBool,
    progress: &ZipProgress,
    mut observe: impl FnMut(StagePhase) -> Result<(), ZipError>,
) -> Result<ZipOperationResult, ZipError> {
    publication_supported()?;
    check_cancel(cancel)?;
    if !preview.reviewed.matches_display(preview) {
        return Err(stale("reviewed paths or manifest were edited"));
    }
    let directory = preview
        .destination
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let parent = preview
        .reviewed
        .parent()
        .map_err(|error| name_directory(error, &directory))?;
    preview.apply_support()?;
    fdio::eligible(&parent).map_err(|_| ZipError::UnsupportedPublication {
        directory: directory.clone(),
    })?;
    progress.begin(
        preview.entries.iter().filter(|e| !e.directory).count() as u64,
        preview.total_size,
    );
    progress.set_phase(ZipPhase::Checking);
    preview
        .reviewed
        .validate(cancel)
        .map_err(|error| name_directory(error, &directory))?;
    progress.set_phase(ZipPhase::Staging);
    stage_and_publish(preview, cancel, progress, &parent, &mut observe)
        .map_err(|error| name_directory(error, &directory))?;
    progress.set_phase(ZipPhase::Done);
    Ok(ZipOperationResult {
        source: preview.source.clone(),
        destination: preview.destination.clone(),
        files: preview
            .entries
            .iter()
            .filter(|entry| !entry.directory)
            .count(),
        total_size: preview.total_size,
        verification: "Reviewed ZIP operation completed; member names, sizes and SHA-256 verified"
            .into(),
    })
}

/// The existing reviewed-copy transaction now keeps its verified output unnamed
/// until the single link commit. No name-based cleanup or directory publication.
#[cfg(target_os = "linux")]
fn stage_and_publish(
    preview: &ZipPreview,
    cancel: &AtomicBool,
    progress: &ZipProgress,
    parent: &File,
    observe: &mut dyn FnMut(StagePhase) -> Result<(), ZipError>,
) -> Result<(), ZipError> {
    use std::io::{Seek, SeekFrom};
    use std::os::unix::fs::MetadataExt;
    let mut temporary = fdio::anonymous(parent).map_err(|error| {
        if matches!(
            error.raw_os_error(),
            Some(libc::EOPNOTSUPP | libc::EINVAL | libc::ENOSYS)
        ) {
            ZipError::UnsupportedPublication {
                directory: preview.destination.parent().unwrap().into(),
            }
        } else {
            io_error(error)
        }
    })?;
    match &preview.reviewed.manifest {
        Manifest::Compress(manifest) => {
            let mut writer = zip::ZipWriter::new(temporary.try_clone().map_err(io_error)?);
            for (name, (directory, path, size, hash)) in manifest {
                check_cancel(cancel)?;
                if *directory {
                    writer
                        .add_directory(format!("{name}/"), preview.reviewed.options)
                        .map_err(|_| ZipError::CorruptZip)?;
                } else {
                    writer
                        .start_file(name, preview.reviewed.options)
                        .map_err(|_| ZipError::CorruptZip)?;
                    let mut input = CountingReader {
                        inner: preview.reviewed.open_member(name, path)?,
                        progress,
                    };
                    copy_approved(&mut input, &mut writer, *size, hash, name, cancel)?;
                    progress.file_done();
                    observe(StagePhase::EntryStaged)?;
                }
            }
            writer.finish().map_err(|_| ZipError::CorruptZip)?;
            progress.set_phase(ZipPhase::Verifying);
            let entries =
                read_extract_manifest_file(temporary.try_clone().map_err(io_error)?, cancel, None)?;
            if entries.len() != manifest.len()
                || entries.iter().any(|entry| {
                    manifest
                        .get(&entry.name)
                        .is_none_or(|(dir, _, size, hash)| {
                            entry.directory != *dir || entry.size != *size || entry.hash != *hash
                        })
                })
            {
                return Err(ZipError::VerificationFailed(
                    "ZIP differs from reviewed members".into(),
                ));
            }
        }
        Manifest::Extract(entries) => {
            preview.apply_support()?;
            let entry = &entries[0];
            let mut archive = open_archive(
                preview.reviewed.open_source()?,
                cancel,
                preview.reviewed.container_digest(),
            )?;
            if archive.len() != 1 {
                return Err(stale("source ZIP member count changed"));
            }
            let mut member = archive
                .by_index(entry.index)
                .map_err(|_| ZipError::CorruptZip)?;
            let name_unchanged = crate::zip_member_name::checked(member.name())
                .is_ok_and(|decoded| decoded == entry.name.as_str());
            if !name_unchanged || member.is_dir() || member.size() != entry.size {
                return Err(stale("source ZIP member changed"));
            }
            let mut counted = CountingReader {
                inner: &mut member,
                progress,
            };
            copy_approved(
                &mut counted,
                &mut temporary,
                entry.size,
                &entry.hash,
                &entry.name,
                cancel,
            )?;
            temporary.seek(SeekFrom::Start(0)).map_err(io_error)?;
            if digest_reader(&mut temporary, cancel)? != (entry.size, entry.hash) {
                return Err(ZipError::VerificationFailed(
                    "extracted bytes differ from review".into(),
                ));
            }
            progress.file_done();
            observe(StagePhase::EntryStaged)?;
        }
    }
    temporary.sync_all().map_err(io_error)?;
    // Fingerprint the exact fully verified anonymous output, including ZIP
    // container bytes. Post-publication inspection does not trust a pathname.
    temporary.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let output_digest = digest_reader(&mut temporary, cancel)?;
    observe(StagePhase::BeforePublication)?;
    progress.set_phase(ZipPhase::Checking);
    observe(StagePhase::PublishReady)?;
    preview.reviewed.validate(cancel)?;
    check_cancel(cancel)?;
    observe(StagePhase::BeforeLink)?;
    check_cancel(cancel)?;
    progress.set_phase(ZipPhase::Publishing);
    let name = preview
        .destination
        .file_name()
        .ok_or(ZipError::InvalidName)?;
    let uncertain = |detail: String| ZipError::PublicationUncertain {
        destination: preview.destination.clone(),
        detail,
    };
    fdio::publish(parent, &temporary, name).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            stale("destination became occupied; no existing entry was overwritten")
        } else {
            uncertain(error.to_string())
        }
    })?;
    // After the commit point no error is called "nothing published", and no
    // cancellation or failure ever deletes an output/foreign entry by name.
    let verify = || -> Result<(), ZipError> {
        parent.sync_all().map_err(io_error)?;
        preview.reviewed.validate_source(&AtomicBool::new(false))?;
        preview.reviewed.parent()?;
        let mut final_file = fdio::open_file_at(parent, name).map_err(io_error)?;
        let ours = temporary.metadata().map_err(io_error)?;
        let found = final_file.metadata().map_err(io_error)?;
        if !found.is_file() || (ours.dev(), ours.ino()) != (found.dev(), found.ino()) {
            return Err(stale("published name was externally replaced"));
        }
        if digest_reader(&mut final_file, &AtomicBool::new(false))? != output_digest {
            return Err(stale("published bytes changed"));
        }
        Ok(())
    };
    observe(StagePhase::AfterPublication)
        .and_then(|_| verify())
        .map_err(|error| uncertain(error.to_string()))
}
