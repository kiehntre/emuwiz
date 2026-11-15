//! Moving a whole game folder (a PS3 disc folder, a ScummVM or arcade set
//! folder, or a folder the person picked) into its system's folder.
//!
//! One folder is one unit: it is renamed in a single no-overwrite
//! `renameat2(RENAME_NOREPLACE)` within the same filesystem, so every file
//! inside keeps its bytes, inode, times and relative path. There is no copy
//! fallback; a destination on another filesystem is refused before anything
//! changes.
//!
//! Order (each step journalled durably before the next):
//! 1. preview: boundary checks + the folder's layout (paths, sizes, times,
//!    inodes - no file contents are read);
//! 2. apply: layout re-checked, intent journalled, missing system folders
//!    created (recorded only after creation), library path changes staged
//!    in an open SQLite transaction, the rename, the layout re-checked at the
//!    destination, then the library commit. Any failure before the commit
//!    renames the folder back and rolls the library back;
//! 3. Undo: the intent journalled (`Undoing`), then the same steps in
//!    reverse, refusing if the folder changed since.
//!
//! Interrupted moves and Undos are found by [`interrupted`] and settled by
//! [`recover`], which decides from what is actually on disk and in the
//! library, never from the journal alone.
//!
//! Durability. A directory entry change (create, rename, remove) is assumed
//! durable only after an fsync of the directory holding it; a file or
//! database fsync is not assumed to persist earlier directory changes. So:
//! a created system folder's parent is synced before anything is renamed
//! into it; after a rename, both parents are synced before the library may
//! name the new place; the library commits on a `synchronous=EXTRA`
//! connection, and what recovery reads from it is made durable before it is
//! acted on; and an outcome (`Committed`, `RolledBack`, `Undone`) is
//! journalled only after everything it describes is durable. A failed sync
//! is never ignored: the record stays open and recovery settles it later.
//! Process death is covered by the tests; power-loss behaviour rests on the
//! assumption above and is not tested directly.

use crate::database::organisation_paths::{FolderCatalogueMove, FolderRow};
use crate::game_package::{FolderLayout, capture_folder_layout};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Journal folder name inside the organisation journal directory.
pub const JOURNAL_SUBDIR: &str = "folder-moves";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FolderMoveState {
    /// Intent recorded; nothing renamed yet (or renamed and not yet known).
    Planned,
    /// Moved and the library committed.
    Committed,
    /// Undo started (recorded before it changes anything); settled by
    /// [`recover`] if it was interrupted. Records written before this state
    /// existed never contain it.
    Undoing,
    /// Put back by Undo.
    Undone,
    /// Failed before completion and put back.
    RolledBack,
    /// Interrupted in a state recovery could not settle safely.
    NeedsReview,
}

/// A previewed folder move.
#[derive(Debug, Clone)]
pub struct FolderMovePlan {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub root: PathBuf,
    pub platform: String,
    /// Why this system: "your system choice", or the layout evidence.
    pub evidence: String,
    pub layout: FolderLayout,
    pub rows: Vec<FolderRow>,
    /// The destination's parent folder does not exist yet.
    pub creates_folder: bool,
}

