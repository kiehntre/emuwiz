//! Durable, append-only history of Library View apply/removal operations.
//!
//! This module is purely additive to the existing apply/remove pipeline in
//! `library_views` - it never decides *what* happens to a view's managed
//! symlinks or its manifest, it only records what already happened, after
//! the fact. One JSON file is written per operation, only once the
//! corresponding manifest write has already succeeded, so a history
//! record never describes something that did not actually happen on disk.
//!
//! # Storage
//!
//! Records live under `{library_views_data_dir}/history/`, alongside (but
//! never inside) the per-view manifests that directory already holds - see
//! `library_view_history_dir`. Each record is its own file, named from a
//! zero-padded nanosecond timestamp plus a monotonic counter and the
//! writing process's id, so lexicographic filename order is chronological
//! order and a directory listing alone (no file contents read) is enough
//! to sort newest-first.
//!
//! # Write semantics
//!
//! Each record is written to a fresh temp file, `fsync`ed, then linked
//! into place under its final name and the temp file removed - never a
//! plain rename, which would silently replace an existing file with the
//! same name. A collision (which should never happen given the filename
//! scheme) fails the write instead of discarding a prior record. This is
//! an append-only store: nothing here ever edits or removes a record that
//! was already written.
//!
//! A history write failure is never allowed to make the caller believe an
//! apply/remove that already completed did not happen, or to roll it
//! back - see `library_views::LibraryViewApplyReport::history_warning` and
//! its call sites in `apply_library_view`/`remove_library_view_symlinks`.

use crate::library_views::{FrontendProfileKind, LibraryViewApplyOutcome, LibraryViewApplyReport};
use crate::{ArchiveFsError, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const LIBRARY_VIEW_HISTORY_SCHEMA_VERSION: u32 = 1;

/// Bounds how many per-entry diagnostics one record carries, so a
/// pathological operation (thousands of failed entries) cannot make a
/// single history record unbounded.
const MAX_RECORD_WARNINGS: usize = 20;
const MAX_WARNING_MESSAGE_CHARS: usize = 200;

/// Which operation a [`LibraryViewHistoryRecord`] documents.
///
/// `repair_library_view` is byte-for-byte `apply_library_view` (see that
/// function's own doc comment - re-running the full plan already fixes
/// drift, so Repair is not a narrower operation than Apply) and shares its
/// code path exactly, so a Repair run is recorded as `Apply`: there is no
/// separate call site for it to diverge from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LibraryViewHistoryOperation {
    Apply,
    Remove,
}

/// One durable, append-only record of a completed Library View apply or
/// remove operation. Every count here is read straight off the
/// [`LibraryViewApplyReport`]/plan the operation itself already produced -
/// nothing is inferred or invented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryViewHistoryRecord {
    pub schema_version: u32,
    /// RFC3339 UTC - the same format `library_views` already uses for
    /// manifest entry timestamps.
    pub timestamp: String,
    pub operation: LibraryViewHistoryOperation,
    pub view_id: String,
    pub view_name: String,
    pub profile_kind: FrontendProfileKind,
    pub destination_root: String,
    pub manifest_path: String,
    /// How many entries the operation considered: the plan's entry count
    /// for Apply/Repair, the manifest's entry count (before removal) for
    /// Remove.
    pub planned_count: usize,
    pub created: usize,
    pub repaired: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub failed: usize,
    /// `plan.counts.skip + plan.counts.collision` for Apply/Repair - Remove
    /// has no plan to draw this from, so `None` there.
    pub skipped_or_collision: Option<usize>,
    /// `true` exactly when the report recorded zero failed entries.
    pub success: bool,
    /// Short, capped diagnostics already produced by the operation itself
    /// (one per `Failed`/`LeftUnchanged` result entry) - never invented
    /// here. See `MAX_RECORD_WARNINGS`/`MAX_WARNING_MESSAGE_CHARS`.
    pub warnings: Vec<String>,
}

