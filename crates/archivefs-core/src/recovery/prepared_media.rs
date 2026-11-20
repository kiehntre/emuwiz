//! EmuWiz's own store of verified, prepared copies of disc sets.
//!
//! A prepared copy is a new set of files extracted from a ZIP the emulator
//! cannot open directly. The original is opened read-only and never changed.
//!
//! Layout under the store root (default `<data>/prepared-media`):
//!
//! ```text
//! v1/lock                      one preparation, removal or clean-up at a time
//! v1/journal.jsonl             append-only record of what this store did
//! v1/staging/<id>-<random>/    work in progress: intent.json, media/, receipt.json
//! v1/sets/<id>/                committed: receipt.json, intent.json, media/<files>
//! v1/sources/<hash>.json      which unchanged ZIP a set was prepared from (EmuWiz's own index)
//! ```
//!
//! Guarantees:
//!
//! * Create-new only. Every file is created with `O_EXCL|O_NOFOLLOW` inside a
//!   fresh private staging folder; the finished folder is published with one
//!   no-replace rename, so a set is either complete with its receipt or not
//!   visible at all. Nothing that exists is ever overwritten.
//! * Verified twice. The member reader checks each file's decoded size and
//!   CRC-32 against the ZIP's central directory; the files are then read back
//!   from disk and hashed again, the cue sheet is re-parsed from the extracted
//!   bytes, and the data track must carry an ISO 9660 volume descriptor.
//! * Bounded. Member count, per-file size, total size, compression ratio and
//!   free disk space are checked before any byte is written.
//! * Reversible. `remove` deletes only the files the receipt lists, and only
//!   while they still have the recorded size; anything else is left in place
//!   and reported.
//! * Interruption-safe. A killed preparation leaves only a staging folder
//!   with an intent record; it is never used for launch and is removed by
//!   `clean_interrupted`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use super::cue_sheet::{CueFileKind, CueTrackMode, parse_cue_sheet};
use super::disc_zip::{
    DiscMemberRole, DiscSetInspection, DiscSetLimits, DiscSetMember, DiscSetRefusal,
    SourceIdentity, inspect_disc_zip,
};
use crate::dat::archive::hash::hash_member_stream;
use crate::dat::archive::zip::{ZipMemberError, ZipMemberRequest, copy_zip_member_into};
use crate::dat::rename_apply::noclobber::{NoClobberError, rename_noreplace};
use crate::safe_read::{TrustedRoots, open_directory_no_symlinks};

/// Overrides the store root. Must be an absolute path.
pub const PREPARED_MEDIA_DIR_ENV: &str = "EMUWIZ_PREPARED_MEDIA_DIR";
const DEFAULT_LEAF: &str = "prepared-media";
const FORMAT_VERSION: u32 = 1;
/// Kept free on the destination filesystem beyond the set's own size.
pub const FREE_SPACE_RESERVE_BYTES: u64 = 1024 * 1024 * 1024;
const SET_ID_LENGTH: usize = 32;
const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;
const RECEIPT_NAME: &str = "receipt.json";
const INTENT_NAME: &str = "intent.json";
const MEDIA_DIR: &str = "media";
/// The ISO 9660 system identifier on Commodore CDTV and CD32 discs.
pub const COMMODORE_CD_SYSTEM_IDENTIFIER: &str = "CDTV";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptFile {
    pub name: String,
    pub role: DiscMemberRole,
    pub size_bytes: u64,
    pub crc32: String,
    pub sha256: String,
    pub inode: u64,
    pub modified_ns: i128,
    pub changed_ns: i128,
}

/// What the data track's own volume descriptor says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscFacts {
    pub system_identifier: String,
    pub volume_identifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub format_version: u32,
    pub set_id: String,
    pub fingerprint: String,
    pub platform_id: String,
    pub created_unix: u64,
    pub source_path: PathBuf,
    pub source: SourceIdentity,
    pub files: Vec<ReceiptFile>,
    pub total_bytes: u64,
    pub disc: Option<DiscFacts>,
}