/// What preview decided for a folder.
#[derive(Debug, Clone)]
pub enum FolderPreview {
    Move(Box<FolderMovePlan>),
    AlreadyInPlace,
    Refused(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderMoveRecord {
    pub version: u32,
    pub id: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub root: PathBuf,
    pub platform: String,
    pub database: PathBuf,
    pub layout_digest: String,
    pub entries: usize,
    pub total_bytes: u64,
    pub rows: Vec<FolderRow>,
    /// Folders this move created (removed again by Undo when empty).
    pub created_dirs: Vec<PathBuf>,
    pub state: FolderMoveState,
    pub at_unix: u64,
}

fn digest(layout: &FolderLayout) -> String {
    let text = serde_json::to_vec(&layout.entries).unwrap_or_default();
    Sha256::digest(&text)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Read-only preview of moving `source` (one game folder) into `platform`'s
/// folder in its library root.
pub fn preview(
    database: &crate::Database,
    roots: &[PathBuf],
    source: &Path,
    platform: &str,
    evidence: &str,
) -> FolderPreview {
    use std::os::unix::fs::MetadataExt;
    let refuse = |why: String| FolderPreview::Refused(why);
    if crate::platform::platform_by_id(platform).is_none() {
        return refuse(format!("{platform} is not a system EmuWiz knows"));
    }
    match std::fs::canonicalize(source) {
        Ok(real) if real == source => {}
        _ => {
            return refuse(format!(
                "{} is reached through a link or is unavailable; nothing was moved",
                source.display()
            ));
        }
    }
    let Some(root) = super::root_of(source, roots).cloned() else {
        return refuse("this folder is not inside a library folder".into());
    };
    if source == root {
        return refuse("a library folder itself is never moved".into());
    }
    let folder_name = name(source);
    if crate::platform::detect::platform_for_folder_name(&folder_name).is_some()
        || ["unnamed", "unknown"].contains(&folder_name.to_ascii_lowercase().as_str())
    {
        return refuse(format!(
            "\"{folder_name}\" is a system folder, not one game; choose the game folder inside it"
        ));
    }
    let parent = source.parent().unwrap_or(&root);
    if parent.parent() == Some(root.as_path())
        && parent
            .file_name()
            .and_then(|n| crate::platform::detect::platform_for_folder_name(&n.to_string_lossy()))
            .is_some_and(|p| p.id == platform)
    {
        return FolderPreview::AlreadyInPlace;
    }
    let layout = match capture_folder_layout(source) {
        Ok(layout) => layout,
        Err(why) => return refuse(why),
    };
    if layout.entries.is_empty() {
        return refuse(format!("{folder_name} is empty"));
    }
    // One game per folder: no second game folder nested inside.
    let nested = layout.entries.iter().any(|e| {
        e.is_dir
            && e.relative.components().count() > 1
            && crate::game_package::ps3_game_folder(&source.join(&e.relative)).is_some()
    });
    if nested {
        return refuse(format!(
            "{folder_name} holds more than one game folder; move each game folder on its own"
        ));
    }
    let layout_folder = match crate::dat::rom_organisation::layout_folder_in(&root, platform) {
        Ok(folder) => folder,
        Err(why) => return refuse(why),
    };
    let destination_parent = root.join(&layout_folder);
    let destination = destination_parent.join(&folder_name);
    if destination.starts_with(source) {
        return refuse("the destination would be inside the folder itself".into());
    }
    if std::fs::symlink_metadata(&destination).is_ok() {
        return refuse(format!(
            "{} already exists; nothing is ever overwritten",
            destination.display()
        ));
    }
    let creates_folder = std::fs::symlink_metadata(&destination_parent).is_err();
    if !creates_folder && let Ok(entries) = std::fs::read_dir(&destination_parent) {
        let clash = entries.flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(&folder_name)
        });
        if clash {
            return refuse(format!(
                "{} already holds \"{folder_name}\" in another letter case",
                destination_parent.display()
            ));
        }
    }
    let device_of = if creates_folder {
        &root
    } else {
        &destination_parent
    };
    match std::fs::metadata(device_of) {
        Ok(meta) if meta.dev() == layout.device => {}
        Ok(_) => {
            return refuse(
                "the system folder is on a different filesystem; folders are only moved by rename, never copied"
                    .into(),
            );
        }
        Err(error) => return refuse(error.to_string()),
    }
    let (rows, inside) =
        match crate::database::organisation_paths::folder_rows(database, source, &destination) {
            Ok(found) => found,
            Err(why) => return refuse(why),
        };
    if inside > 0 {
        return refuse(format!(
            "{folder_name} holds {inside} other game{} in your library; a shared folder is never moved as one game",
            if inside == 1 { "" } else { "s" }
        ));
    }
    FolderPreview::Move(Box::new(FolderMovePlan {
        source: source.to_path_buf(),
        destination,
        root,
        platform: platform.to_string(),
        evidence: evidence.to_string(),
        layout,
        rows,
        creates_folder,
    }))
}

/// The record format this release writes and acts on. A record with any
/// other version (written by a newer EmuWiz) is reported by [`unreadable`]
/// and never acted on.
pub const RECORD_VERSION: u32 = 1;

fn journal_path(journal_dir: &Path, id: &str) -> PathBuf {
    journal_dir.join(JOURNAL_SUBDIR).join(format!("{id}.json"))
}

fn sync_dir(dir: &Path) -> Result<(), String> {
    #[cfg(test)]
    faults::sync(dir).map_err(|e| format!("could not make {} durable: {e}", dir.display()))?;
    std::fs::File::open(dir)
        .and_then(|handle| handle.sync_all())
        .map_err(|e| format!("could not make {} durable: {e}", dir.display()))
}

/// Makes the folder entries on both sides of the move durable: the source
/// folder's parent always, the destination's parent while it exists, and
/// the parent of every system folder this move created that still exists
/// (a record written by an earlier release may not have synced it).
fn sync_both_sides(record: &FolderMoveRecord) -> Result<(), String> {
    for dir in &record.created_dirs {
        if std::fs::symlink_metadata(dir).is_ok()
            && let Some(parent) = dir.parent()
        {
            sync_dir(parent)?;
        }
    }
    // The source's parent may have been removed since (an emptied folder):
    // its removal is then durable through the nearest folder that exists.
    sync_dir(nearest_existing(
        record.source.parent().unwrap_or(&record.root),
    ))?;
    if let Some(parent) = record.destination.parent()
        && std::fs::symlink_metadata(parent).is_ok()
    {
        sync_dir(parent)?;
    }
    Ok(())
}

/// `dir` itself, or its nearest ancestor that exists.
fn nearest_existing(dir: &Path) -> &Path {
    dir.ancestors()
        .find(|d| std::fs::symlink_metadata(d).is_ok())
        .unwrap_or(dir)
}

fn rename(
    from: &Path,
    to: &Path,
) -> Result<(), crate::dat::rename_apply::noclobber::NoClobberError> {
    trace(|| format!("rename {} -> {}", from.display(), to.display()));
    crate::dat::rename_apply::noclobber::rename_noreplace(from, to)
}

/// A named step boundary. Tests can stop the process there (a crash).
fn point(_name: &'static str) {
    #[cfg(test)]
    faults::point(_name);
}

fn trace(_what: impl FnOnce() -> String) {
    #[cfg(test)]
    faults::trace(_what());
}

fn write_record(journal_dir: &Path, record: &FolderMoveRecord) -> Result<(), String> {
    let dir = journal_dir.join(JOURNAL_SUBDIR);
    if std::fs::symlink_metadata(&dir).is_err() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("journal folder: {e}"))?;
    }
    // Every time, not only when the folder is created: if that first sync
    // failed, the folder's own entry is still not durable.
    sync_dir(journal_dir)?;
    let text = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    crate::atomic_write_text(&journal_path(journal_dir, &record.id), &text)
        .map_err(|e| format!("journal write failed: {e}"))?;
    // The record's rename into place is durable only once its folder is.
    sync_dir(&dir).map_err(|e| format!("journal write not yet durable: {e}"))?;
    trace(|| format!("record {:?}", record.state));
    Ok(())
}

