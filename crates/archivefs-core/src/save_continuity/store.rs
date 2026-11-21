//! Content-addressed snapshot store.
//!
//! ```text
//! <root>/objects/ab/<sha256>          one file per distinct content, read-only
//! <root>/manifests/<lineage>/<time>-<id>.json   one immutable record per snapshot
//! <root>/tmp/                         work in progress; never read as data
//! ```
//!
//! A snapshot is published in this order: every object first (written to
//! `tmp`, flushed, then linked into `objects` under its SHA-256 without ever
//! replacing a file), the manifest last (same way). A crash at any point
//! leaves either no manifest, or a manifest whose objects are all present.
//! Leftovers in `tmp` are reported by [`SnapshotStore::health`], never used.
//!
//! The same bytes are stored once, however many snapshots name them.
//!
//! Nothing here deletes. [`SnapshotStore::pruning_proposal`] only lists what
//! a person could remove and what that would free.

use super::model::InUse;
use super::status::{GameIntegrity, PrecedingSession, StructureCheck};
use super::{ContinuityError, io, now_unix, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MANIFEST_FORMAT_VERSION: u32 = 1;
/// Largest single file a snapshot will take (a PS2 card is 8 MiB; the
/// largest common card, 64 MiB with ECC, is 66 MiB).
pub const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_FILES_PER_SNAPSHOT: usize = 4096;
/// Free space that must remain on the store's filesystem after a write.
pub const FREE_SPACE_RESERVE_BYTES: u64 = 64 * 1024 * 1024;
/// The record a recovery writes beside the recovered files. No snapshot may
/// hold a file of this name, so the record can never replace one.
pub const RECOVERY_RECEIPT_NAME: &str = "EMUWIZ-RECOVERY.json";
/// Default ceiling for everything the store holds.
pub const DEFAULT_BUDGET_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// How it was established that the save was not being written while it was
/// copied. Recorded in the manifest so a timeline never overstates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureBoundary {
    /// A reviewed check found the emulator closed before and after the copy,
    /// and the file read the same twice.
    EmulatorClosedObserved,
    /// The file read the same twice, but EmuWiz has no reliable way to tell
    /// whether this emulator was running. The copy may be mid-write.
    StableReadOnly,
    /// Arrived from another device, already a snapshot there.
    Imported,
}

impl CaptureBoundary {
    pub fn label(self) -> &'static str {
        match self {
            Self::EmulatorClosedObserved => "Taken with the emulator closed",
            Self::StableReadOnly => "Emulator state not confirmed: may have been taken mid-write",
            Self::Imported => "Received from another device",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureReason {
    Manual,
    SessionStart,
    SessionEnd,
    BeforeContinuation,
    Imported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotFile {
    /// Name inside the snapshot: a plain relative path, no `..`.
    pub name: String,
    pub size_bytes: u64,
    pub sha256: String,
    /// The original's modification time when read (nanoseconds since 1970).
    pub source_mtime_ns: i64,
    /// Other names on disk shared the same data when it was read.
    pub source_hard_links: u64,
}

/// One game save inside a snapshotted container (a card holds many).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainedSave {
    /// `ps2:BASLUS-20827MANHUNT`: the same save has the same key on any device.
    pub key: String,
    pub name: String,
    pub product_code: Option<String>,
    pub title: Option<String>,
    /// SHA-256 of this save's own content.
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub device_id: String,
    pub emulator: String,
    pub profile: String,
    pub emulator_version: Option<String>,
    pub platform: Option<String>,
    /// Where the files were read from.
    pub source_root: PathBuf,
}

/// Everything a snapshot says. The snapshot's id is the SHA-256 of this,
/// so a manifest cannot be edited without its id no longer matching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotBody {
    /// Which save location on which device this history belongs to.
    pub lineage: String,
    /// The snapshot of this location that came before, if any.
    pub parent: Option<String>,
    pub captured_unix: i64,
    pub provenance: Provenance,
    pub boundary: CaptureBoundary,
    pub reason: CaptureReason,
    pub session_id: Option<String>,
    pub files: Vec<SnapshotFile>,
    pub contained_saves: Vec<ContainedSave>,
    /// What was checked when the snapshot was taken.
    pub checks: Vec<String>,
    /// Whether the file has the shape its format requires. Separate from
    /// `boundary` (was the copy safe) on purpose; see [`super::status`].
    /// Absent on records written before this was tracked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<StructureCheck>,
    /// Whether the save's own data is valid as the game sees it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_integrity: Option<GameIntegrity>,
    /// How the emulator ended the last time it could have written this save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preceding_session: Option<PrecedingSession>,
}

impl SnapshotBody {
    /// **Snapshot integrity.** The copy itself can be relied on: taken at a
    /// verified safe point (emulator closed, nothing holding the save), and
    /// no structural problem was found. This is about the copy only. It is
    /// true or false whatever is known about how the emulator last ended.
    pub fn safe_copy(&self) -> bool {
        self.boundary == CaptureBoundary::EmulatorClosedObserved
            && !matches!(self.structure, Some(StructureCheck::Problems { .. }))
    }

    /// **Session status.** An abnormal termination was actually observed in
    /// the session before this snapshot. "Not known" is not this.
    pub fn after_observed_abnormal_end(&self) -> bool {
        self.preceding_session
            .as_ref()
            .is_some_and(|preceding| preceding.advisory() == super::status::Advisory::Warning)
    }

    /// A safe copy that also followed a session known to have ended
    /// normally. Says nothing about the game's own data.
    pub fn clean_recovery_point(&self) -> bool {
        self.safe_copy()
            && self
                .preceding_session
                .as_ref()
                .is_some_and(PrecedingSession::clean)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub format_version: u32,
    pub snapshot_id: String,
    pub body: SnapshotBody,
}

impl SnapshotManifest {
    pub fn total_bytes(&self) -> u64 {
        self.body.files.iter().map(|file| file.size_bytes).sum()
    }
}

pub(crate) fn body_id(body: &SnapshotBody) -> String {
    // Field order is fixed by the struct, so this text is canonical. A body
    // that cannot be written as JSON (a path that is not UTF-8) gets an empty
    // id, which is not a SHA-256 and so can never match or be published.
    match serde_json::to_string(body) {
        Ok(text) => sha256_hex(text.as_bytes()),
        Err(_) => String::new(),
    }
}

/// The key for one save location on one device.
pub fn lineage_key(device_id: &str, emulator: &str, source_root: &Path) -> String {
    let mut text = format!("v1\0{device_id}\0{emulator}\0").into_bytes();
    text.extend_from_slice(source_root.to_string_lossy().as_bytes());
    sha256_hex(&text)[..24].to_string()
}

/// Reports whether a save may be in use. The production implementation is
/// [`super::discover::ProcessObserver`]; tests supply their own.
pub trait InUseObserver {
    /// One look, now.
    fn observe(&self, emulator: &str, path: &Path) -> InUse;

