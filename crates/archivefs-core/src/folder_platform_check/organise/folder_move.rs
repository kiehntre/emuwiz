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
//! 3. Undo: the same steps in reverse, refusing if the folder changed since.
//!
//! Interrupted moves are found by [`interrupted`] and settled by
//! [`recover`], which decides from what is actually on disk and in the
//! library, never from the journal alone.

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

fn journal_path(journal_dir: &Path, id: &str) -> PathBuf {
    journal_dir.join(JOURNAL_SUBDIR).join(format!("{id}.json"))
}

fn sync_dir(dir: &Path) {
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
}

fn write_record(journal_dir: &Path, record: &FolderMoveRecord) -> Result<(), String> {
    let dir = journal_dir.join(JOURNAL_SUBDIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("journal folder: {e}"))?;
    let text = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    crate::atomic_write_text(&journal_path(journal_dir, &record.id), &text)
        .map_err(|e| format!("journal write failed: {e}"))?;
    sync_dir(&dir);
    Ok(())
}

/// Moves the previewed folder. On failure nothing is left half-done: the
/// folder is put back and the library unchanged (the journal says so).
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
        version: 1,
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

    let fail = |record: &mut FolderMoveRecord, why: String| -> String {
        remove_created(record);
        record.state = FolderMoveState::RolledBack;
        let _ = write_record(journal_dir, record);
        why
    };

    // Missing system folder(s) between the root and the destination.
    let parent = plan
        .destination
        .parent()
        .ok_or("destination has no folder")?;
    let mut chain: Vec<PathBuf> = parent
        .ancestors()
        .take_while(|dir| *dir != plan.root)
        .map(Path::to_path_buf)
        .collect();
    chain.reverse();
    for dir in chain {
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                record.created_dirs.push(dir.clone());
                if let Err(why) = write_record(journal_dir, &record) {
                    return Err(fail(&mut record, why));
                }
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
    if let Err(error) =
        crate::dat::rename_apply::noclobber::rename_noreplace(&plan.source, &plan.destination)
    {
        drop(library);
        return Err(fail(&mut record, format!("not moved: {error}")));
    }
    sync_dir(plan.source.parent().unwrap_or(&plan.root));
    sync_dir(parent);
    let moved_back = |record: &mut FolderMoveRecord, why: String| -> String {
        match crate::dat::rename_apply::noclobber::rename_noreplace(&plan.destination, &plan.source)
        {
            Ok(()) => {
                sync_dir(parent);
                sync_dir(plan.source.parent().unwrap_or(&plan.root));
                fail(record, format!("{why}; the folder was put back"))
            }
            Err(error) => {
                record.state = FolderMoveState::NeedsReview;
                let _ = write_record(journal_dir, record);
                format!(
                    "{why}; putting it back also failed ({error}). It is at {} and the journal {} records both places",
                    plan.destination.display(),
                    record.id
                )
            }
        }
    };
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
        return Err(moved_back(&mut record, why));
    }
    record.state = FolderMoveState::Committed;
    write_record(journal_dir, &record).map_err(|why| {
        format!("moved and recorded in the library, but the journal could not be finished: {why}")
    })?;
    Ok(record)
}

fn remove_created(record: &FolderMoveRecord) {
    for dir in record.created_dirs.iter().rev() {
        // Only empty folders this move created; never anything else.
        let _ = std::fs::remove_dir(dir);
    }
}

/// Puts a moved folder back where it was, with its library path.
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
    let library = FolderCatalogueMove::begin(&record.database, &record.rows, true)?;
    crate::dat::rename_apply::noclobber::rename_noreplace(&record.destination, &record.source)
        .map_err(|e| format!("not put back: {e}"))?;
    let source_parent = record.source.parent().unwrap_or(&record.root).to_path_buf();
    sync_dir(&source_parent);
    if let Some(parent) = record.destination.parent() {
        sync_dir(parent);
    }
    if let Err(why) = library.commit() {
        // Keep files and library in agreement: return the folder.
        let _ = crate::dat::rename_apply::noclobber::rename_noreplace(
            &record.source,
            &record.destination,
        );
        return Err(why);
    }
    remove_created(record);
    record.state = FolderMoveState::Undone;
    write_record(journal_dir, record)
}

/// Every folder-move record in the journal, newest first.
pub fn records(journal_dir: &Path) -> Vec<FolderMoveRecord> {
    let mut found: Vec<FolderMoveRecord> = std::fs::read_dir(journal_dir.join(JOURNAL_SUBDIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    found.sort_by(|a, b| b.id.cmp(&a.id));
    found
}

/// Records left in `Planned` by an interruption.
pub fn interrupted(journal_dir: &Path) -> Vec<FolderMoveRecord> {
    records(journal_dir)
        .into_iter()
        .filter(|r| r.state == FolderMoveState::Planned)
        .collect()
}

/// Settles an interrupted move from what is on disk and in the library.
pub fn recover(
    record: &mut FolderMoveRecord,
    journal_dir: &Path,
) -> Result<FolderMoveState, String> {
    if record.state != FolderMoveState::Planned {
        return Ok(record.state);
    }
    let at_source = std::fs::symlink_metadata(&record.source).is_ok();
    let at_destination = std::fs::symlink_metadata(&record.destination).is_ok();
    // With no library rows, only the files decide.
    let library = if record.rows.is_empty() {
        Some(at_destination && !at_source)
    } else {
        crate::database::organisation_paths::folder_rows_moved(&record.database, &record.rows)?
    };
    record.state = match (at_source, at_destination, library) {
        (true, false, Some(false)) => {
            remove_created(record);
            FolderMoveState::RolledBack
        }
        (false, true, Some(true)) => FolderMoveState::Committed,
        (false, true, Some(false)) => {
            // Renamed but the library never committed: put it back.
            crate::dat::rename_apply::noclobber::rename_noreplace(
                &record.destination,
                &record.source,
            )
            .map_err(|e| format!("could not put the folder back: {e}"))?;
            remove_created(record);
            FolderMoveState::RolledBack
        }
        _ => FolderMoveState::NeedsReview,
    };
    write_record(journal_dir, record)?;
    Ok(record.state)
}