/// Everything about the surrounding view/operation that a
/// [`LibraryViewApplyReport`] does not itself carry (it is generic over
/// both Apply and Remove, and knows nothing about the view it came from).
pub(crate) struct LibraryViewHistoryContext<'a> {
    pub view_id: &'a str,
    pub view_name: &'a str,
    pub profile_kind: FrontendProfileKind,
    pub destination_root: &'a Path,
    pub manifest_path: &'a Path,
    pub planned_count: usize,
    pub skipped_or_collision: Option<usize>,
}

fn build_history_record(
    operation: LibraryViewHistoryOperation,
    context: LibraryViewHistoryContext<'_>,
    report: &LibraryViewApplyReport,
) -> LibraryViewHistoryRecord {
    let warnings: Vec<String> = report
        .results
        .iter()
        .filter(|entry| {
            matches!(
                entry.outcome,
                LibraryViewApplyOutcome::Failed | LibraryViewApplyOutcome::LeftUnchanged
            )
        })
        .filter_map(|entry| {
            let message = entry.error.as_ref()?;
            let truncated: String = message.chars().take(MAX_WARNING_MESSAGE_CHARS).collect();
            Some(format!(
                "{}: {truncated}",
                entry.relative_link_path.display()
            ))
        })
        .take(MAX_RECORD_WARNINGS)
        .collect();

    LibraryViewHistoryRecord {
        schema_version: LIBRARY_VIEW_HISTORY_SCHEMA_VERSION,
        timestamp: crate::format_unix_timestamp_utc(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or_default(),
        ),
        operation,
        view_id: context.view_id.to_string(),
        view_name: context.view_name.to_string(),
        profile_kind: context.profile_kind,
        destination_root: context.destination_root.display().to_string(),
        manifest_path: context.manifest_path.display().to_string(),
        planned_count: context.planned_count,
        created: report.created,
        repaired: report.repaired,
        removed: report.removed,
        unchanged: report.unchanged,
        failed: report.failed,
        skipped_or_collision: context.skipped_or_collision,
        success: report.failed == 0,
        warnings,
    }
}

/// The history directory for a given Library View data directory -
/// `{data_dir}/history`, a subdirectory of the same directory
/// `apply_library_view`/`remove_library_view_symlinks` already take (and,
/// by default, `default_library_views_data_dir()`), never a separate root.
/// A caller that points `data_dir` at an isolated location (as every
/// `library_views` test already does) gets an isolated history directory
/// for free, with no second path to keep in sync.
pub fn library_view_history_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("history")
}

/// The history directory under the default Library Views data directory.
pub fn default_library_view_history_dir() -> Result<PathBuf> {
    Ok(library_view_history_dir(
        &crate::default_library_views_data_dir()?,
    ))
}

/// Records `report` (already-completed) as one history entry under
/// `data_dir`'s history directory. Builds the record from `report` and
/// `context` and writes it; on failure, returns the diagnostic instead of
/// panicking or silently dropping it - the caller decides how to surface
/// that without ever implying the underlying apply/remove itself failed
/// (it already succeeded, or this would never have been called).
pub(crate) fn record_library_view_operation(
    data_dir: &Path,
    operation: LibraryViewHistoryOperation,
    context: LibraryViewHistoryContext<'_>,
    report: &LibraryViewApplyReport,
) -> Result<PathBuf> {
    let record = build_history_record(operation, context, report);
    write_record_atomically(&library_view_history_dir(data_dir), &record)
}

/// Writes `record` as a brand-new file in `dir` - never overwrites an
/// existing file, even one with the exact same generated name (which
/// should never happen given the filename scheme below, but a collision
/// fails loudly here rather than silently discarding a prior record).
fn write_record_atomically(dir: &Path, record: &LibraryViewHistoryRecord) -> Result<PathBuf> {
    fs::create_dir_all(dir).map_err(|source| ArchiveFsError::io(dir.to_path_buf(), source))?;

    let bytes = serde_json::to_vec_pretty(record).map_err(|error| {
        ArchiveFsError::Config(format!(
            "cannot serialize library view history record: {error}"
        ))
    })?;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let unix_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    // Zero-padded so lexicographic filename order is chronological order -
    // `list_library_view_history_at` relies on this to sort newest-first
    // from directory names alone.
    let file_name = format!("{unix_nanos:023}-{sequence:010}-{pid}.json");
    let final_path = dir.join(&file_name);
    let temp_path = dir.join(format!(".{file_name}.tmp"));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(&bytes)?;
        file.flush()?;
        file.sync_all()
    })();
    if let Err(source) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(ArchiveFsError::io(temp_path, source));
    }

    if let Err(source) = rename_no_replace(&temp_path, &final_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(ArchiveFsError::io(final_path, source));
    }
    sync_directory_best_effort(dir);
    Ok(final_path)
}