    /// Starts guarding one file for the length of a copy. The default has no
    /// kernel evidence: it looks when asked and nothing more.
    fn guard<'a>(&'a self, emulator: &'a str, path: &'a Path) -> Box<dyn CopyGuard + 'a> {
        Box::new(LookOnly {
            observer: self,
            emulator,
            path,
        })
    }
}

/// Held from before a file is read until after. `state` is asked at both
/// ends; anything but [`InUse::NotDetected`] at either end means the copy is
/// not a verified safe point.
pub trait CopyGuard {
    fn state(&self) -> InUse;
    /// The kernel itself is watching the file for the whole copy (a lease):
    /// no program had it open at the start, and any program opening it for
    /// writing since is noticed, including ones that cannot be inspected.
    fn kernel_watched(&self) -> bool {
        false
    }
}

struct LookOnly<'a, O: InUseObserver + ?Sized> {
    observer: &'a O,
    emulator: &'a str,
    path: &'a Path,
}

impl<O: InUseObserver + ?Sized> CopyGuard for LookOnly<'_, O> {
    fn state(&self) -> InUse {
        self.observer.observe(self.emulator, self.path)
    }
}

/// One file to capture.
#[derive(Debug, Clone)]
pub struct CaptureFile {
    pub name: String,
    pub source: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub provenance: Provenance,
    pub files: Vec<CaptureFile>,
    pub contained_saves: Vec<ContainedSave>,
    pub reason: CaptureReason,
    pub session_id: Option<String>,
    /// Take the snapshot even when emulator closure cannot be established
    /// (it is then recorded as [`CaptureBoundary::StableReadOnly`]). A running
    /// emulator is always a refusal.
    pub allow_unproven_closure: bool,
    /// For repeatable tests; `None` is now.
    pub captured_unix: Option<i64>,
    /// Result of the format's structural check, if one was run.
    pub structure: StructureCheck,
    /// How the emulator last ended, when a session can be tied to this save.
    pub preceding_session: PrecedingSession,
    /// SHA-256 each file had when the saves inside it were listed
    /// (`contained_saves`), by snapshot file name. If a file read for the
    /// snapshot hashes to something else, the listing no longer describes
    /// it and the snapshot is refused.
    pub expected_sha256: BTreeMap<String, String>,
    /// For a save folder: the fingerprint the whole folder had when it was
    /// listed (every file's name, size and SHA-256, in name order). If the
    /// files read for the snapshot give another, the folder changed in
    /// between and the snapshot is refused.
    pub expected_folder_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CaptureOutcome {
    Created {
        manifest: SnapshotManifest,
        new_bytes: u64,
        reused_bytes: u64,
    },
    /// The newest snapshot of this location already holds exactly these bytes.
    Unchanged { existing: SnapshotManifest },
}

impl CaptureOutcome {
    pub fn manifest(&self) -> &SnapshotManifest {
        match self {
            Self::Created { manifest, .. } => manifest,
            Self::Unchanged { existing } => existing,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileCheck {
    Intact,
    Missing,
    /// The stored bytes hash to something else.
    Corrupt {
        actual_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifyReport {
    pub snapshot_id: String,
    /// The manifest still hashes to its own id.
    pub manifest_intact: bool,
    pub files: Vec<(String, FileCheck)>,
    pub bytes_checked: u64,
}

impl VerifyReport {
    pub fn intact(&self) -> bool {
        self.manifest_intact
            && self
                .files
                .iter()
                .all(|(_, check)| *check == FileCheck::Intact)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryPlan {
    pub snapshot_id: String,
    pub destination: PathBuf,
    pub files: Vec<SnapshotFile>,
    pub total_bytes: u64,
    /// Reasons this recovery would be refused. Empty means it can go ahead.
    pub blockers: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReceipt {
    pub snapshot_id: String,
    pub destination: PathBuf,
    pub recovered_unix: i64,
    pub files: Vec<SnapshotFile>,
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StoreUsage {
    pub snapshots: usize,
    pub objects: usize,
    /// Bytes actually on disk (each distinct content once).
    pub stored_bytes: u64,
    /// What the snapshots would take without sharing identical content.
    pub logical_bytes: u64,
    pub budget_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PruneCandidate {
    pub snapshot_id: String,
    pub lineage: String,
    pub captured_unix: i64,
    pub why: String,
    /// Bytes that only this snapshot (among those kept) still needs.
    pub would_free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PruningProposal {
    pub usage: StoreUsage,
    pub keep_newest_per_save: usize,
    pub candidates: Vec<PruneCandidate>,
    pub would_free_bytes: u64,
    /// Always true: this is a list for a person, nothing was removed.
    pub nothing_was_deleted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StoreHealth {
    /// Unfinished work left by an interrupted run. Safe to ignore.
    pub leftover_work_files: Vec<PathBuf>,
    /// Manifests that do not parse or do not hash to their id.
    pub damaged_manifests: Vec<PathBuf>,
    /// Stored content no snapshot refers to.
    pub unreferenced_objects: usize,
}

pub struct SnapshotStore {
    root: PathBuf,
    budget_bytes: u64,
}

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Test seam: stop after a named step, as a crash would.
#[cfg(test)]
thread_local! {
    pub(crate) static CRASH_AT: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
}

fn crash_point(_step: &'static str) -> Result<(), ContinuityError> {
    #[cfg(test)]
    if CRASH_AT.with(|at| *at.borrow() == Some(_step)) {
        return Err(ContinuityError::Io(format!("simulated crash at {_step}")));
    }
    Ok(())
}

fn safe_name(name: &str) -> Result<(), ContinuityError> {
    let path = Path::new(name);
    let plain = !name.is_empty()
        && !name.contains('\0')
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)));
    if plain {
        Ok(())
    } else {
        Err(ContinuityError::Refused(format!(
            "{name:?} is not a plain relative file name"
        )))
    }
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn create_private_dir(path: &Path) -> Result<(), ContinuityError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|error| io("cannot create", path, error))?;
    let meta = fs::symlink_metadata(path).map_err(|error| io("cannot check", path, error))?;
    if !meta.is_dir() {
        return Err(ContinuityError::Refused(format!(
            "{} is not a real folder",
            path.display()
        )));
    }
    Ok(())
}

fn sync_dir(path: &Path) -> Result<(), ContinuityError> {
    File::open(path)
        .and_then(|dir| dir.sync_all())
        .map_err(|error| io("cannot flush", path, error))
}

/// Free bytes on the filesystem holding `path`.
pub(crate) fn free_bytes(path: &Path) -> Result<u64, ContinuityError> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| ContinuityError::Refused("path contains a NUL byte".into()))?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid C string and `stat` a valid out-pointer.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return Err(io(
            "cannot measure free space on",
            path,
            std::io::Error::last_os_error(),
        ));
    }
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

/// Opens a regular file without following a link at its last component.
fn open_no_follow(path: &Path) -> Result<(File, fs::Metadata), ContinuityError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                ContinuityError::Refused(format!(
                    "{} is a link, not a file; EmuWiz reads saves only where they really are",
                    path.display()
                ))
            } else if error.kind() == std::io::ErrorKind::NotFound {
                ContinuityError::NotFound(path.display().to_string())
            } else {
                io("cannot open", path, error)
            }
        })?;
    let meta = file
        .metadata()
        .map_err(|error| io("cannot check", path, error))?;
    if !meta.is_file() {
        return Err(ContinuityError::Refused(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    Ok((file, meta))
}

struct StableRead {
    bytes: Vec<u8>,
    sha256: String,
    mtime_ns: i64,
    hard_links: u64,
}

fn mtime_ns(meta: &fs::Metadata) -> i64 {
    meta.mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(meta.mtime_nsec())
}

/// Reads a file twice through separate opens. It is accepted only if both
/// reads, and the file's size, time and identity around them, agree. That
/// rules out a copy torn by a write that happened *during* the read; it does
/// not prove the emulator had finished writing (see [`CaptureBoundary`]).
fn stable_read(path: &Path) -> Result<StableRead, ContinuityError> {
    let read_once = || -> Result<(Vec<u8>, fs::Metadata, fs::Metadata), ContinuityError> {
        let (mut file, before) = open_no_follow(path)?;
        if before.len() > MAX_FILE_BYTES {
            return Err(ContinuityError::Refused(format!(
                "{} is {} bytes, more than the {} byte limit for one save file",
                path.display(),
                before.len(),
                MAX_FILE_BYTES
            )));
        }
        let mut bytes = Vec::with_capacity(before.len() as usize);
        (&mut file)
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io("cannot read", path, error))?;
        let after = file
            .metadata()
            .map_err(|error| io("cannot check", path, error))?;
        Ok((bytes, before, after))
    };
    let (first, before, after) = read_once()?;
    let (second, again_before, again_after) = read_once()?;
    let same_file = |a: &fs::Metadata, b: &fs::Metadata| {
        a.dev() == b.dev() && a.ino() == b.ino() && a.len() == b.len() && mtime_ns(a) == mtime_ns(b)
    };
    if first != second
        || first.len() as u64 != before.len()
        || !same_file(&before, &after)
        || !same_file(&before, &again_before)
        || !same_file(&before, &again_after)
    {
        return Err(ContinuityError::Unstable(format!(
            "{} was being written; no snapshot was kept",
            path.display()
        )));
    }
    Ok(StableRead {
        sha256: sha256_hex(&first),
        bytes: first,
        mtime_ns: mtime_ns(&before),
        hard_links: before.nlink(),
    })
}

impl SnapshotStore {
    /// Opens (creating if needed) a store at `root`, private to the user.
    pub fn open(root: &Path) -> Result<Self, ContinuityError> {
        Self::open_with_budget(root, DEFAULT_BUDGET_BYTES)
    }

    pub fn open_with_budget(root: &Path, budget_bytes: u64) -> Result<Self, ContinuityError> {
        if !root.is_absolute() {
            return Err(ContinuityError::Refused(
                "the store folder must be an absolute path".into(),
            ));
        }
        create_private_dir(root)?;
        for part in ["objects", "manifests", "tmp"] {
            create_private_dir(&root.join(part))?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            budget_bytes,
        })
    }

    /// Opens a store that already exists, creating nothing. For looking
    /// (timeline, verify, proposals, dry runs) and for another device's
    /// folder: a mistyped path is an error, not a new empty store.
    pub fn open_existing(root: &Path) -> Result<Self, ContinuityError> {
        if !root.is_absolute() {
            return Err(ContinuityError::Refused(
                "the store folder must be an absolute path".into(),
            ));
        }
        for part in ["", "objects", "manifests"] {
            let folder = root.join(part);
            if !fs::symlink_metadata(&folder).is_ok_and(|meta| meta.is_dir()) {
                return Err(ContinuityError::NotFound(format!(
                    "no snapshot folder at {} (nothing was created)",
                    root.display()
                )));
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
            budget_bytes: DEFAULT_BUDGET_BYTES,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, sha256: &str) -> PathBuf {
        self.root.join("objects").join(&sha256[..2]).join(sha256)
    }

    fn work_file(&self) -> PathBuf {
        let n = WORK_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        self.root
            .join("tmp")
            .join(format!("{}-{n}-{nanos}.part", std::process::id()))
    }

    /// Writes `bytes` to a new file at `final_path` without ever replacing
    /// one: work file, flush, link into place, flush the folder.
    /// `Ok(false)` means a file was already there (left untouched).
    fn publish(&self, final_path: &Path, bytes: &[u8], mode: u32) -> Result<bool, ContinuityError> {
        let work = self.work_file();
        // Created here or not at all: if this fails, nothing at `work` is
        // ours and nothing there is removed.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&work)
            .map_err(|error| io("cannot create", &work, error))?;
        let result = (|| {
            file.write_all(bytes)
                .map_err(|error| io("cannot write", &work, error))?;
            file.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(|error| io("cannot protect", &work, error))?;
            file.sync_all()
                .map_err(|error| io("cannot flush", &work, error))?;
            crash_point("before-link")?;
            let parent = final_path.parent().unwrap_or(&self.root);
            let parent_is_new = fs::symlink_metadata(parent).is_err();
            create_private_dir(parent)?;
            if parent_is_new {
                // A folder made just now must itself be durable before
                // anything is published inside it.
                if let Some(above) = parent.parent() {
                    sync_dir(above)?;
                }
            }
            match fs::hard_link(&work, final_path) {
                Ok(()) => {
                    sync_dir(parent)?;
                    Ok(true)
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                Err(error) => Err(io("cannot publish", final_path, error)),
            }
        })();
        if !matches!(&result, Err(ContinuityError::Io(text)) if text.starts_with("simulated crash"))
        {
            let _ = fs::remove_file(&work);
        }
        result
    }

    /// Stores `bytes` under their SHA-256. Returns true when new.
    pub(crate) fn put_object(&self, bytes: &[u8], sha256: &str) -> Result<bool, ContinuityError> {
        if sha256_hex(bytes) != sha256 {
            return Err(ContinuityError::Corrupt(
                "content does not match the checksum it came with".into(),
            ));
        }
        let path = self.object_path(sha256);
        if self.publish(&path, bytes, 0o400)? {
            return Ok(true);
        }
        // Already stored: it must really be the same content.
        match self.read_object(sha256) {
            Ok(_) => Ok(false),
            Err(error) => Err(ContinuityError::Corrupt(format!(
                "the stored copy of {sha256} is damaged ({error}); it was not replaced"
            ))),
        }
    }

    /// Reads one stored object, verified against its name.
    pub fn read_object(&self, sha256: &str) -> Result<Vec<u8>, ContinuityError> {
        if !is_sha256(sha256) {
            return Err(ContinuityError::Refused(format!(
                "{sha256:?} is not a SHA-256"
            )));
        }
        let path = self.object_path(sha256);
        let (mut file, meta) = open_no_follow(&path)?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(ContinuityError::Corrupt(format!(
                "{} is too large",
                path.display()
            )));
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.read_to_end(&mut bytes)
            .map_err(|error| io("cannot read", &path, error))?;
        let actual = sha256_hex(&bytes);
        if actual != sha256 {
            return Err(ContinuityError::Corrupt(format!(
                "{} holds {actual}",
                path.display()
            )));
        }
        Ok(bytes)
    }

    pub fn has_object(&self, sha256: &str) -> bool {
        is_sha256(sha256)
            && fs::symlink_metadata(self.object_path(sha256)).is_ok_and(|m| m.is_file())
    }

    fn manifest_path(&self, manifest: &SnapshotManifest) -> PathBuf {
        self.root
            .join("manifests")
            .join(&manifest.body.lineage)
            .join(format!(
                "{:012}-{}.json",
                manifest.body.captured_unix.max(0),
                &manifest.snapshot_id[..16]
            ))
    }

    /// Publishes a manifest whose objects are already stored.
    pub(crate) fn put_manifest(
        &self,
        manifest: &SnapshotManifest,
    ) -> Result<bool, ContinuityError> {
        check_manifest(manifest)?;
        // Before a record becomes visible, every file it names must be
        // stored, intact, and exactly the size the record says: a record
        // is never published and checked afterwards.
        for file in &manifest.body.files {
            let stored = self.read_object(&file.sha256).map_err(|error| {
                ContinuityError::Refused(format!(
                    "snapshot {} names content that is not stored intact ({}: {error})",
                    &manifest.snapshot_id[..12],
                    file.name
                ))
            })?;
            if stored.len() as u64 != file.size_bytes {
                return Err(ContinuityError::Refused(format!(
                    "snapshot {} says {} is {} bytes; the stored content is {}",
                    &manifest.snapshot_id[..12],
                    file.name,
                    file.size_bytes,
                    stored.len()
                )));
            }
        }
        let text = serde_json::to_vec_pretty(manifest)
            .map_err(|error| ContinuityError::Io(format!("cannot write manifest: {error}")))?;
        crash_point("before-manifest")?;
        let path = self.manifest_path(manifest);
        if self.publish(&path, &text, 0o400)? {
            return Ok(true);
        }
        // A record with this name is already there. It must be this record.
        match Self::read_manifest_file(&path) {
            Ok(existing) if existing == *manifest => Ok(false),
            _ => Err(ContinuityError::Corrupt(format!(
                "{} already exists and is not this snapshot's record; it was not replaced",
                path.display()
            ))),
        }
    }

    fn read_manifest_file(path: &Path) -> Result<SnapshotManifest, ContinuityError> {
        let (mut file, meta) = open_no_follow(path)?;
        if meta.len() > 16 * 1024 * 1024 {
            return Err(ContinuityError::Corrupt(format!(
                "{} is too large",
                path.display()
            )));
        }
        let mut text = Vec::new();
        file.read_to_end(&mut text)
            .map_err(|error| io("cannot read", path, error))?;
        let manifest: SnapshotManifest = serde_json::from_slice(&text)
            .map_err(|error| ContinuityError::Corrupt(format!("{}: {error}", path.display())))?;
        check_manifest(&manifest)?;
        Ok(manifest)
    }

    fn manifest_files(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut good = Vec::new();
        let mut odd = Vec::new();
        let lineages = sorted_dir(&self.root.join("manifests"));
        for lineage in lineages {
            for path in sorted_dir(&lineage) {
                if path.extension().is_some_and(|ext| ext == "json") {
                    good.push(path);
                } else {
                    odd.push(path);
                }
            }
        }
        (good, odd)
    }

    /// Every readable snapshot, oldest first within each save location.
    /// Damaged manifests are skipped here and listed by [`Self::health`].
    pub fn list(&self) -> Vec<SnapshotManifest> {
        self.manifest_files()
            .0
            .iter()
            .filter_map(|path| Self::read_manifest_file(path).ok())
            .collect()
    }

    /// Snapshots of one save location, oldest first. Reads one small folder.
    pub fn list_lineage(&self, lineage: &str) -> Vec<SnapshotManifest> {
        if safe_name(lineage).is_err() || lineage.contains('/') {
            return Vec::new();
        }
        let mut found: Vec<SnapshotManifest> =
            sorted_dir(&self.root.join("manifests").join(lineage))
                .iter()
                .filter_map(|path| Self::read_manifest_file(path).ok())
                .collect();
        found.sort_by(|a, b| {
            (a.body.captured_unix, &a.snapshot_id).cmp(&(b.body.captured_unix, &b.snapshot_id))
        });
        found
    }

    /// Finds a snapshot by id or unambiguous id prefix (8+ characters).
    pub fn get(&self, id: &str) -> Result<SnapshotManifest, ContinuityError> {
        if id.len() < 8 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ContinuityError::Refused(
                "a snapshot is named by at least 8 characters of its id".into(),
            ));
        }
        let mut matches: Vec<SnapshotManifest> = self
            .list()
            .into_iter()
            .filter(|manifest| manifest.snapshot_id.starts_with(id))
            .collect();
        match matches.len() {
            0 => Err(ContinuityError::NotFound(format!("snapshot {id}"))),
            1 => Ok(matches.remove(0)),
            _ => Err(ContinuityError::Refused(format!(
                "{id} matches more than one snapshot"
            ))),
        }
    }

    pub fn usage(&self) -> StoreUsage {
        let snapshots = self.list();
        let mut objects: BTreeMap<String, u64> = BTreeMap::new();
        let mut logical = 0u64;
        for manifest in &snapshots {
            for file in &manifest.body.files {
                logical += file.size_bytes;
                objects.insert(file.sha256.clone(), file.size_bytes);
            }
        }
        StoreUsage {
            snapshots: snapshots.len(),
            objects: objects.len(),
            stored_bytes: objects.values().sum(),
            logical_bytes: logical,
            budget_bytes: self.budget_bytes,
        }
    }

    /// Takes a snapshot. The originals are opened read-only and never
    /// changed. See the module documentation for the publication order.
    pub fn capture(
        &self,
        request: &CaptureRequest,
        observer: &dyn InUseObserver,
    ) -> Result<CaptureOutcome, ContinuityError> {
        if request.files.is_empty() || request.files.len() > MAX_FILES_PER_SNAPSHOT {
            return Err(ContinuityError::Refused(format!(
                "a snapshot holds between 1 and {MAX_FILES_PER_SNAPSHOT} files"
            )));
        }
        let mut names = BTreeSet::new();
        for file in &request.files {
            safe_name(&file.name)?;
            if !names.insert(file.name.as_str()) {
                return Err(ContinuityError::Refused(format!(
                    "{} is listed twice",
                    file.name
                )));
            }
            if Path::new(&file.name)
                .components()
                .next()
                .is_some_and(|first| first.as_os_str() == RECOVERY_RECEIPT_NAME)
            {
                return Err(ContinuityError::Refused(format!(
                    "{RECOVERY_RECEIPT_NAME} is the record EmuWiz writes into a recovered folder; \
                     a snapshot cannot hold a file of that name"
                )));
            }
            if file.source.starts_with(&self.root) {
                return Err(ContinuityError::Refused(
                    "the snapshot store cannot take a snapshot of itself".into(),
                ));
            }
        }
        let emulator = request.provenance.emulator.as_str();
        // Guards are taken before the first byte is read and held until the
        // last one has been: with kernel support, a program that opens a
        // save for writing in between is noticed even if it cannot be seen.
        let guards: Vec<Box<dyn CopyGuard + '_>> = request
            .files
            .iter()
            .map(|file| observer.guard(emulator, &file.source))
            .collect();
        let look = |when: &str| -> Result<InUse, ContinuityError> {
            let mut worst = InUse::NotDetected;
            for (file, guard) in request.files.iter().zip(&guards) {
                match guard.state() {
                    InUse::EmulatorRunning => {
                        return Err(ContinuityError::InUse(format!(
                            "{emulator} is running ({when}). Close it first: its saves may be half-written while it runs"
                        )));
                    }
                    InUse::OpenByAProgram => {
                        return Err(ContinuityError::InUse(format!(
                            "a program has {} open ({when})",
                            file.source.display()
                        )));
                    }
                    InUse::Unknown => worst = InUse::Unknown,
                    InUse::NotDetected => {}
                }
            }
            Ok(worst)
        };
        let before = look("before the copy")?;
        if before == InUse::Unknown && !request.allow_unproven_closure {
            return Err(ContinuityError::ClosureUnproven(format!(
                "EmuWiz could not establish that {emulator} is closed and nothing has this save open, so it cannot promise the copy is complete. \
                 A best-effort copy is possible only when asked for explicitly, and is recorded as unverified"
            )));
        }

        let mut reads = Vec::with_capacity(request.files.len());
        let mut total = 0u64;
        for file in &request.files {
            let read = stable_read(&file.source)?;
            if request
                .expected_sha256
                .get(&file.name)
                .is_some_and(|expected| *expected != read.sha256)
            {
                return Err(ContinuityError::Unstable(format!(
                    "{} changed between being listed and being copied; list the saves again",
                    file.source.display()
                )));
            }
            total += read.bytes.len() as u64;
            reads.push((file, read));
        }
        if let Some(expected) = &request.expected_folder_sha256 {
            let mut items: Vec<(&str, u64, &str)> = reads
                .iter()
                .map(|(file, read)| {
                    (
                        file.name.as_str(),
                        read.bytes.len() as u64,
                        read.sha256.as_str(),
                    )
                })
                .collect();
            items.sort();
            if super::folder_fingerprint(items.into_iter()) != *expected {
                return Err(ContinuityError::Unstable(format!(
                    "{} changed between being listed and being copied; list the saves again",
                    request.provenance.source_root.display()
                )));
            }
        }
        // Still closed, and nothing opened a save, once every byte is read.
        let after = look("during the copy")?;
        // The same rule at the end as at the start: without an explicit
        // best-effort request, "could not tell" keeps nothing.
        if after == InUse::Unknown && !request.allow_unproven_closure {
            return Err(ContinuityError::ClosureUnproven(format!(
                "by the end of the copy EmuWiz could no longer establish that {emulator} was closed and nothing had this save open; no snapshot was kept"
            )));
        }
        let kernel_watched = guards.iter().all(|guard| guard.kernel_watched());
        drop(guards);

        let boundary = if before == InUse::NotDetected && after == InUse::NotDetected {
            CaptureBoundary::EmulatorClosedObserved
        } else {
            CaptureBoundary::StableReadOnly
        };
        let lineage = lineage_key(
            &request.provenance.device_id,
            &request.provenance.emulator,
            &request.provenance.source_root,
        );
        let history = self.list_lineage(&lineage);
        let files: Vec<SnapshotFile> = reads
            .iter()
            .map(|(file, read)| SnapshotFile {
                name: file.name.clone(),
                size_bytes: read.bytes.len() as u64,
                sha256: read.sha256.clone(),
                source_mtime_ns: read.mtime_ns,
                source_hard_links: read.hard_links,
            })
            .collect();
        if let Some(newest) = history.last() {
            let same = |a: &[SnapshotFile], b: &[SnapshotFile]| {
                a.len() == b.len()
                    && a.iter()
                        .zip(b)
                        .all(|(x, y)| x.name == y.name && x.sha256 == y.sha256)
            };
            // An unverified copy never stands in for, or is hidden behind,
            // a verified one: only the same kind counts as "unchanged".
            // Nor does a snapshot taken after a crash hide behind an earlier
            // one taken after a clean exit (or the reverse): the bytes may be
            // the same, what is known about them is not.
            if same(&newest.body.files, &files)
                && newest.body.boundary == boundary
                && newest.body.preceding_session.as_ref() == Some(&request.preceding_session)
                && self.verify(&newest.snapshot_id)?.intact()
            {
                return Ok(CaptureOutcome::Unchanged {
                    existing: newest.clone(),
                });
            }
        }

        let mut needed: BTreeMap<&str, u64> = BTreeMap::new();
        for (_, read) in &reads {
            if !self.has_object(&read.sha256) {
                needed.insert(read.sha256.as_str(), read.bytes.len() as u64);
            }
        }
        self.require_space(needed.values().sum())?;

        let mut checks = vec![
            "each file was read twice and both reads were identical".to_string(),
            "each file's size, time and identity were the same before and after reading"
                .to_string(),
        ];
        match boundary {
            CaptureBoundary::EmulatorClosedObserved => {
                checks.push(format!(
                    "no {emulator} process (native, Flatpak, AppImage or launcher) was found before or after the copy"
                ));
                checks.push(if kernel_watched {
                    "the kernel confirmed no program had a save file open at the start, and none opened one for writing during the copy".to_string()
                } else {
                    "every running process could be inspected and none had a save file open".to_string()
                });
            }
            _ => checks.push(format!(
                "UNVERIFIED: it could not be established that {emulator} was closed; this copy may have been taken mid-write"
            )),
        }
        if files.iter().any(|file| file.source_hard_links > 1) {
            checks.push("note: a file has more than one name on disk (hard link); another folder shares this save".into());
        }
        let mut stored_new = 0u64;
        for (_, read) in &reads {
            if self.put_object(&read.bytes, &read.sha256)? {
                stored_new += read.bytes.len() as u64;
            }
            crash_point("after-object")?;
        }
        checks.push("every stored file was read back and matched its SHA-256".to_string());
        let body = SnapshotBody {
            lineage,
            parent: history.last().map(|parent| parent.snapshot_id.clone()),
            captured_unix: request.captured_unix.unwrap_or_else(now_unix),
            provenance: request.provenance.clone(),
            boundary,
            reason: request.reason,
            session_id: request.session_id.clone(),
            files,
            contained_saves: request.contained_saves.clone(),
            checks,
            structure: Some(request.structure.clone()),
            game_integrity: Some(GameIntegrity::not_checked()),
            preceding_session: Some(request.preceding_session.clone()),
        };
        let manifest = SnapshotManifest {
            format_version: MANIFEST_FORMAT_VERSION,
            snapshot_id: body_id(&body),
            body,
        };
        // Read back before the manifest makes the snapshot visible.
        for file in &manifest.body.files {
            self.read_object(&file.sha256)?;
        }
        self.put_manifest(&manifest)?;
        Ok(CaptureOutcome::Created {
            manifest,
            new_bytes: stored_new,
            reused_bytes: total - stored_new,
        })
    }

    pub(crate) fn require_space(&self, new_bytes: u64) -> Result<(), ContinuityError> {
        let free = free_bytes(&self.root)?;
        if free < new_bytes.saturating_add(FREE_SPACE_RESERVE_BYTES) {
            return Err(ContinuityError::NoSpace(format!(
                "{free} bytes free where the snapshots are kept; this needs {new_bytes} and EmuWiz keeps {FREE_SPACE_RESERVE_BYTES} spare"
            )));
        }
        let stored = self.usage().stored_bytes;
        if stored.saturating_add(new_bytes) > self.budget_bytes {
            return Err(ContinuityError::NoSpace(format!(
                "snapshots already use {stored} of the {} bytes allowed. Nothing is removed automatically: \
                 review the pruning proposal, or raise the limit",
                self.budget_bytes
            )));
        }
        Ok(())
    }

    /// Re-reads every byte of a snapshot and compares it with the manifest.
    pub fn verify(&self, id: &str) -> Result<VerifyReport, ContinuityError> {
        let manifest = self.get(id)?;
        Ok(self.verify_manifest(&manifest))
    }

    pub fn verify_manifest(&self, manifest: &SnapshotManifest) -> VerifyReport {
        let mut bytes_checked = 0;
        let files = manifest
            .body
            .files
            .iter()
            .map(|file| {
                let check = match self.read_object(&file.sha256) {
                    Ok(bytes) if bytes.len() as u64 == file.size_bytes => {
                        bytes_checked += file.size_bytes;
                        FileCheck::Intact
                    }
                    Ok(bytes) => FileCheck::Corrupt {
                        actual_sha256: sha256_hex(&bytes),
                    },
                    Err(ContinuityError::NotFound(_)) => FileCheck::Missing,
                    Err(ContinuityError::Corrupt(_)) => {
                        // Read the damaged copy the same guarded way: no
                        // link followed, size capped.
                        let actual = open_no_follow(&self.object_path(&file.sha256))
                            .ok()
                            .filter(|(_, meta)| meta.len() <= MAX_FILE_BYTES)
                            .and_then(|(mut object, _)| {
                                let mut bytes = Vec::new();
                                object
                                    .read_to_end(&mut bytes)
                                    .ok()
                                    .map(|_| sha256_hex(&bytes))
                            })
                            .unwrap_or_default();
                        FileCheck::Corrupt {
                            actual_sha256: actual,
                        }
                    }
                    Err(_) => FileCheck::Missing,
                };
                (file.name.clone(), check)
            })
            .collect();
        VerifyReport {
            snapshot_id: manifest.snapshot_id.clone(),
            manifest_intact: body_id(&manifest.body) == manifest.snapshot_id,
            files,
            bytes_checked,
        }
    }

    /// What recovering a snapshot into `destination` would do. Reads only.
    pub fn plan_recovery(
        &self,
        id: &str,
        destination: &Path,
    ) -> Result<RecoveryPlan, ContinuityError> {
        let manifest = self.get(id)?;
        let mut blockers = Vec::new();
        if !destination.is_absolute() {
            blockers.push("the destination must be an absolute path".to_string());
        }
        if fs::symlink_metadata(destination).is_ok() {
            blockers.push(format!(
                "{} already exists. A recovery always goes into a new folder, so nothing can be overwritten",
                destination.display()
            ));
        }
        // Compare real locations, not spellings: `/a/../store/x` is inside
        // the store. The destination does not exist yet, so its parent is
        // resolved and the last name put back.
        let resolved = destination
            .parent()
            .and_then(|parent| fs::canonicalize(parent).ok())
            .zip(destination.file_name())
            .map(|(parent, name)| parent.join(name));
        let real = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let located = resolved.as_deref().unwrap_or(destination);
        if located.starts_with(real(&self.root)) || destination.starts_with(&self.root) {
            blockers.push("the destination is inside the snapshot store".to_string());
        }
        let source_root = &manifest.body.provenance.source_root;
        let real_source = real(source_root);
        if destination == source_root.as_path()
            || located == real_source
            || (located.starts_with(&real_source) || destination.starts_with(source_root))
                && source_root.is_dir()
        {
            blockers.push(format!(
                "the destination is the emulator's own save location ({}). EmuWiz recovers beside it, never into it",
                source_root.display()
            ));
        }
        let parent = destination.parent().filter(|p| !p.as_os_str().is_empty());
        match parent.map(fs::symlink_metadata) {
            Some(Ok(meta)) if meta.is_dir() => {}
            _ => blockers
                .push("the folder that will hold the destination does not exist".to_string()),
        }
        let report = self.verify_manifest(&manifest);
        if !report.intact() {
            blockers.push(
                "the snapshot does not pass verification; it will not be recovered from"
                    .to_string(),
            );
        }
        let total = manifest.total_bytes();
        if let Some(Ok(free)) = parent.map(free_bytes) {
            if free < total.saturating_add(FREE_SPACE_RESERVE_BYTES) {
                blockers.push(format!(
                    "not enough free space at the destination ({free} bytes free, {total} needed)"
                ));
            }
        }
        Ok(RecoveryPlan {
            snapshot_id: manifest.snapshot_id.clone(),
            destination: destination.to_path_buf(),
            files: manifest.body.files.clone(),
            total_bytes: total,
            blockers,
            notes: vec![
                "The original save is not touched.".to_string(),
                "To use the recovered save, close the emulator and copy it into place yourself; EmuWiz does not replace a live save.".to_string(),
                manifest.body.boundary.label().to_string(),
                super::status::describe_structure(manifest.body.structure.as_ref()),
                super::status::describe_integrity(manifest.body.game_integrity.as_ref()),
                super::status::describe_preceding(manifest.body.preceding_session.as_ref()),
                "Recovering a snapshot does not make it a good save: only the game can show that.".to_string(),
            ],
        })
    }

    /// Recovers a snapshot into a folder that does not exist yet. The folder
    /// appears complete or not at all.
    pub fn recover_to_new(
        &self,
        id: &str,
        destination: &Path,
    ) -> Result<RecoveryReceipt, ContinuityError> {
        let plan = self.plan_recovery(id, destination)?;
        if let Some(blocker) = plan.blockers.first() {
            return Err(ContinuityError::Refused(blocker.clone()));
        }
        let parent = destination.parent().unwrap_or(Path::new("/"));
        let name = destination
            .file_name()
            .ok_or_else(|| ContinuityError::Refused("the destination needs a name".into()))?;
        let staging = parent.join(format!(
            ".{}.recovering-{}-{}",
            name.to_string_lossy(),
            std::process::id(),
            WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&staging)
            .map_err(|error| io("cannot create", &staging, error))?;
        let receipt = RecoveryReceipt {
            snapshot_id: plan.snapshot_id.clone(),
            destination: destination.to_path_buf(),
            recovered_unix: now_unix(),
            files: plan.files.clone(),
            checks: vec![
                "the snapshot passed verification immediately before recovery".to_string(),
                "every recovered file was read back and matched its SHA-256".to_string(),
                "the destination did not exist before; nothing was overwritten".to_string(),
            ],
        };
        let result = (|| {
            let mut folders_made: BTreeSet<PathBuf> = BTreeSet::new();
            for file in &plan.files {
                let bytes = self.read_object(&file.sha256)?;
                let path = staging.join(&file.name);
                if let Some(dir) = path.parent() {
                    create_private_dir(dir)?;
                    // Every folder between the staging folder and the file.
                    let mut folder = dir.to_path_buf();
                    while folder != staging && folder.starts_with(&staging) {
                        folders_made.insert(folder.clone());
                        if !folder.pop() {
                            break;
                        }
                    }
                }
                let mut out = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)
                    .map_err(|error| io("cannot create", &path, error))?;
                out.write_all(&bytes)
                    .map_err(|error| io("cannot write", &path, error))?;
                out.sync_all()
                    .map_err(|error| io("cannot flush", &path, error))?;
                crash_point("recover-after-file")?;
                let back = fs::read(&path).map_err(|error| io("cannot read back", &path, error))?;
                if sha256_hex(&back) != file.sha256 {
                    return Err(ContinuityError::Corrupt(format!(
                        "{} did not read back as written",
                        path.display()
                    )));
                }
            }
            let text = serde_json::to_vec_pretty(&receipt)
                .map_err(|error| ContinuityError::Io(error.to_string()))?;
            // The record is a new file like any other: it can replace nothing
            // (no snapshot may hold a file of this name) and is flushed.
            let receipt_path = staging.join(RECOVERY_RECEIPT_NAME);
            let mut record = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&receipt_path)
                .map_err(|error| io("cannot create", &receipt_path, error))?;
            record
                .write_all(&text)
                .and_then(|()| record.sync_all())
                .map_err(|error| io("cannot write", &receipt_path, error))?;
            // Deepest folders first, then the staging folder itself.
            for folder in folders_made.iter().rev() {
                sync_dir(folder)?;
            }
            sync_dir(&staging)?;
            rename_no_replace(&staging, destination)?;
            // The folder is in place and complete. If flushing its parent
            // fails now, the recovery still happened; saying otherwise would
            // send a person looking for a failure that is not there.
            let _ = sync_dir(parent);
            Ok(())
        })();
        match result {
            Ok(()) => Ok(receipt),
            Err(error) => {
                // Only the staging folder this call created is removed.
                if !matches!(&error, ContinuityError::Io(text) if text.starts_with("simulated crash"))
                {
                    let _ = fs::remove_dir_all(&staging);
                }
                Err(error)
            }
        }
    }

    /// A list of snapshots a person could remove, oldest first, keeping the
    /// newest `keep_newest_per_save` of every save location and never
    /// proposing the only verified copy. Nothing is deleted.
    pub fn pruning_proposal(&self, keep_newest_per_save: usize) -> PruningProposal {
        let keep = keep_newest_per_save.max(1);
        let all = self.list();
        let mut by_lineage: BTreeMap<&str, Vec<&SnapshotManifest>> = BTreeMap::new();
        for manifest in &all {
            by_lineage
                .entry(manifest.body.lineage.as_str())
                .or_default()
                .push(manifest);
        }
        let mut kept: Vec<&SnapshotManifest> = Vec::new();
        let mut removable: Vec<&SnapshotManifest> = Vec::new();
        for snapshots in by_lineage.values_mut() {
            snapshots.sort_by_key(|manifest| {
                (manifest.body.captured_unix, manifest.snapshot_id.clone())
            });
            let cut = snapshots.len().saturating_sub(keep);
            // Never proposed, whatever the quota says and however many newer
            // snapshots there are. Three separate protections, because
            // snapshot integrity and session status are separate questions:
            //  1. the newest safe copy (integrity only: clean-exit evidence
            //     is usually not available, and its absence must not make a
            //     sound recovery point disposable);
            //  2. the newest safe copy that did not follow an observed
            //     abnormal termination (so a run of post-crash snapshots
            //     cannot push the last unsuspected one out);
            //  3. the newest safe copy after a session known to have ended
            //     normally.
            let protected: BTreeSet<usize> = [
                snapshots.iter().rposition(|m| m.body.safe_copy()),
                snapshots
                    .iter()
                    .rposition(|m| m.body.safe_copy() && !m.body.after_observed_abnormal_end()),
                snapshots
                    .iter()
                    .rposition(|m| m.body.clean_recovery_point()),
            ]
            .into_iter()
            .flatten()
            .collect();
            for (index, manifest) in snapshots.iter().enumerate() {
                if index >= cut || protected.contains(&index) {
                    kept.push(manifest);
                } else {
                    removable.push(manifest);
                }
            }
        }
        let kept_objects: BTreeSet<&str> = kept
            .iter()
            .flat_map(|manifest| manifest.body.files.iter().map(|file| file.sha256.as_str()))
            .collect();
        let mut freed_already: BTreeSet<&str> = BTreeSet::new();
        let mut candidates = Vec::new();
        for manifest in removable {
            let mut would_free = 0;
            for file in &manifest.body.files {
                if !kept_objects.contains(file.sha256.as_str())
                    && freed_already.insert(file.sha256.as_str())
                {
                    would_free += file.size_bytes;
                }
            }
            candidates.push(PruneCandidate {
                snapshot_id: manifest.snapshot_id.clone(),
                lineage: manifest.body.lineage.clone(),
                captured_unix: manifest.body.captured_unix,
                why: format!("older than the newest {keep} snapshots of the same save"),
                would_free_bytes: would_free,
            });
        }
        candidates
            .sort_by_key(|candidate| (candidate.captured_unix, candidate.snapshot_id.clone()));
        PruningProposal {
            usage: self.usage(),
            keep_newest_per_save: keep,
            would_free_bytes: candidates
                .iter()
                .map(|candidate| candidate.would_free_bytes)
                .sum(),
            candidates,
            nothing_was_deleted: true,
        }
    }

    pub fn health(&self) -> StoreHealth {
        let (manifest_paths, odd) = self.manifest_files();
        let mut damaged = odd;
        let mut referenced = BTreeSet::new();
        for path in manifest_paths {
            match Self::read_manifest_file(&path) {
                Ok(manifest) => {
                    referenced.extend(manifest.body.files.into_iter().map(|file| file.sha256))
                }
                Err(_) => damaged.push(path),
            }
        }
        let mut unreferenced = 0;
        for shard in sorted_dir(&self.root.join("objects")) {
            for object in sorted_dir(&shard) {
                let name = object
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !referenced.contains(&name) {
                    unreferenced += 1;
                }
            }
        }
        StoreHealth {
            leftover_work_files: sorted_dir(&self.root.join("tmp")),
            damaged_manifests: damaged,
            unreferenced_objects: unreferenced,
        }
    }
}

pub(crate) fn check_manifest(manifest: &SnapshotManifest) -> Result<(), ContinuityError> {
    if manifest.format_version != MANIFEST_FORMAT_VERSION {
        return Err(ContinuityError::Refused(format!(
            "snapshot record version {} is not one this EmuWiz reads",
            manifest.format_version
        )));
    }
    if !is_sha256(&manifest.snapshot_id) || body_id(&manifest.body) != manifest.snapshot_id {
        return Err(ContinuityError::Corrupt(
            "a snapshot record does not match its own id (it was altered or damaged)".into(),
        ));
    }
    // A record belongs to exactly one history: the one its own device,
    // emulator and source location determine. Without this a record from
    // another device could be filed into one of this device's histories.
    let provenance = &manifest.body.provenance;
    let expected = lineage_key(
        &provenance.device_id,
        &provenance.emulator,
        &provenance.source_root,
    );
    if manifest.body.lineage != expected {
        return Err(ContinuityError::Refused(
            "a snapshot record names a save history that is not its own".into(),
        ));
    }
    let mut names = BTreeSet::new();
    for file in &manifest.body.files {
        if !names.insert(file.name.as_str()) {
            return Err(ContinuityError::Refused(format!(
                "{} is listed twice in a snapshot record",
                file.name
            )));
        }
        // Neither the name itself nor a folder of that name: either would
        // collide with the record a recovery writes.
        if Path::new(&file.name)
            .components()
            .next()
            .is_some_and(|first| first.as_os_str() == RECOVERY_RECEIPT_NAME)
        {
            return Err(ContinuityError::Refused(format!(
                "a snapshot cannot hold a file called {RECOVERY_RECEIPT_NAME}"
            )));
        }
    }
    if manifest.body.files.len() > MAX_FILES_PER_SNAPSHOT {
        return Err(ContinuityError::Refused(
            "a snapshot record lists too many files".into(),
        ));
    }
    for file in &manifest.body.files {
        safe_name(&file.name)?;
        if !is_sha256(&file.sha256) || file.size_bytes > MAX_FILE_BYTES {
            return Err(ContinuityError::Refused(format!(
                "{} has a malformed record",
                file.name
            )));
        }
    }
    Ok(())
}

fn sorted_dir(path: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    entries
}

/// Renames `from` to `to` only if `to` does not exist (one atomic step).
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> Result<(), ContinuityError> {
    use std::os::unix::ffi::OsStrExt;
    let c = |path: &Path| {
        std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| ContinuityError::Refused("path contains a NUL byte".into()))
    };
    let (from_c, to_c) = (c(from)?, c(to)?);
    // SAFETY: both pointers are valid NUL-terminated strings for the call.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EEXIST) {
        Err(ContinuityError::Refused(format!(
            "{} appeared while the recovery was being prepared; nothing was overwritten",
            to.display()
        )))
    } else {
        Err(io("cannot publish", to, error))
    }
}