/// Which side the library names for this move (`true` = the destination),
/// read through SQLite and made durable before it is acted on. With no
/// library rows, only the files decide.
fn library_side(
    record: &FolderMoveRecord,
    at_source: bool,
    at_destination: bool,
) -> Result<Option<bool>, String> {
    if record.rows.is_empty() {
        Ok(Some(at_destination && !at_source))
    } else {
        crate::database::organisation_paths::folder_rows_moved(&record.database, &record.rows)
    }
}

fn unknown_side(read: Result<Option<bool>, String>) -> String {
    match read {
        Err(why) => why,
        Ok(_) => "its rows are split between both places".into(),
    }
}

/// Moves the previewed folder. On failure nothing is left half-done: the
/// folder is put back and the library unchanged (the journal says so).
/// An outcome is recorded only once the renames it describes are durable;
/// when that cannot be established the record stays open (`Planned`) and
/// [`recover`] settles it from what is on disk and in the library.
pub fn apply(
    plan: &FolderMovePlan,
    database_path: &Path,
    journal_dir: &Path,
) -> Result<FolderMoveRecord, String> {
    let now = capture_folder_layout(&plan.source)?;
    if now != plan.layout {
        return Err(format!(
            "{} changed since the preview; preview it again",
            name(&plan.source)
        ));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let mut record = FolderMoveRecord {
        version: RECORD_VERSION,
        id: format!(
            "folder-move-{}-{}-{}",
            stamp.as_secs(),
            std::process::id(),
            stamp.subsec_nanos()
        ),
        source: plan.source.clone(),
        destination: plan.destination.clone(),
        root: plan.root.clone(),
        platform: plan.platform.clone(),
        database: database_path.to_path_buf(),
        layout_digest: digest(&plan.layout),
        entries: plan.layout.entries.len(),
        total_bytes: plan.layout.total_bytes,
        rows: plan.rows.clone(),
        created_dirs: Vec::new(),
        state: FolderMoveState::Planned,
        at_unix: stamp.as_secs(),
    };
    write_record(journal_dir, &record)?;
    point("apply:planned");

    // Nothing was renamed: remove the folders this move created and record
    // RolledBack, or leave the record open if that cannot be made durable.
    let fail = |record: &mut FolderMoveRecord, why: String| -> String {
        if let Err(more) = remove_created(record) {
            return format!(
                "{why}; {more}; the journal {} stays open for recovery",
                record.id
            );
        }
        record.state = FolderMoveState::RolledBack;
        match write_record(journal_dir, record) {
            Ok(()) => why,
            Err(more) => format!(
                "{why}; {more}; the journal {} stays open for recovery",
                record.id
            ),
        }
    };

    // Missing system folder(s) between the root and the destination.
    let parent = plan
        .destination
        .parent()
        .ok_or("destination has no folder")?;
    let source_parent = plan.source.parent().unwrap_or(&plan.root);
    let mut chain: Vec<PathBuf> = parent
        .ancestors()
        .take_while(|dir| *dir != plan.root)
        .map(Path::to_path_buf)
        .collect();
    chain.reverse();
    for dir in chain {
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                trace(|| format!("mkdir {}", dir.display()));
                record.created_dirs.push(dir.clone());
                // The new folder's own entry must be durable before anything
                // is renamed into it, or a power loss could orphan the game.
                let durable = sync_dir(dir.parent().unwrap_or(&plan.root))
                    .and_then(|()| write_record(journal_dir, &record));
                if let Err(why) = durable {
                    return Err(fail(&mut record, why));
                }
                point("apply:created-dir");
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(fail(
                    &mut record,
                    format!("could not create {}: {error}", dir.display()),
                ));
            }
        }
    }

    let library = match FolderCatalogueMove::begin(database_path, &plan.rows, false) {
        Ok(library) => library,
        Err(why) => return Err(fail(&mut record, why)),
    };
    if let Err(error) = rename(&plan.source, &plan.destination) {
        drop(library);
        return Err(fail(&mut record, format!("not moved: {error}")));
    }
    point("apply:renamed");
    let moved_back = |record: &mut FolderMoveRecord, why: String| -> String {
        match rename(&plan.destination, &plan.source) {
            Ok(()) => {
                point("apply:moved-back");
                match sync_both_sides(record) {
                    Ok(()) => fail(record, format!("{why}; the folder was put back")),
                    Err(more) => format!(
                        "{why}; the folder was put back but {more}; the journal {} stays open for recovery",
                        record.id
                    ),
                }
            }
            Err(error) => {
                record.state = FolderMoveState::NeedsReview;
                let kept = write_record(journal_dir, record)
                    .err()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default();
                format!(
                    "{why}; putting it back also failed ({error}). It is at {} and the journal {} records both places{kept}",
                    plan.destination.display(),
                    record.id
                )
            }
        }
    };
    // The rename is durable on both sides before the library may say so.
    if let Err(why) = sync_dir(source_parent).and_then(|()| sync_dir(parent)) {
        drop(library);
        return Err(moved_back(&mut record, why));
    }
    point("apply:rename-durable");
    match capture_folder_layout(&plan.destination) {
        Ok(after) if after.entries == plan.layout.entries => {}
        Ok(_) => {
            drop(library);
            return Err(moved_back(
                &mut record,
                "the folder's contents differ after the move".into(),
            ));
        }
        Err(why) => {
            drop(library);
            return Err(moved_back(&mut record, why));
        }
    }
    if let Err(why) = library.commit() {
        // A commit that reports failure can still have taken effect (for
        // example when the folder fsync after SQLite's commit point fails).
        // Decide from what the library shows, made durable first.
        match library_side(&record, false, true) {
            Ok(Some(true)) if !record.rows.is_empty() => {}
            Ok(Some(_)) => return Err(moved_back(&mut record, why)),
            other => {
                return Err(format!(
                    "{why}; whether the library recorded the move is unknown ({}); the journal {} stays open for recovery",
                    unknown_side(other),
                    record.id
                ));
            }
        }
    }
    trace(|| "commit".into());
    point("apply:committed");
    record.state = FolderMoveState::Committed;
    write_record(journal_dir, &record).map_err(|why| {
        format!("moved and recorded in the library, but the journal could not be finished: {why}")
    })?;
    Ok(record)
}