#[cfg(target_os = "linux")]
fn rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(destination).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "destination exists",
        ));
    }
    fs::rename(source, destination)
}

fn sync_directory_best_effort(dir: &Path) {
    if let Ok(handle) = fs::File::open(dir) {
        let _ = handle.sync_all();
    }
}

/// One entry from listing a history directory: either a record that
/// parsed cleanly, or a diagnostic naming the specific file that did not -
/// so one corrupted/unreadable history file can never hide the valid
/// records around it.
#[derive(Debug)]
pub enum LibraryViewHistoryEntry {
    Record {
        path: PathBuf,
        record: LibraryViewHistoryRecord,
    },
    Malformed {
        path: PathBuf,
        error: String,
    },
}

/// Lists up to `limit` history records under `dir`, newest first. A
/// missing directory (no operation has ever been recorded yet) yields an
/// empty list rather than an error, exactly like a missing manifest file
/// means "never applied yet" elsewhere in `library_views`.
///
/// Bounded: only the `limit` most recent files (by filename, which sorts
/// chronologically - see `write_record_atomically`) ever have their
/// contents read: a directory holding far more records than `limit` never
/// causes more than `limit` reads.
pub fn list_library_view_history_at(dir: &Path, limit: usize) -> Vec<LibraryViewHistoryEntry> {
    let mut paths: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect(),
        Err(_) => return Vec::new(),
    };
    paths.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    paths
        .into_iter()
        .take(limit)
        .map(|path| match read_history_record_file(&path) {
            Ok(record) => LibraryViewHistoryEntry::Record { path, record },
            Err(error) => LibraryViewHistoryEntry::Malformed { path, error },
        })
        .collect()
}

/// Lists up to `limit` history records under the default Library Views
/// history directory, newest first.
pub fn list_library_view_history_default(limit: usize) -> Result<Vec<LibraryViewHistoryEntry>> {
    Ok(list_library_view_history_at(
        &default_library_view_history_dir()?,
        limit,
    ))
}

/// Loads and parses a single history record file - the same tolerant
/// read `list_library_view_history_at` uses per-entry, exposed directly
/// so a caller holding a path from a previous listing can re-read (or
/// verify) just that one record.
pub fn load_library_view_history_record(
    path: &Path,
) -> std::result::Result<LibraryViewHistoryRecord, String> {
    read_history_record_file(path)
}