/// Links one unchanged source ZIP to the set prepared from it, so a launch
/// can find the prepared copy from the ZIP's path and file metadata alone,
/// without reading the ZIP again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceLink {
    format_version: u32,
    source_path: PathBuf,
    source: SourceIdentity,
    set_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Intent {
    format_version: u32,
    set_id: String,
    fingerprint: String,
    source_path: PathBuf,
    files: Vec<String>,
    started_unix: u64,
    process_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreparedSetSummary {
    pub set_id: String,
    pub fingerprint: String,
    pub platform_id: String,
    pub directory: PathBuf,
    /// The file an emulator is given.
    pub launch_path: PathBuf,
    pub file_count: usize,
    pub total_bytes: u64,
    pub created_unix: u64,
    pub source_path: PathBuf,
    pub disc: Option<DiscFacts>,
}

/// What `prepare` would do, for the person to review first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreparationPlan {
    pub set_id: String,
    pub destination: PathBuf,
    pub files: Vec<DiscSetMember>,
    pub total_bytes: u64,
    pub required_free_bytes: u64,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetState {
    Ready,
    /// The receipt or a file no longer matches; never used for launch.
    Damaged(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedSet {
    pub set_id: String,
    pub state: SetState,
    pub summary: Option<PreparedSetSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterruptedPreparation {
    pub directory: PathBuf,
    pub set_id: Option<String>,
    pub source_path: Option<PathBuf>,
    pub started_unix: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovalReport {
    pub set_id: String,
    pub removed_files: Vec<String>,
    /// Paths left in place because they were not exactly what the receipt
    /// lists. Never deleted by this engine.
    pub kept: Vec<PathBuf>,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareProgress<'a> {
    Extracting {
        name: &'a str,
        index: usize,
        count: usize,
    },
    Verifying {
        name: &'a str,
        index: usize,
        count: usize,
    },
    Publishing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareOutcome {
    pub summary: PreparedSetSummary,
    /// True when a verified copy of this exact content was already there and
    /// nothing was written.
    pub already_prepared: bool,
    /// Set when the copy is in place but the record linking it to this ZIP
    /// could not be written; the Launch page will not find the copy until
    /// preparation is run again.
    pub link_warning: Option<String>,
}

/// A prepared copy's link to its ZIP, captured while it was valid, so it can
/// follow that same file through a rename EmuWiz itself is about to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveCandidate {
    pub old_path: PathBuf,
    pub new_path: PathBuf,
    pub set_id: String,
    pub platform_id: String,
    identity: SourceIdentity,
}

/// What looking for an existing prepared copy of a ZIP's contents found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelinkOutcome {
    /// No prepared copy holds this disc. Nothing was written.
    NoMatchingCopy,
    /// The ZIP was read in full, every file matches the prepared copy's
    /// recorded SHA-256, and the ZIP is now linked to it.
    Linked {
        summary: Box<PreparedSetSummary>,
        /// The one path this copy was linked from that no longer exists: the
        /// ZIP was renamed or moved from there. `None` when there is no such
        /// path or more than one (then nothing is assumed).
        moved_from: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    /// The store's own folder is unusable (named).
    Store(String),
    /// Another preparation, removal or clean-up holds the store.
    Busy,
    Refused(DiscSetRefusal),
    /// A copy with this identity exists but fails its checks; it is never
    /// overwritten. Remove it first.
    ExistingCopyDamaged(String),
    NotEnoughSpace {
        needed_bytes: u64,
        available_bytes: u64,
    },
    Extraction(String),
    /// The extracted files did not pass independent verification.
    Verification(String),
    SourceChanged,
    Cancelled,
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(detail) => write!(f, "the prepared-media folder cannot be used: {detail}"),
            Self::Busy => f.write_str("another preparation is running; try again when it finishes"),
            Self::Refused(refusal) => write!(f, "{refusal}"),
            Self::ExistingCopyDamaged(detail) => write!(
                f,
                "a prepared copy already exists but fails its checks ({detail}); remove it first"
            ),
            Self::NotEnoughSpace {
                needed_bytes,
                available_bytes,
            } => write!(
                f,
                "not enough free space: {needed_bytes} bytes needed, {available_bytes} available"
            ),
            Self::Extraction(detail) => write!(f, "extraction failed: {detail}"),
            Self::Verification(detail) => write!(f, "verification failed: {detail}"),
            Self::SourceChanged => f.write_str("the ZIP changed while it was being prepared"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveError {
    Store(String),
    Busy,
    NotFound,
    /// The receipt is missing or unreadable, so the engine cannot prove which
    /// files are its own. Nothing was deleted.
    NoUsableReceipt(String),
}

impl std::fmt::Display for RemoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(detail) => write!(f, "the prepared-media folder cannot be used: {detail}"),
            Self::Busy => f.write_str("another preparation is running; try again when it finishes"),
            Self::NotFound => f.write_str("no prepared copy with that id exists"),
            Self::NoUsableReceipt(detail) => write!(
                f,
                "the prepared copy has no usable receipt ({detail}); nothing was deleted"
            ),
        }
    }
}

/// Whether a launch may use a path that lies inside the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchPathCheck {
    /// The path is not inside the store; this module has no opinion.
    NotPreparedMedia,
    /// The path is the launch file of a set that passes its checks.
    Verified(PreparedSetSummary),
    /// The path is inside the store but must not be started.
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedMediaStore {
    root: PathBuf,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn set_id_of(fingerprint: &str) -> String {
    fingerprint.chars().take(SET_ID_LENGTH).collect()
}

fn valid_set_id(id: &str) -> bool {
    id.len() == SET_ID_LENGTH && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Removed file names, paths left in place, bytes freed.
type OwnedRemoval = (Vec<String>, Vec<PathBuf>, u64);

struct StoreLock(File);

impl Drop for StoreLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor is open for the lifetime of `self`.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn create_new_file(path: &Path, mode: u32) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

fn open_existing_no_follow(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

fn sync_directory(path: &Path) {
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
}

fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    let mut file = create_new_file(path, 0o600).map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let file = open_existing_no_follow(path).map_err(|error| error.to_string())?;
    let mut text = String::new();
    file.take(MAX_RECEIPT_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() as u64 > MAX_RECEIPT_BYTES {
        return Err("record is larger than any this engine writes".into());
    }
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

#[cfg(test)]
thread_local! {
    /// Test-only: pretend the disk has this many bytes free.
    pub(crate) static FREE_SPACE_OVERRIDE: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
}

pub(crate) fn available_bytes(directory: &Path) -> Result<u64, String> {
    use std::os::unix::ffi::OsStrExt;
    #[cfg(test)]
    if let Some(pretend) = FREE_SPACE_OVERRIDE.with(std::cell::Cell::get) {
        return Ok(pretend);
    }
    let name = std::ffi::CString::new(directory.as_os_str().as_bytes())
        .map_err(|_| "path contains NUL".to_string())?;
    // SAFETY: zeroed is a valid initial value for this plain C struct.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is NUL-terminated and `stats` is writable and sized.
    if unsafe { libc::statvfs(name.as_ptr(), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    fn widen<T: Into<u64>>(value: T) -> u64 {
        value.into()
    }
    Ok(widen(stats.f_bavail).saturating_mul(widen(stats.f_frsize)))
}

/// The ISO 9660 primary volume descriptor of a data track, if it has one.
fn read_disc_facts(file: &mut File, mode: CueTrackMode) -> Option<DiscFacts> {
    let offset = match mode {
        CueTrackMode::Mode1Cooked => 16 * 2048,
        CueTrackMode::Mode1Raw => 16 * 2352 + 16,
        CueTrackMode::Mode2Raw => 16 * 2352 + 24,
        CueTrackMode::Audio => return None,
    };
    let mut descriptor = [0_u8; 72];
    file.seek(SeekFrom::Start(offset)).ok()?;
    file.read_exact(&mut descriptor).ok()?;
    if descriptor[0] != 1 || &descriptor[1..6] != b"CD001" {
        return None;
    }
    let text = |bytes: &[u8]| -> String {
        bytes
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| {
                if byte.is_ascii_graphic() || *byte == b' ' {
                    *byte as char
                } else {
                    '?'
                }
            })
            .collect::<String>()
            .trim()
            .to_string()
    };
    Some(DiscFacts {
        system_identifier: text(&descriptor[8..40]),
        volume_identifier: text(&descriptor[40..72]),
    })
}

fn is_riff_wave(file: &mut File) -> bool {
    let mut header = [0_u8; 12];
    file.seek(SeekFrom::Start(0)).is_ok()
        && file.read_exact(&mut header).is_ok()
        && &header[0..4] == b"RIFF"
        && &header[8..12] == b"WAVE"
}

impl PreparedMediaStore {
    /// A store at an explicit root (tests, the CLI's `--store`).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store for this profile: `EMUWIZ_PREPARED_MEDIA_DIR` when set to an
    /// absolute path, otherwise `<data>/prepared-media`.
    pub fn for_profile() -> Result<Self, String> {
        if let Some(value) = std::env::var_os(PREPARED_MEDIA_DIR_ENV) {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(format!("{PREPARED_MEDIA_DIR_ENV} must be an absolute path"));
            }
            return Ok(Self::at(path));
        }
        if let Some(folder) = super::storage::configured_folder()? {
            return Ok(Self::at(folder));
        }
        Self::default_location()
    }

    /// `<data>/prepared-media`, where copies go unless a folder was chosen.
    pub fn default_location() -> Result<Self, String> {
        crate::app_dirs::data_path(DEFAULT_LEAF)
            .map(Self::at)
            .map_err(|error| error.to_string())
    }

    /// Bytes free for new copies where this store is (or would be created).
    pub fn available_bytes(&self) -> Result<u64, String> {
        let mut probe = self.root.as_path();
        while fs::symlink_metadata(probe).is_err() {
            probe = probe
                .parent()
                .ok_or_else(|| "no existing folder above the store".to_string())?;
        }
        available_bytes(probe)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn versioned(&self) -> PathBuf {
        self.root.join("v1")
    }

    fn sets_dir(&self) -> PathBuf {
        self.versioned().join("sets")
    }

    fn staging_dir(&self) -> PathBuf {
        self.versioned().join("staging")
    }

    fn set_dir(&self, set_id: &str) -> PathBuf {
        self.sets_dir().join(set_id)
    }

    fn sources_dir(&self) -> PathBuf {
        self.versioned().join("sources")
    }

    fn source_link_path(&self, source: &Path) -> PathBuf {
        use sha2::{Digest, Sha256};
        use std::os::unix::ffi::OsStrExt;
        let digest = Sha256::digest(source.as_os_str().as_bytes());
        let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        self.sources_dir().join(format!("{name}.json"))
    }

    /// Records that `source`, exactly as it is now, is served by `set_id`.
    /// This is EmuWiz's own index entry; an older entry for the same path is
    /// replaced atomically.
    fn link_source(
        &self,
        source: &Path,
        identity: SourceIdentity,
        set_id: &str,
    ) -> Result<(), String> {
        let link = SourceLink {
            format_version: FORMAT_VERSION,
            source_path: source.to_path_buf(),
            source: identity,
            set_id: set_id.to_string(),
        };
        let text = serde_json::to_string_pretty(&link).map_err(|error| error.to_string())?;
        let mut temporary = tempfile::Builder::new()
            .prefix("link-")
            .tempfile_in(self.sources_dir())
            .map_err(|error| error.to_string())?;
        temporary
            .write_all(text.as_bytes())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| error.to_string())?;
        temporary
            .persist(self.source_link_path(source))
            .map_err(|error| error.to_string())?;
        sync_directory(&self.sources_dir());
        Ok(())
    }

    fn unlink_sources_of(&self, set_id: &str) {
        let Ok(entries) = fs::read_dir(self.sources_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if read_json::<SourceLink>(&path).is_ok_and(|link| link.set_id == set_id) {
                let _ = fs::remove_file(&path);
            }
        }
        sync_directory(&self.sources_dir());
    }

    /// Link records of `set_id` whose ZIP is no longer at the recorded path.
    /// They are removed; the paths are returned.
    fn retire_stale_links(&self, set_id: &str) -> Vec<PathBuf> {
        let mut gone = Vec::new();
        let Ok(entries) = fs::read_dir(self.sources_dir()) else {
            return gone;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(link) = read_json::<SourceLink>(&path) else {
                continue;
            };
            if link.set_id == set_id
                && fs::symlink_metadata(&link.source_path).is_err()
                && fs::remove_file(&path).is_ok()
            {
                gone.push(link.source_path);
            }
        }
        if !gone.is_empty() {
            sync_directory(&self.sources_dir());
        }
        gone
    }

    /// Captures the link of `old_path` if, right now, it is fully valid (the
    /// same check a launch uses). Called before EmuWiz renames or moves that
    /// file to `new_path`. Cheap; reads no game data.
    pub fn movable_link(&self, old_path: &Path, new_path: &Path) -> Option<MoveCandidate> {
        let summary = self.prepared_for_source(old_path)?;
        let store = self.existing()?;
        let link: SourceLink = read_json(&store.source_link_path(old_path)).ok()?;
        Some(MoveCandidate {
            old_path: old_path.to_path_buf(),
            new_path: new_path.to_path_buf(),
            set_id: summary.set_id,
            platform_id: summary.platform_id,
            identity: link.source,
        })
    }

    /// Moves a captured link to the file's new path after EmuWiz renamed it.
    /// The file at the new path must be the very same file object (device and
    /// inode) with the same size and modification time, and the old path must
    /// be gone; a name, a size or a timestamp alone is never enough. A move to
    /// another filesystem creates a new file and is not followed here: the
    /// content check in [`Self::relink`] covers it.
    pub fn carry_link(&self, candidate: &MoveCandidate) -> Result<PreparedSetSummary, String> {
        let store = self
            .existing()
            .ok_or_else(|| "the prepared-media folder is unavailable".to_string())?;
        let never = AtomicBool::new(false);
        let _lock = store
            .lock(&never, false)
            .map_err(|_| "the prepared-media folder is busy".to_string())?;
        let metadata =
            fs::symlink_metadata(&candidate.new_path).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("the renamed game file is not a regular file".into());
        }
        let now = SourceIdentity::of(&metadata);
        if now.device != candidate.identity.device
            || now.inode != candidate.identity.inode
            || now.size_bytes != candidate.identity.size_bytes
            || now.modified_ns != candidate.identity.modified_ns
        {
            return Err("the file at the new name is not the file that was prepared".into());
        }
        if fs::symlink_metadata(&candidate.old_path).is_ok() {
            return Err("a file still exists at the old name".into());
        }
        let receipt = store.quick_check(&candidate.set_id)?;
        store.link_source(&candidate.new_path, now, &candidate.set_id)?;
        let _ = fs::remove_file(store.source_link_path(&candidate.old_path));
        sync_directory(&store.sources_dir());
        store.journal(
            "source_link_followed_rename",
            &candidate.set_id,
            &format!(
                "{} -> {}",
                candidate.old_path.display(),
                candidate.new_path.display()
            ),
        );
        Ok(store.summary_of(&receipt))
    }

    /// Looks for an existing prepared copy of exactly this ZIP's disc and, if
    /// there is one, links the ZIP to it. Never extracts: the ZIP is read in
    /// full and every file must match the copy's recorded SHA-256. This is how
    /// a ZIP renamed or moved outside EmuWiz finds its copy again.
    pub fn relink(
        &self,
        source: &Path,
        trusted: &TrustedRoots,
        limits: &DiscSetLimits,
        cancel: &AtomicBool,
        progress: &dyn Fn(PrepareProgress<'_>),
    ) -> Result<RelinkOutcome, PrepareError> {
        let Some(store) = self.existing() else {
            return Ok(RelinkOutcome::NoMatchingCopy);
        };
        let _lock = store.lock(cancel, true).map_err(|busy| {
            if !busy {
                PrepareError::Store("the lock file could not be opened".into())
            } else if cancel.load(Ordering::Relaxed) {
                PrepareError::Cancelled
            } else {
                PrepareError::Busy
            }
        })?;
        let inspection = inspect_disc_zip(source, trusted, limits, cancel).map_err(|refusal| {
            if refusal == DiscSetRefusal::Cancelled {
                PrepareError::Cancelled
            } else {
                PrepareError::Refused(refusal)
            }
        })?;
        let set_id = set_id_of(&inspection.fingerprint);
        if fs::symlink_metadata(store.set_dir(&set_id)).is_err() {
            return Ok(RelinkOutcome::NoMatchingCopy);
        }
        let listed = store.listed(&set_id, Some(&inspection.fingerprint));
        let summary = match (listed.state, listed.summary) {
            (SetState::Ready, Some(summary)) => summary,
            (SetState::Damaged(detail), _) => {
                return Err(PrepareError::ExistingCopyDamaged(detail));
            }
            (SetState::Ready, None) => {
                return Err(PrepareError::ExistingCopyDamaged("no summary".into()));
            }
        };
        store.source_matches_receipt(
            source,
            trusted,
            limits,
            cancel,
            progress,
            &inspection,
            &set_id,
        )?;
        store
            .link_source(source, inspection.source, &set_id)
            .map_err(PrepareError::Store)?;
        let mut gone = store.retire_stale_links(&set_id);
        let moved_from = if gone.len() == 1 { gone.pop() } else { None };
        store.journal(
            "source_linked_by_content",
            &set_id,
            &source.display().to_string(),
        );
        Ok(RelinkOutcome::Linked {
            summary: Box::new(summary),
            moved_from,
        })
    }

    /// The verified prepared copy for this exact, unchanged ZIP, found without
    /// reading the ZIP: one small index record, the ZIP's own metadata and the
    /// set's quick check. Cheap enough for readiness; `None` whenever anything
    /// does not match (the ZIP changed, the set is damaged or gone).
    pub fn prepared_for_source(&self, source: &Path) -> Option<PreparedSetSummary> {
        let metadata = fs::symlink_metadata(source).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let store = self.existing()?;
        let link: SourceLink = read_json(&store.source_link_path(source)).ok()?;
        if link.format_version != FORMAT_VERSION
            || link.source_path != source
            || link.source != SourceIdentity::of(&metadata)
            || !valid_set_id(&link.set_id)
        {
            return None;
        }
        let receipt = store.quick_check(&link.set_id).ok()?;
        Some(store.summary_of(&receipt))
    }

    /// Creates the store's folders (0700) and proves that nothing below the
    /// root is a symlink. The root itself may sit behind the person's own
    /// symlinks (a data folder on another disk); it is resolved once.
    fn ensure_layout(&self) -> Result<PreparedMediaStore, String> {
        if !self.root.is_absolute() {
            return Err("the store root is not an absolute path".into());
        }
        let mut builder = fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.recursive(true);
        builder
            .create(&self.root)
            .map_err(|error| format!("{}: {error}", self.root.display()))?;
        let resolved = fs::canonicalize(&self.root)
            .map_err(|error| format!("{}: {error}", self.root.display()))?;
        let store = Self::at(resolved);
        // One level at a time, each proven to be a plain folder before
        // anything is created inside it: a planted link is refused before it
        // is ever written through.
        let mut one = fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut one, 0o700);
        for directory in [
            store.versioned(),
            store.sets_dir(),
            store.staging_dir(),
            store.sources_dir(),
        ] {
            match one.create(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("{}: {error}", directory.display())),
            }
            open_directory_no_symlinks(&directory).map_err(|refusal| {
                format!(
                    "{} is not a plain folder ({refusal:?})",
                    directory.display()
                )
            })?;
        }
        Ok(store)
    }

    /// This store with its folders created and checked (see `ensure_layout`).
    pub fn ready(&self) -> Result<PreparedMediaStore, String> {
        self.ensure_layout()
    }

    /// This store resolved, if its folders already exist. Creates nothing.
    pub fn resolved(&self) -> Option<PreparedMediaStore> {
        self.existing()
    }

    /// The resolved store when its folders already exist; never creates any.
    fn existing(&self) -> Option<PreparedMediaStore> {
        let resolved = fs::canonicalize(&self.root).ok()?;
        let store = Self::at(resolved);
        // Every folder this store reads, moves into or deletes from must be
        // a plain folder; a link anywhere means the store is not used at all.
        for directory in [store.sets_dir(), store.staging_dir(), store.sources_dir()] {
            open_directory_no_symlinks(&directory).ok()?;
        }
        Some(store)
    }

    fn lock(&self, cancel: &AtomicBool, wait: bool) -> Result<StoreLock, bool> {
        use std::os::fd::AsRawFd;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.versioned().join("lock"))
            .map_err(|_| false)?;
        loop {
            // SAFETY: the descriptor is open; flock has no memory effects.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(StoreLock(file));
            }
            if !wait || cancel.load(Ordering::Relaxed) {
                return Err(true);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn journal(&self, event: &str, set_id: &str, detail: &str) {
        let line = serde_json::json!({
            "at_unix": now_unix(),
            "event": event,
            "set_id": set_id,
            "detail": detail,
        });
        if let Ok(mut file) = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.versioned().join("journal.jsonl"))
        {
            let _ = writeln!(file, "{line}");
            let _ = file.sync_all();
        }
    }

    /// What `prepare` would do. Pure description; nothing is created.
    pub fn plan(&self, inspection: &DiscSetInspection) -> PreparationPlan {
        let set_id = set_id_of(&inspection.fingerprint);
        PreparationPlan {
            destination: self.set_dir(&set_id),
            set_id,
            files: inspection.members.clone(),
            total_bytes: inspection.total_bytes,
            required_free_bytes: inspection
                .total_bytes
                .saturating_add(FREE_SPACE_RESERVE_BYTES),
            steps: vec![
                "Read each file out of the ZIP and check it against the checksum the ZIP records"
                    .into(),
                "Write the files as new files in EmuWiz's prepared-media folder".into(),
                "Read the new files back from disk and check them again".into(),
                "Check that the cue sheet names exactly these files and the data track is a CD image"
                    .into(),
                "Check that the ZIP did not change meanwhile, then publish the folder in one step"
                    .into(),
            ],
        }
    }

    fn summary_of(&self, receipt: &Receipt) -> PreparedSetSummary {
        let directory = self.set_dir(&receipt.set_id);
        PreparedSetSummary {
            set_id: receipt.set_id.clone(),
            fingerprint: receipt.fingerprint.clone(),
            platform_id: receipt.platform_id.clone(),
            launch_path: directory.join(MEDIA_DIR).join(&receipt.files[0].name),
            directory,
            file_count: receipt.files.len(),
            total_bytes: receipt.total_bytes,
            created_unix: receipt.created_unix,
            source_path: receipt.source_path.clone(),
            disc: receipt.disc.clone(),
        }
    }

    fn read_receipt(&self, set_id: &str) -> Result<Receipt, String> {
        let receipt: Receipt = read_json(&self.set_dir(set_id).join(RECEIPT_NAME))?;
        if receipt.format_version != FORMAT_VERSION {
            return Err(format!(
                "receipt format {} is not understood",
                receipt.format_version
            ));
        }
        if receipt.set_id != set_id || set_id_of(&receipt.fingerprint) != set_id {
            return Err("the receipt belongs to another set".into());
        }
        if receipt.files.is_empty() || receipt.files[0].role != DiscMemberRole::CueSheet {
            return Err("the receipt lists no cue sheet".into());
        }
        if receipt
            .files
            .iter()
            .any(|file| !super::cue_sheet::is_plain_file_name(&file.name))
        {
            return Err("the receipt lists an unsafe file name".into());
        }
        Ok(receipt)
    }

    /// The quick check used before every launch: the receipt is intact and
    /// every file it lists is still the same regular file (inode, size,
    /// modification and change times). No file contents are read.
    fn quick_check(&self, set_id: &str) -> Result<Receipt, String> {
        let receipt = self.read_receipt(set_id)?;
        let media = self.set_dir(set_id).join(MEDIA_DIR);
        for file in &receipt.files {
            let metadata = fs::symlink_metadata(media.join(&file.name))
                .map_err(|error| format!("{}: {error}", file.name))?;
            if !metadata.is_file() {
                return Err(format!("{} is no longer a regular file", file.name));
            }
            let identity = SourceIdentity::of(&metadata);
            if identity.size_bytes != file.size_bytes
                || identity.inode != file.inode
                || identity.modified_ns != file.modified_ns
                || identity.changed_ns != file.changed_ns
            {
                return Err(format!("{} changed after it was verified", file.name));
            }
        }
        Ok(receipt)
    }

    /// The verified prepared copy holding exactly this disc set, if any.
    pub fn lookup(&self, fingerprint: &str) -> Option<ListedSet> {
        let store = self.existing()?;
        let set_id = set_id_of(fingerprint);
        if !valid_set_id(&set_id) || fs::symlink_metadata(store.set_dir(&set_id)).is_err() {
            return None;
        }
        Some(store.listed(&set_id, Some(fingerprint)))
    }

    fn listed(&self, set_id: &str, fingerprint: Option<&str>) -> ListedSet {
        match self.quick_check(set_id) {
            Ok(receipt) if fingerprint.is_none_or(|wanted| wanted == receipt.fingerprint) => {
                ListedSet {
                    set_id: set_id.to_string(),
                    state: SetState::Ready,
                    summary: Some(self.summary_of(&receipt)),
                }
            }
            Ok(_) => ListedSet {
                set_id: set_id.to_string(),
                state: SetState::Damaged("the receipt describes different content".into()),
                summary: None,
            },
            Err(detail) => ListedSet {
                set_id: set_id.to_string(),
                state: SetState::Damaged(detail),
                summary: None,
            },
        }
    }

    /// Every committed set, with its quick-check state. Read-only.
    pub fn list(&self) -> Vec<ListedSet> {
        let Some(store) = self.existing() else {
            return Vec::new();
        };
        let Ok(entries) = fs::read_dir(store.sets_dir()) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| valid_set_id(name))
            .collect();
        ids.sort();
        ids.iter().map(|id| store.listed(id, None)).collect()
    }

    /// Staging folders left by a preparation that did not finish. Read-only.
    pub fn interrupted(&self) -> Vec<InterruptedPreparation> {
        let Some(store) = self.existing() else {
            return Vec::new();
        };
        let Ok(entries) = fs::read_dir(store.staging_dir()) else {
            return Vec::new();
        };
        let mut found: Vec<InterruptedPreparation> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
            .map(|entry| {
                let directory = entry.path();
                let intent: Option<Intent> = read_json(&directory.join(INTENT_NAME)).ok();
                InterruptedPreparation {
                    set_id: intent.as_ref().map(|intent| intent.set_id.clone()),
                    source_path: intent.as_ref().map(|intent| intent.source_path.clone()),
                    started_unix: intent.as_ref().map(|intent| intent.started_unix),
                    directory,
                }
            })
            .collect();
        found.sort_by(|a, b| a.directory.cmp(&b.directory));
        found
    }

    /// Removes exactly the files a folder's own records list, then the folder
    /// if it is empty. Returns what was left behind.
    fn remove_owned_tree(
        directory: &Path,
        names: &[String],
        sizes: Option<&[u64]>,
    ) -> OwnedRemoval {
        let media = directory.join(MEDIA_DIR);
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        let mut freed = 0_u64;
        let plain_folder =
            |path: &Path| fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir());
        // A folder that was swapped for a link is never deleted through.
        if !plain_folder(directory) {
            if fs::symlink_metadata(directory).is_ok() {
                kept.push(directory.to_path_buf());
            }
            return (removed, kept, freed);
        }
        let media_exists = fs::symlink_metadata(&media).is_ok();
        if media_exists && !plain_folder(&media) {
            kept.push(media);
            kept.push(directory.to_path_buf());
            return (removed, kept, freed);
        }
        for (index, name) in names.iter().enumerate() {
            if !super::cue_sheet::is_plain_file_name(name) {
                continue;
            }
            let path = media.join(name);
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            let expected = sizes.and_then(|sizes| sizes.get(index)).copied();
            if !metadata.is_file() || expected.is_some_and(|size| size != metadata.len()) {
                kept.push(path);
                continue;
            }
            // Prepared files are read-only; removal needs only the folder.
            if fs::remove_file(&path).is_ok() {
                freed = freed.saturating_add(metadata.len());
                removed.push(name.clone());
            } else {
                kept.push(path);
            }
        }
        for record in [RECEIPT_NAME, INTENT_NAME] {
            let path = directory.join(record);
            if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                let _ = fs::remove_file(&path);
            }
        }
        // `remove_dir` refuses a non-empty folder, so anything unexpected
        // keeps its folder and is reported.
        if fs::remove_dir(&media).is_err() && fs::symlink_metadata(&media).is_ok() {
            kept.push(media);
        }
        if fs::remove_dir(directory).is_err() && fs::symlink_metadata(directory).is_ok() {
            kept.push(directory.to_path_buf());
        }
        (removed, kept, freed)
    }

    /// Removes the staging folders of preparations that did not finish. It
    /// takes the store lock, so it can never touch one that is still running.
    pub fn clean_interrupted(&self) -> Result<Vec<RemovalReport>, RemoveError> {
        let Some(store) = self.existing() else {
            return Ok(Vec::new());
        };
        let never = AtomicBool::new(false);
        let _lock = store.lock(&never, false).map_err(|busy| {
            if busy {
                RemoveError::Busy
            } else {
                RemoveError::Store("the lock file could not be opened".into())
            }
        })?;
        let mut reports = Vec::new();
        for interrupted in store.interrupted() {
            // A removal that was cut short parked the whole set in `set/`,
            // with its receipt: finish it by the receipt, exactly as `remove`.
            let parked = interrupted.directory.join("set");
            let parked_receipt: Option<Receipt> = read_json(&parked.join(RECEIPT_NAME)).ok();
            let (removed_files, mut kept, freed_bytes) = match parked_receipt {
                Some(receipt) => {
                    let names: Vec<String> =
                        receipt.files.iter().map(|file| file.name.clone()).collect();
                    let sizes: Vec<u64> =
                        receipt.files.iter().map(|file| file.size_bytes).collect();
                    let result = Self::remove_owned_tree(&parked, &names, Some(&sizes));
                    let _ = fs::remove_dir(&interrupted.directory);
                    result
                }
                None => {
                    let intent: Option<Intent> =
                        read_json(&interrupted.directory.join(INTENT_NAME)).ok();
                    let names = intent.map(|intent| intent.files).unwrap_or_default();
                    Self::remove_owned_tree(&interrupted.directory, &names, None)
                }
            };
            if kept.is_empty() && fs::symlink_metadata(&interrupted.directory).is_ok() {
                kept.push(interrupted.directory.clone());
            }
            let set_id = interrupted.set_id.unwrap_or_default();
            store.journal(
                "interrupted_preparation_cleaned",
                &set_id,
                &interrupted.directory.display().to_string(),
            );
            reports.push(RemovalReport {
                set_id,
                removed_files,
                kept,
                freed_bytes,
            });
        }
        sync_directory(&store.staging_dir());
        test_hooks::fire(test_hooks::Point::AfterClean);
        Ok(reports)
    }

    /// Removes a prepared copy: the Undo of `prepare`. Only files the receipt
    /// lists, and only while they still have the recorded size. The original
    /// ZIP is not involved in any way.
    pub fn remove(&self, set_id: &str) -> Result<RemovalReport, RemoveError> {
        if !valid_set_id(set_id) {
            return Err(RemoveError::NotFound);
        }
        let store = self.existing().ok_or(RemoveError::NotFound)?;
        let never = AtomicBool::new(false);
        let _lock = store.lock(&never, false).map_err(|busy| {
            if busy {
                RemoveError::Busy
            } else {
                RemoveError::Store("the lock file could not be opened".into())
            }
        })?;
        let directory = store.set_dir(set_id);
        if !fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(RemoveError::NotFound);
        }
        let receipt = store
            .read_receipt(set_id)
            .map_err(RemoveError::NoUsableReceipt)?;
        // Take the set out of sight first, so no launch can pick up a set
        // that is half removed.
        let retired = tempfile::Builder::new()
            .prefix(&format!("removing-{set_id}-"))
            .tempdir_in(store.staging_dir())
            .map_err(|error| RemoveError::Store(error.to_string()))?
            .keep();
        let parked = retired.join("set");
        rename_noreplace(&directory, &parked)
            .map_err(|error| RemoveError::Store(error.to_string()))?;
        sync_directory(&store.sets_dir());
        test_hooks::fire(test_hooks::Point::AfterRemovalParked);
        let names: Vec<String> = receipt.files.iter().map(|file| file.name.clone()).collect();
        let sizes: Vec<u64> = receipt.files.iter().map(|file| file.size_bytes).collect();
        let (removed_files, mut kept, freed_bytes) =
            Self::remove_owned_tree(&parked, &names, Some(&sizes));
        if fs::remove_dir(&retired).is_err()
            && fs::symlink_metadata(&retired).is_ok()
            && kept.is_empty()
        {
            kept.push(retired.clone());
        }
        sync_directory(&store.staging_dir());
        store.unlink_sources_of(set_id);
        store.journal(
            "prepared_copy_removed",
            set_id,
            &format!("{} files, {} kept", removed_files.len(), kept.len()),
        );
        Ok(RemovalReport {
            set_id: set_id.to_string(),
            removed_files,
            kept,
            freed_bytes,
        })
    }

    /// Reads every file of a set back from disk and compares it with the
    /// receipt's SHA-256. Read-only.
    pub fn verify_full(
        &self,
        set_id: &str,
        cancel: &AtomicBool,
    ) -> Result<PreparedSetSummary, String> {
        if !valid_set_id(set_id) {
            return Err("no prepared copy with that id exists".into());
        }
        let store = self
            .existing()
            .ok_or_else(|| "no prepared copy with that id exists".to_string())?;
        let receipt = store.quick_check(set_id)?;
        let media = store.set_dir(set_id).join(MEDIA_DIR);
        for file in &receipt.files {
            let handle = open_existing_no_follow(&media.join(&file.name))
                .map_err(|error| error.to_string())?;
            let hashed = hash_member_stream(handle, file.size_bytes, cancel)
                .map_err(|error| format!("{}: {error:?}", file.name))?;
            if hashed.bytes_read != file.size_bytes
                || !hashed.hashes.sha256.eq_ignore_ascii_case(&file.sha256)
            {
                return Err(format!(
                    "{} no longer matches its recorded checksum",
                    file.name
                ));
            }
        }
        Ok(store.summary_of(&receipt))
    }

    /// Whether a launch may start `content`. A path outside the store is not
    /// this module's business; a path inside it must be the launch file of a
    /// set that passes the quick check.
    pub fn check_launch_path(&self, content: &Path) -> LaunchPathCheck {
        let Ok(resolved_root) = fs::canonicalize(&self.root) else {
            return LaunchPathCheck::NotPreparedMedia;
        };
        // Judge where the file really is, not how its path is spelled: the
        // folder is resolved through any links and `..`, the file name kept
        // (a linked file itself is refused by the launcher).
        let resolved_content = match (content.parent(), content.file_name()) {
            (Some(parent), Some(name)) => match fs::canonicalize(parent) {
                Ok(parent) => parent.join(name),
                Err(_) => return LaunchPathCheck::NotPreparedMedia,
            },
            _ => return LaunchPathCheck::NotPreparedMedia,
        };
        let content = resolved_content.as_path();
        if !content.starts_with(&resolved_root) {
            return LaunchPathCheck::NotPreparedMedia;
        }
        let store = Self::at(resolved_root);
        let relative = match content.strip_prefix(store.sets_dir()) {
            Ok(relative) => relative,
            Err(_) => {
                return LaunchPathCheck::Refused(
                    "this file is inside the prepared-media folder but is not a finished prepared copy"
                        .into(),
                );
            }
        };
        let parts: Vec<_> = relative.components().collect();
        let [
            std::path::Component::Normal(set_id),
            std::path::Component::Normal(media),
            std::path::Component::Normal(name),
        ] = parts.as_slice()
        else {
            return LaunchPathCheck::Refused(
                "this is not the launch file of a prepared copy".into(),
            );
        };
        let (Some(set_id), Some(name)) = (set_id.to_str(), name.to_str()) else {
            return LaunchPathCheck::Refused(
                "this is not the launch file of a prepared copy".into(),
            );
        };
        if *media != std::ffi::OsStr::new(MEDIA_DIR) || !valid_set_id(set_id) {
            return LaunchPathCheck::Refused(
                "this is not the launch file of a prepared copy".into(),
            );
        }
        if open_directory_no_symlinks(&store.set_dir(set_id).join(MEDIA_DIR)).is_err() {
            return LaunchPathCheck::Refused(
                "the prepared copy's folder is not a plain folder".into(),
            );
        }
        match store.quick_check(set_id) {
            Ok(receipt) if receipt.files[0].name == name => {
                LaunchPathCheck::Verified(store.summary_of(&receipt))
            }
            Ok(_) => {
                LaunchPathCheck::Refused("only the prepared copy's cue sheet may be started".into())
            }
            Err(detail) => LaunchPathCheck::Refused(format!(
                "the prepared copy no longer passes its checks: {detail}. Remove it and prepare it again"
            )),
        }
    }

    /// Extracts the disc set in `source` into a new, verified prepared copy.
    ///
    /// `source` is opened read-only. Nothing that exists is overwritten: if a
    /// verified copy of this exact content is already present it is returned
    /// untouched, and a damaged one is an error.
    pub fn prepare(
        &self,
        source: &Path,
        platform_id: &str,
        trusted: &TrustedRoots,
        limits: &DiscSetLimits,
        cancel: &AtomicBool,
        progress: &dyn Fn(PrepareProgress<'_>),
    ) -> Result<PrepareOutcome, PrepareError> {
        let store = self.ensure_layout().map_err(PrepareError::Store)?;
        let _lock = store.lock(cancel, true).map_err(|busy| {
            if !busy {
                PrepareError::Store("the lock file could not be opened".into())
            } else if cancel.load(Ordering::Relaxed) {
                PrepareError::Cancelled
            } else {
                PrepareError::Busy
            }
        })?;
        let inspection = inspect_disc_zip(source, trusted, limits, cancel).map_err(|refusal| {
            if refusal == DiscSetRefusal::Cancelled {
                PrepareError::Cancelled
            } else {
                PrepareError::Refused(refusal)
            }
        })?;
        let set_id = set_id_of(&inspection.fingerprint);
        let final_dir = store.set_dir(&set_id);
        if fs::symlink_metadata(&final_dir).is_ok() {
            let listed = store.listed(&set_id, Some(&inspection.fingerprint));
            return match (listed.state, listed.summary) {
                (SetState::Ready, Some(summary)) => {
                    // Same names, sizes and ZIP checksums are not yet the
                    // same bytes: this ZIP is decoded and every member must
                    // match the prepared copy's recorded SHA-256 before it
                    // may use that copy.
                    store.source_matches_receipt(
                        source,
                        trusted,
                        limits,
                        cancel,
                        progress,
                        &inspection,
                        &set_id,
                    )?;
                    let link_warning = store.link_source(source, inspection.source, &set_id).err();
                    store.retire_stale_links(&set_id);
                    Ok(PrepareOutcome {
                        summary,
                        already_prepared: true,
                        link_warning,
                    })
                }
                (SetState::Damaged(detail), _) => Err(PrepareError::ExistingCopyDamaged(detail)),
                (SetState::Ready, None) => {
                    Err(PrepareError::ExistingCopyDamaged("no summary".into()))
                }
            };
        }
        let needed = inspection
            .total_bytes
            .saturating_add(FREE_SPACE_RESERVE_BYTES);
        let available = available_bytes(&store.staging_dir()).map_err(PrepareError::Store)?;
        if available < needed {
            return Err(PrepareError::NotEnoughSpace {
                needed_bytes: needed,
                available_bytes: available,
            });
        }

        let staging = tempfile::Builder::new()
            .prefix(&format!("{set_id}-"))
            .tempdir_in(store.staging_dir())
            .map_err(|error| PrepareError::Store(error.to_string()))?
            .keep();
        let names: Vec<String> = inspection
            .members
            .iter()
            .map(|member| member.name.clone())
            .collect();
        let result = store.fill_and_publish(
            source,
            platform_id,
            trusted,
            limits,
            cancel,
            progress,
            &inspection,
            &set_id,
            &staging,
        );
        match result {
            Ok(summary) => {
                store.journal(
                    "prepared_copy_created",
                    &set_id,
                    &source.display().to_string(),
                );
                // The copy is published and verified whatever happens to
                // the index record; a failed record is reported, not turned
                // into "nothing was prepared".
                let link_warning = store.link_source(source, inspection.source, &set_id).err();
                Ok(PrepareOutcome {
                    summary,
                    already_prepared: false,
                    link_warning,
                })
            }
            Err(error) => {
                // Leave nothing behind from a failed attempt. Whatever cannot
                // be removed stays as an interrupted preparation.
                let _ = Self::remove_owned_tree(&staging, &names, None);
                sync_directory(&store.staging_dir());
                store.journal("preparation_failed", &set_id, &error.to_string());
                Err(error)
            }
        }
    }

    /// Decodes every set member of `source` (nothing is written) and compares
    /// its SHA-256 with the prepared copy's receipt.
    #[allow(clippy::too_many_arguments)]
    fn source_matches_receipt(
        &self,
        source: &Path,
        trusted: &TrustedRoots,
        limits: &DiscSetLimits,
        cancel: &AtomicBool,
        progress: &dyn Fn(PrepareProgress<'_>),
        inspection: &DiscSetInspection,
        set_id: &str,
    ) -> Result<(), PrepareError> {
        use sha2::{Digest, Sha256};
        struct Hashing(Sha256);
        impl Write for Hashing {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let receipt = self
            .quick_check(set_id)
            .map_err(PrepareError::ExistingCopyDamaged)?;
        let archive_limits = limits.archive_limits();
        let count = inspection.members.len();
        for (index, member) in inspection.members.iter().enumerate() {
            progress(PrepareProgress::Verifying {
                name: &member.name,
                index,
                count,
            });
            let recorded = receipt
                .files
                .iter()
                .find(|file| file.name == member.name)
                .ok_or_else(|| {
                    PrepareError::Verification(format!(
                        "{} is not in the existing prepared copy",
                        member.name
                    ))
                })?;
            let mut hasher = Hashing(Sha256::new());
            crate::dat::archive::zip::stream_zip_member_into(
                source,
                trusted,
                &archive_limits,
                cancel,
                &ZipMemberRequest {
                    member_name: Some(member.name.clone()),
                    require_exact_name: true,
                    size_bytes: Some(member.size_bytes),
                    sha1: None,
                    crc32: Some(member.crc32.clone()),
                },
                &mut hasher,
            )
            .map_err(|error| match error {
                ZipMemberError::Cancelled => PrepareError::Cancelled,
                ZipMemberError::SourceChanged => PrepareError::SourceChanged,
                other => PrepareError::Extraction(format!("{}: {other}", member.name)),
            })?;
            let sha256: String = hasher
                .0
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            if !sha256.eq_ignore_ascii_case(&recorded.sha256) {
                return Err(PrepareError::Verification(format!(
                    "{} has the same name, size and ZIP checksum as the existing prepared copy but different contents; the existing copy is not used for this ZIP",
                    member.name
                )));
            }
        }
        let now = fs::symlink_metadata(source).map_err(|_| PrepareError::SourceChanged)?;
        if !now.is_file() || SourceIdentity::of(&now) != inspection.source {
            return Err(PrepareError::SourceChanged);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_and_publish(
        &self,
        source: &Path,
        platform_id: &str,
        trusted: &TrustedRoots,
        limits: &DiscSetLimits,
        cancel: &AtomicBool,
        progress: &dyn Fn(PrepareProgress<'_>),
        inspection: &DiscSetInspection,
        set_id: &str,
        staging: &Path,
    ) -> Result<PreparedSetSummary, PrepareError> {
        let io = |error: std::io::Error| PrepareError::Store(error.to_string());
        let cancelled = || cancel.load(Ordering::Relaxed);
        write_json_new(
            &staging.join(INTENT_NAME),
            &Intent {
                format_version: FORMAT_VERSION,
                set_id: set_id.to_string(),
                fingerprint: inspection.fingerprint.clone(),
                source_path: source.to_path_buf(),
                files: inspection
                    .members
                    .iter()
                    .map(|member| member.name.clone())
                    .collect(),
                started_unix: now_unix(),
                process_id: std::process::id(),
            },
        )
        .map_err(PrepareError::Store)?;
        let media = staging.join(MEDIA_DIR);
        let mut builder = fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&media).map_err(io)?;
        test_hooks::fire(test_hooks::Point::AfterIntent);

        let archive_limits = limits.archive_limits();
        let count = inspection.members.len();
        let mut decoded_sha1 = Vec::with_capacity(count);
        for (index, member) in inspection.members.iter().enumerate() {
            if cancelled() {
                return Err(PrepareError::Cancelled);
            }
            progress(PrepareProgress::Extracting {
                name: &member.name,
                index,
                count,
            });
            let mut output = create_new_file(&media.join(&member.name), 0o600).map_err(io)?;
            let copy = copy_zip_member_into(
                source,
                trusted,
                &archive_limits,
                cancel,
                &ZipMemberRequest {
                    member_name: Some(member.name.clone()),
                    require_exact_name: true,
                    size_bytes: Some(member.size_bytes),
                    sha1: None,
                    crc32: Some(member.crc32.clone()),
                },
                &mut output,
            )
            .map_err(|error| match error {
                ZipMemberError::Cancelled => PrepareError::Cancelled,
                ZipMemberError::SourceChanged => PrepareError::SourceChanged,
                other => PrepareError::Extraction(format!("{}: {other}", member.name)),
            })?;
            output.sync_all().map_err(io)?;
            decoded_sha1.push(copy.sha1);
            test_hooks::fire(test_hooks::Point::AfterMember(index));
        }

        // Independent read-back: the bytes now on disk, not the bytes that
        // were written, are what must match.
        let mut files = Vec::with_capacity(count);
        for (index, member) in inspection.members.iter().enumerate() {
            if cancelled() {
                return Err(PrepareError::Cancelled);
            }
            progress(PrepareProgress::Verifying {
                name: &member.name,
                index,
                count,
            });
            let path = media.join(&member.name);
            // Read-only first, then one handle for both the hash and the
            // recorded identity, so what is recorded is what was hashed.
            fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).map_err(io)?;
            let handle = open_existing_no_follow(&path).map_err(io)?;
            let hashed =
                hash_member_stream(&handle, member.size_bytes, cancel).map_err(|error| {
                    if cancelled() {
                        PrepareError::Cancelled
                    } else {
                        PrepareError::Verification(format!("{}: {error:?}", member.name))
                    }
                })?;
            if hashed.bytes_read != member.size_bytes
                || !hashed.hashes.crc32.eq_ignore_ascii_case(&member.crc32)
                || !hashed
                    .hashes
                    .sha1
                    .eq_ignore_ascii_case(&decoded_sha1[index])
            {
                return Err(PrepareError::Verification(format!(
                    "{} read back from disk does not match the ZIP's checksum",
                    member.name
                )));
            }
            let identity = SourceIdentity::of(&handle.metadata().map_err(io)?);
            files.push((member, hashed.hashes.sha256, identity));
        }

        // Structure: the extracted sheet must be the sheet that was planned,
        // and its tracks must be what it says they are.
        let cue_bytes = fs::read(media.join(inspection.cue_name())).map_err(io)?;
        let sheet = parse_cue_sheet(&cue_bytes)
            .map_err(|refusal| PrepareError::Verification(refusal.to_string()))?;
        if sheet != inspection.sheet {
            return Err(PrepareError::Verification(
                "the extracted cue sheet differs from the one that was inspected".into(),
            ));
        }
        let mut disc = None;
        for cue_file in &sheet.files {
            let mut handle = open_existing_no_follow(&media.join(&cue_file.name)).map_err(io)?;
            match cue_file.kind {
                CueFileKind::Wave => {
                    if !is_riff_wave(&mut handle) {
                        return Err(PrepareError::Verification(format!(
                            "{} is named as WAVE audio but is not a WAV file",
                            cue_file.name
                        )));
                    }
                }
                CueFileKind::Binary => {
                    if disc.is_none()
                        && let Some(track) = cue_file
                            .tracks
                            .iter()
                            .find(|track| track.mode != CueTrackMode::Audio)
                    {
                        disc = Some(read_disc_facts(&mut handle, track.mode).ok_or_else(|| {
                            PrepareError::Verification(format!(
                                "the data track {} has no ISO 9660 volume descriptor",
                                cue_file.name
                            ))
                        })?);
                    }
                }
            }
        }

        // A disc whose own label names no Commodore CD system is not prepared
        // as a CD32 game, whatever folder its ZIP is in. ("CDTV" is what both
        // CDTV and CD32 discs carry; it cannot tell the two apart.)
        if platform_id == crate::amiga_cd_evidence::AMIGA_CD32_PLATFORM_ID
            && !disc.as_ref().is_some_and(|facts: &DiscFacts| {
                facts
                    .system_identifier
                    .eq_ignore_ascii_case(COMMODORE_CD_SYSTEM_IDENTIFIER)
            })
        {
            return Err(PrepareError::Verification(format!(
                "the disc's own system label is \"{}\", not the \"{COMMODORE_CD_SYSTEM_IDENTIFIER}\" label Amiga CD32 discs carry",
                disc.as_ref()
                    .map_or("(none)", |facts| facts.system_identifier.as_str())
            )));
        }

        // The ZIP must still be the file that was inspected.
        let now = fs::symlink_metadata(source).map_err(|_| PrepareError::SourceChanged)?;
        if !now.is_file() || SourceIdentity::of(&now) != inspection.source {
            return Err(PrepareError::SourceChanged);
        }
        test_hooks::fire(test_hooks::Point::BeforeReceipt);

        let mut receipt_files = Vec::with_capacity(count);
        for (member, sha256, identity) in files {
            receipt_files.push(ReceiptFile {
                name: member.name.clone(),
                role: member.role,
                size_bytes: member.size_bytes,
                crc32: member.crc32.clone(),
                sha256,
                inode: identity.inode,
                modified_ns: identity.modified_ns,
                changed_ns: identity.changed_ns,
            });
        }
        let receipt = Receipt {
            format_version: FORMAT_VERSION,
            set_id: set_id.to_string(),
            fingerprint: inspection.fingerprint.clone(),
            platform_id: platform_id.to_string(),
            created_unix: now_unix(),
            source_path: source.to_path_buf(),
            source: inspection.source,
            files: receipt_files,
            total_bytes: inspection.total_bytes,
            disc,
        };
        write_json_new(&staging.join(RECEIPT_NAME), &receipt).map_err(PrepareError::Store)?;
        sync_directory(&media);
        sync_directory(staging);
        if cancelled() {
            return Err(PrepareError::Cancelled);
        }
        progress(PrepareProgress::Publishing);
        test_hooks::fire(test_hooks::Point::BeforePublish);
        match rename_noreplace(staging, &self.set_dir(set_id)) {
            Ok(()) => {}
            Err(NoClobberError::DestinationExists) => {
                return Err(PrepareError::ExistingCopyDamaged(
                    "a folder with this id appeared during preparation".into(),
                ));
            }
            Err(error) => return Err(PrepareError::Store(error.to_string())),
        }
        sync_directory(&self.sets_dir());
        sync_directory(&self.staging_dir());
        // Published: the quick check now is the same one a launch will run.
        self.quick_check(set_id)
            .map(|receipt| self.summary_of(&receipt))
            .map_err(PrepareError::Verification)
    }
}

/// Why a launch of `content` must be refused, or `None` when the path is
/// outside this profile's store or is the launch file of a verified prepared
/// copy. This only ever adds a refusal; it grants nothing.
pub fn launch_refusal(content: &Path) -> Option<String> {
    let store = PreparedMediaStore::for_profile().ok()?;
    match store.check_launch_path(content) {
        LaunchPathCheck::Refused(detail) => Some(detail),
        LaunchPathCheck::NotPreparedMedia | LaunchPathCheck::Verified(_) => None,
    }
}

/// Test-only fault injection: lets a test stop a preparation at a named
/// point, exactly as a crash or a kill would.
pub(crate) mod test_hooks {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[allow(dead_code)]
    pub(crate) enum Point {
        AfterIntent,
        AfterMember(usize),
        BeforeReceipt,
        BeforePublish,
        AfterRemovalParked,
        AfterClean,
    }

    #[cfg(test)]
    pub(crate) type Hook = Box<dyn Fn(Point)>;

    #[cfg(test)]
    thread_local! {
        pub(crate) static HOOK: std::cell::RefCell<Option<Hook>> =
            const { std::cell::RefCell::new(None) };
    }

    #[cfg(test)]
    pub(crate) fn fire(point: Point) {
        HOOK.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook(point);
            }
        });
    }

    #[cfg(not(test))]
    #[inline(always)]
    pub(crate) fn fire(_point: Point) {}
}