/// Removes the folders this move created, deepest first, if they are empty
/// (never anything else), and makes each removal durable. Repeatable: a
/// folder already gone, or one something else now lives in, is left as is.
fn remove_created(record: &FolderMoveRecord) -> Result<(), String> {
    for dir in record.created_dirs.iter().rev() {
        match std::fs::remove_dir(dir) {
            Ok(()) => trace(|| format!("rmdir {}", dir.display())),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => return Err(format!("could not remove {}: {error}", dir.display())),
        }
    }
    for dir in record.created_dirs.iter().rev() {
        if let Some(parent) = dir.parent()
            && std::fs::symlink_metadata(parent).is_ok()
        {
            sync_dir(parent)?;
        }
    }
    Ok(())
}

/// Records the move as Committed again after an Undo that changed nothing
/// durable, or leaves it open for recovery.
fn back_to_committed(record: &mut FolderMoveRecord, journal_dir: &Path, why: String) -> String {
    record.state = FolderMoveState::Committed;
    match write_record(journal_dir, record) {
        Ok(()) => why,
        Err(more) => format!(
            "{why}; {more}; the journal {} stays open for recovery",
            record.id
        ),
    }
}

/// Undo renamed the folder back but the library was not changed: return the
/// folder to the destination and record Committed again.
fn return_to_destination(record: &mut FolderMoveRecord, journal_dir: &Path, why: String) -> String {
    match rename(&record.source, &record.destination) {
        Ok(()) => {
            point("undo:returned");
            match sync_both_sides(record) {
                Ok(()) => back_to_committed(record, journal_dir, why),
                Err(more) => format!(
                    "{why}; the folder was returned but {more}; the journal {} stays open for recovery",
                    record.id
                ),
            }
        }
        Err(error) => {
            record.state = FolderMoveState::NeedsReview;
            let kept = write_record(journal_dir, record)
                .err()
                .map(|e| format!(" ({e})"))
                .unwrap_or_default();
            format!(
                "{why}; returning it also failed ({error}). It is at {} and the journal {} records both places{kept}",
                record.source.display(),
                record.id
            )
        }
    }
}