fn read_history_record_file(path: &Path) -> std::result::Result<LibraryViewHistoryRecord, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&contents).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_views::{LibraryViewApplyEntryResult, LibraryViewApplyOutcome};
    use std::path::PathBuf as StdPathBuf;

    fn temp_dir(name: &str) -> StdPathBuf {
        let dir = std::env::temp_dir().join(format!(
            "archivefs-core-library-view-history-test-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_report(created: usize, failed: usize) -> LibraryViewApplyReport {
        let mut results = Vec::new();
        if failed > 0 {
            results.push(LibraryViewApplyEntryResult {
                relative_link_path: PathBuf::from("NES/Broken.zip"),
                outcome: LibraryViewApplyOutcome::Failed,
                error: Some("could not create symlink".to_string()),
            });
        }
        LibraryViewApplyReport {
            view_id: "view-1".to_string(),
            created,
            repaired: 0,
            removed: 0,
            unchanged: 0,
            failed,
            results,
            history_warning: None,
        }
    }

    fn sample_context<'a>(
        destination_root: &'a Path,
        manifest_path: &'a Path,
    ) -> LibraryViewHistoryContext<'a> {
        LibraryViewHistoryContext {
            view_id: "view-1",
            view_name: "My View",
            profile_kind: FrontendProfileKind::Generic,
            destination_root,
            manifest_path,
            planned_count: 1,
            skipped_or_collision: Some(0),
        }
    }

    #[test]
    fn write_then_list_round_trips_the_record() {
        let dir = temp_dir("round-trip");
        let destination = dir.join("dest");
        let manifest_path = dir.join("view-1.manifest.json");
        let report = sample_report(1, 0);
        let record = build_history_record(
            LibraryViewHistoryOperation::Apply,
            sample_context(&destination, &manifest_path),
            &report,
        );

        let written_path = write_record_atomically(&dir, &record).unwrap();
        assert!(written_path.exists());

        let entries = list_library_view_history_at(&dir, 10);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            LibraryViewHistoryEntry::Record { record: loaded, .. } => {
                assert_eq!(loaded, &record);
                assert_eq!(loaded.view_id, "view-1");
                assert_eq!(loaded.created, 1);
                assert!(loaded.success);
            }
            LibraryViewHistoryEntry::Malformed { error, .. } => {
                panic!("expected a valid record, got malformed: {error}")
            }
        }
    }

    #[test]
    fn listing_a_missing_directory_is_empty_not_an_error() {
        let dir = temp_dir("missing-dir").join("does-not-exist");
        let entries = list_library_view_history_at(&dir, 10);
        assert!(entries.is_empty());
    }

    #[test]
    fn multiple_writes_append_and_list_newest_first() {
        let dir = temp_dir("append-order");
        let destination = dir.join("dest");
        let manifest_path = dir.join("view-1.manifest.json");

        for index in 0..3u32 {
            let report = sample_report(index as usize, 0);
            let record = build_history_record(
                LibraryViewHistoryOperation::Apply,
                sample_context(&destination, &manifest_path),
                &report,
            );
            write_record_atomically(&dir, &record).unwrap();
        }

        let entries = list_library_view_history_at(&dir, 10);
        assert_eq!(entries.len(), 3, "every write must append, never replace");
        let created_order: Vec<usize> = entries
            .iter()
            .map(|entry| match entry {
                LibraryViewHistoryEntry::Record { record, .. } => record.created,
                LibraryViewHistoryEntry::Malformed { .. } => panic!("unexpected malformed entry"),
            })
            .collect();
        assert_eq!(created_order, vec![2, 1, 0], "listing must be newest-first");
    }

    #[test]
    fn malformed_history_file_does_not_hide_valid_records() {
        let dir = temp_dir("malformed-tolerant");
        let destination = dir.join("dest");
        let manifest_path = dir.join("view-1.manifest.json");
        let report = sample_report(1, 0);
        let record = build_history_record(
            LibraryViewHistoryOperation::Apply,
            sample_context(&destination, &manifest_path),
            &report,
        );
        write_record_atomically(&dir, &record).unwrap();

        // An old/corrupted history file sitting alongside a valid one.
        fs::write(dir.join("0000-corrupt.json"), b"{ not valid json").unwrap();

        let entries = list_library_view_history_at(&dir, 10);
        assert_eq!(entries.len(), 2);
        let valid_count = entries
            .iter()
            .filter(|entry| matches!(entry, LibraryViewHistoryEntry::Record { .. }))
            .count();
        let malformed_count = entries
            .iter()
            .filter(|entry| matches!(entry, LibraryViewHistoryEntry::Malformed { .. }))
            .count();
        assert_eq!(valid_count, 1, "the valid record must still be readable");
        assert_eq!(
            malformed_count, 1,
            "the corrupt file must be reported, not silently dropped"
        );
    }

    #[test]
    fn write_never_overwrites_an_existing_file_at_the_final_path() {
        let dir = temp_dir("no-overwrite");
        let existing_path = dir.join("existing.json");
        fs::write(&existing_path, b"original-contents").unwrap();

        let source_path = dir.join("incoming.json");
        fs::write(&source_path, b"new-contents").unwrap();

        let result = rename_no_replace(&source_path, &existing_path);
        assert!(
            result.is_err(),
            "renaming onto an existing path must fail, never silently replace it"
        );
        assert_eq!(
            fs::read(&existing_path).unwrap(),
            b"original-contents",
            "the pre-existing file must be completely untouched"
        );
    }
}