/// Puts a moved folder back where it was, with its library path.
///
/// The intent is recorded first (`Undoing`), so an Undo interrupted at any
/// point is found by [`interrupted`] and finished or reversed by
/// [`recover`]; the outcome (`Undone`) is recorded only once the rename,
/// the library change and the removal of created folders are durable.
pub fn undo(record: &mut FolderMoveRecord, journal_dir: &Path) -> Result<(), String> {
    if record.state != FolderMoveState::Committed {
        return Err("only a completed folder move can be undone".into());
    }
    let layout = capture_folder_layout(&record.destination)?;
    if digest(&layout) != record.layout_digest {
        return Err(format!(
            "{} changed since it was moved; review it before putting it back",
            record.destination.display()
        ));
    }
    if std::fs::symlink_metadata(&record.source).is_ok() {
        return Err(format!(
            "{} exists again; nothing is ever overwritten",
            record.source.display()
        ));
    }
    record.state = FolderMoveState::Undoing;
    write_record(journal_dir, record)?;
    point("undo:undoing");
    let library = match FolderCatalogueMove::begin(&record.database, &record.rows, true) {
        Ok(library) => library,
        Err(why) => return Err(back_to_committed(record, journal_dir, why)),
    };
    if let Err(error) = rename(&record.destination, &record.source) {
        drop(library);
        return Err(back_to_committed(
            record,
            journal_dir,
            format!("not put back: {error}"),
        ));
    }
    point("undo:renamed");
    if let Err(why) = sync_both_sides(record) {
        drop(library);
        return Err(return_to_destination(record, journal_dir, why));
    }
    point("undo:rename-durable");
    if let Err(why) = library.commit() {
        // As in apply: decide from the durable library state.
        match library_side(record, true, false) {
            Ok(Some(false)) if !record.rows.is_empty() => {}
            Ok(Some(_)) => return Err(return_to_destination(record, journal_dir, why)),
            other => {
                return Err(format!(
                    "{why}; whether the library recorded the Undo is unknown ({}); the journal {} stays open for recovery",
                    unknown_side(other),
                    record.id
                ));
            }
        }
    }
    trace(|| "commit".into());
    point("undo:committed");
    remove_created(record)
        .map_err(|why| format!("{why}; the journal {} stays open for recovery", record.id))?;
    point("undo:removed-created");
    record.state = FolderMoveState::Undone;
    write_record(journal_dir, record)
}

/// Every folder-move record in the journal this release can act on, newest
/// first. The others are listed by [`unreadable`].
pub fn records(journal_dir: &Path) -> Vec<FolderMoveRecord> {
    let mut found: Vec<FolderMoveRecord> = std::fs::read_dir(journal_dir.join(JOURNAL_SUBDIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|text| serde_json::from_str::<FolderMoveRecord>(&text).ok())
        .filter(|r| r.version == RECORD_VERSION)
        .collect();
    found.sort_by(|a, b| b.id.cmp(&a.id));
    found
}

/// Folder-move records that cannot be read, name a state this release does
/// not know, or carry another record version, with the reason. They are never skipped silently: Organise
/// reports them and leaves them untouched, and the upgrade preflight and
/// installer refuse while they exist.
pub fn unreadable(journal_dir: &Path) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let dir = journal_dir.join(JOURNAL_SUBDIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return found,
        Err(error) => {
            found.push((
                dir,
                format!("the folder-move journal cannot be read: {error}"),
            ));
            return found;
        }
    };
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                found.push((
                    dir.clone(),
                    format!("the folder-move journal cannot be listed: {error}"),
                ));
                continue;
            }
        };
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let read = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                serde_json::from_str::<FolderMoveRecord>(&text).map_err(|e| e.to_string())
            })
            .and_then(|r| {
                if r.version == RECORD_VERSION {
                    Ok(())
                } else {
                    Err(format!(
                        "record version {} was written by a newer EmuWiz; this release reads version {RECORD_VERSION} only and does not act on it",
                        r.version
                    ))
                }
            });
        if let Err(why) = read {
            found.push((path, why));
        }
    }
    found.sort();
    found
}

/// Records left open by an interruption: a move (`Planned`) or an Undo
/// (`Undoing`).
pub fn interrupted(journal_dir: &Path) -> Vec<FolderMoveRecord> {
    records(journal_dir)
        .into_iter()
        .filter(|r| matches!(r.state, FolderMoveState::Planned | FolderMoveState::Undoing))
        .collect()
}

/// Settles an interrupted move or Undo from what is on disk and in the
/// library, never from the journal alone. Idempotent: running it again
/// after any interruption, including one inside it, reaches the same
/// outcome. An outcome is recorded only after the folder entries on both
/// sides and the library state it was decided from are durable; on any
/// error the record stays open and nothing is claimed.
pub fn recover(
    record: &mut FolderMoveRecord,
    journal_dir: &Path,
) -> Result<FolderMoveState, String> {
    let undoing = match record.state {
        FolderMoveState::Planned => false,
        FolderMoveState::Undoing => true,
        settled => return Ok(settled),
    };
    let at_source = std::fs::symlink_metadata(&record.source).is_ok();
    let at_destination = std::fs::symlink_metadata(&record.destination).is_ok();
    let library = library_side(record, at_source, at_destination)?;
    // Where the folder ends up: true = destination, false = source.
    let settled = match (at_source, at_destination, library) {
        (true, false, Some(false)) => Some(false),
        (false, true, Some(true)) => Some(true),
        (false, true, Some(false)) => {
            // Renamed but the library names the source (a move whose
            // library change never committed, or an Undo whose did): the
            // folder goes to the source.
            rename(&record.destination, &record.source)
                .map_err(|e| format!("could not put the folder back: {e}"))?;
            point("recover:put-back");
            Some(false)
        }
        (true, false, Some(true)) if undoing => {
            // An Undo renamed the folder back but its library change never
            // committed: return it to where the library says it is.
            rename(&record.source, &record.destination)
                .map_err(|e| format!("could not return the folder: {e}"))?;
            point("recover:returned");
            Some(true)
        }
        _ => None,
    };
    record.state = match settled {
        Some(at_destination) => {
            sync_both_sides(record)?;
            point("recover:durable");
            if at_destination {
                FolderMoveState::Committed
            } else {
                remove_created(record)?;
                if undoing {
                    FolderMoveState::Undone
                } else {
                    FolderMoveState::RolledBack
                }
            }
        }
        None => FolderMoveState::NeedsReview,
    };
    if let Err(why) = write_record(journal_dir, record) {
        record.state = if undoing {
            FolderMoveState::Undoing
        } else {
            FolderMoveState::Planned
        };
        return Err(why);
    }
    Ok(record.state)
}

/// Test-only crash points, directory-sync failures and a trace of every
/// durable step, per thread.
#[cfg(test)]
pub(crate) mod faults {
    use std::cell::RefCell;
    use std::path::Path;

    thread_local! {
        static CRASH_AT: RefCell<Option<&'static str>> = const { RefCell::new(None) };
        static FAIL_SYNC: RefCell<Option<usize>> = const { RefCell::new(None) };
        static SYNCS: RefCell<usize> = const { RefCell::new(0) };
        static TRACE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    /// Clears every fault and the trace.
    pub(crate) fn reset() {
        CRASH_AT.with(|c| *c.borrow_mut() = None);
        FAIL_SYNC.with(|f| *f.borrow_mut() = None);
        SYNCS.with(|s| *s.borrow_mut() = 0);
        TRACE.with(|t| t.borrow_mut().clear());
    }

    /// The next time `point` is reached, the thread panics (a crash: no
    /// further code runs except destructors, so an open SQLite transaction
    /// rolls back as it would after process death).
    pub(crate) fn crash_at(point: &'static str) {
        CRASH_AT.with(|c| *c.borrow_mut() = Some(point));
    }

    /// The directory sync with this index (from 0, counted from now) fails.
    pub(crate) fn fail_sync(index: usize) {
        SYNCS.with(|s| *s.borrow_mut() = 0);
        FAIL_SYNC.with(|f| *f.borrow_mut() = Some(index));
    }

    pub(crate) fn syncs() -> usize {
        SYNCS.with(|s| *s.borrow())
    }

    pub(crate) fn take_trace() -> Vec<String> {
        TRACE.with(|t| std::mem::take(&mut *t.borrow_mut()))
    }

    pub(super) fn trace(what: String) {
        TRACE.with(|t| t.borrow_mut().push(what));
    }

    pub(super) fn point(name: &'static str) {
        trace(format!("point {name}"));
        let hit = CRASH_AT.with(|c| {
            let mut c = c.borrow_mut();
            if *c == Some(name) {
                *c = None;
                true
            } else {
                false
            }
        });
        if hit {
            panic!("injected crash at {name}");
        }
    }

    pub(super) fn sync(dir: &Path) -> std::io::Result<()> {
        let index = SYNCS.with(|s| {
            let mut s = s.borrow_mut();
            *s += 1;
            *s - 1
        });
        let fail = FAIL_SYNC.with(|f| {
            let mut f = f.borrow_mut();
            if *f == Some(index) {
                *f = None;
                true
            } else {
                false
            }
        });
        if fail {
            return Err(std::io::Error::other("injected sync failure"));
        }
        trace(format!("sync {}", dir.display()));
        Ok(())
    }
}
