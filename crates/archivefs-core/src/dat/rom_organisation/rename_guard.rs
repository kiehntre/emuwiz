//! Bulk rename guard: a bulk Quick Rename / Organise plan refuses every file
//! the one-game Rename (Blue Game Details) refuses.
//!
//! The organiser plan only knows names, folders and collisions. Renaming or
//! moving the real file can still break a game in ways the plan cannot see:
//!
//! - an arcade romset is found by its set name (MAME, FBNeo);
//! - a `.cue`/`.m3u`/`.m3u8`/`.gdi` names the file, so renaming it alone
//!   stops the disc (or the whole multi-disc set) launching. In Move mode a
//!   file of a complete disc set moves with its sheets, names unchanged
//!   ([`super::disc_sets`]), so only a sheet outside its set refuses it;
//! - RetroArch finds saves and save states by the game's file name. Bulk
//!   rename does not move saves yet, so a game with saves is left for the
//!   one-game Rename, which moves them in the same journal.
//!
//! The guard never changes a file. It turns affected `Suggested` entries into
//! `Blocked` entries with a plain reason, using the one-game Rename's own
//! checks ([`super::game_rename`]). It runs on the preview and again just
//! before Apply ([`refuse_if_newly_blocked`]), so a sidecar or save that
//! appeared after the preview refuses the Apply with nothing changed.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::game_rename::{
    RetroArchSaveLocations, SaveSearchIndex, SaveSearchStats, find_retroarch_saves_with,
    folder_sidecars, owning_source_root_in, refuse_arcade_set_in,
};
use super::model::{OrganisationMode, OrganisationPlan, OrganisationStatus};
use super::split_disc_lock::{SplitSetLock, split_set_locks};
use super::transaction::live_resolution_for;
use crate::platform::identity::{PlatformIdentityResolution, resolve_platform_identity};

/// Every library path's archive id, read in one pass per guard run. Without
/// it each game's platform lookup scans the whole archive table (the path
/// column has no index), which made a 4000-game preview quadratic.
pub(crate) struct LiveResolutions<'a> {
    database: &'a crate::Database,
    ids: Option<crate::database::ArchivePathLookup>,
}

impl<'a> LiveResolutions<'a> {
    pub(crate) fn read(database: &'a crate::Database) -> Self {
        Self {
            database,
            ids: database.archive_path_lookup().ok(),
        }
    }

    /// Exactly [`live_resolution_for`]'s answer: a path that is not in the
    /// library, or that two rows share, is `Unknown`. If the one-pass read
    /// failed, falls back to `live_resolution_for` itself.
    pub(crate) fn for_path(&self, source: &Path, generation: u64) -> PlatformIdentityResolution {
        let Some(ids) = &self.ids else {
            return live_resolution_for(self.database, source, generation);
        };
        match ids.find(source).ok().flatten() {
            Some(archive_id) => {
                let evidence = self
                    .database
                    .current_platform_identity_evidence(archive_id, generation)
                    .ok()
                    .unwrap_or_default();
                resolve_platform_identity(generation, evidence)
            }
            None => PlatformIdentityResolution::Unknown { generation },
        }
    }
}

/// Whether `mode` changes the real file's path (and so can break a game).
/// Symlink-only and linked-library modes never touch the real file.
pub fn mode_changes_real_files(mode: OrganisationMode) -> bool {
    matches!(
        mode,
        OrganisationMode::RenameInPlace | OrganisationMode::MoveRealFile
    )
}

/// Link modes: the original stays put, but a linked library holds only the
/// files it links. Build linked library links a complete disc set together
/// (names unchanged, see `disc_sets`); a disc-set file outside a complete
/// set, or any disc-set file when reorganising existing links, is refused
/// with the reason (owner 2026-11-23: never silently create incomplete sets).
pub fn mode_links_files(mode: OrganisationMode) -> bool {
    matches!(
        mode,
        OrganisationMode::BuildLinkedLibrary | OrganisationMode::OrganiseSymlinkOnly
    )
}

/// Whether the bulk guard runs for this mode (preview and again at Apply).
pub fn mode_is_guarded(mode: OrganisationMode) -> bool {
    mode_changes_real_files(mode) || mode_links_files(mode)
}

const LINKED_DISC_SET: &str = "Linking it on its own would give an incomplete game, so nothing will be linked for this file. A complete disc set (its .cue, .m3u or .gdi and every file it names, in one folder) is linked together with its names unchanged.";

/// What one guard run did: how many rows it blocked and how much it read.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GuardStats {
    pub checked: usize,
    pub blocked: usize,
    /// Folders read for cue sheets and playlists (each once per run).
    pub sidecar_folder_reads: u64,
    pub saves: SaveSearchStats,
}

/// Marks every `Suggested` entry that the one-game Rename would refuse as
/// `Blocked`, with the reason. Returns how many entries were blocked. Plans in
/// modes that leave the real file alone are returned unchanged.
pub fn guard_bulk_rename_plan(
    database: &crate::Database,
    plan: &mut OrganisationPlan,
    save_locations: &RetroArchSaveLocations,
) -> usize {
    guard_bulk_rename_plan_with_stats(database, plan, save_locations).blocked
}

/// [`guard_bulk_rename_plan`], also reporting what it read. Every folder
/// (game folders, their sub-folders, RetroArch save folders, folders that may
/// hold cue sheets) is read at most once per run; nothing is kept afterwards.
pub fn guard_bulk_rename_plan_with_stats(
    database: &crate::Database,
    plan: &mut OrganisationPlan,
    save_locations: &RetroArchSaveLocations,
) -> GuardStats {
    guard_with_index(database, plan, save_locations, SaveSearchIndex::default())
}

/// The guard run with a given (fresh) save index; tests pass one that knows
/// their pretend mount points.
pub(crate) fn guard_with_index(
    database: &crate::Database,
    plan: &mut OrganisationPlan,
    save_locations: &RetroArchSaveLocations,
    mut saves: SaveSearchIndex,
) -> GuardStats {
    let mut stats = GuardStats::default();
    if !mode_is_guarded(plan.mode) {
        return stats;
    }
    let mut sidecars = SidecarCache::default();
    let resolutions = LiveResolutions::read(database);
    // Read once per run (each preview and each Apply check reads it afresh).
    let folders = match database.list_source_folders() {
        Ok(folders) => Ok(folders),
        Err(error) => Err(format!("could not read the library folders: {error}")),
    };
    // Move mode: a disc-set file moves with its own set's sheets (names
    // unchanged), so those sheets naming it is expected; any other sheet
    // naming it still refuses it.
    let mut own_sheets: HashMap<PathBuf, BTreeSet<PathBuf>> = HashMap::new();
    if matches!(
        plan.mode,
        OrganisationMode::MoveRealFile | OrganisationMode::BuildLinkedLibrary
    ) {
        for entry in &plan.entries {
            if let Some(sheet) = &entry.disc_set
                && super::disc_sets::sheet_kind(&entry.source_path).is_some()
            {
                own_sheets
                    .entry(sheet.clone())
                    .or_default()
                    .insert(entry.source_path.clone());
            }
        }
    }
    let mut blocked = 0;
    for entry in &mut plan.entries {
        if entry.status != OrganisationStatus::Suggested
            || entry.destination_path == entry.source_path
        {
            continue;
        }
        stats.checked += 1;
        let refusal = match &folders {
            Err(reason) => Some(reason.clone()),
            Ok(folders) => refusal_for(
                folders,
                &resolutions,
                &entry.source_path,
                &plan.master_root,
                plan.generation,
                plan.mode,
                entry
                    .disc_set
                    .as_ref()
                    .and_then(|sheet| own_sheets.get(sheet)),
                save_locations,
                &mut sidecars,
                &mut saves,
            ),
        };
        if let Some(reason) = refusal {
            entry.status = OrganisationStatus::Blocked;
            entry.reason = Some(reason);
            blocked += 1;
        }
    }
    // Every file of a disc set moves or none does.
    blocked += super::disc_sets::hold_disc_sets_together(&mut plan.entries);
    stats.blocked = blocked;
    stats.sidecar_folder_reads = sidecars.folders.len() as u64;
    stats.saves = saves.stats;
    stats
}

/// Apply-time check: runs the guard again on a copy of the reviewed plan and
/// refuses when any approved entry would now be blocked. Nothing is changed
/// on disk either way.
pub fn refuse_if_newly_blocked(
    database: &crate::Database,
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
    save_locations: &RetroArchSaveLocations,
) -> Result<(), String> {
    if !mode_is_guarded(plan.mode) {
        return Ok(());
    }
    let mut fresh = plan.clone();
    guard_bulk_rename_plan(database, &mut fresh, save_locations);
    for (reviewed, now) in plan.entries.iter().zip(&fresh.entries) {
        let source = reviewed.source_path.to_string_lossy();
        if reviewed.status == OrganisationStatus::Suggested
            && now.status == OrganisationStatus::Blocked
            && approved.contains(source.as_ref())
        {
            return Err(format!(
                "{} changed since the preview: {} Nothing was changed. Preview again.",
                reviewed.source_path.display(),
                now.reason
                    .as_deref()
                    .unwrap_or("it can no longer be renamed safely.")
            ));
        }
    }
    Ok(())
}

/// Why this file must not be renamed or moved in bulk, or `None`.
#[allow(clippy::too_many_arguments)]
fn refusal_for(
    folders: &[crate::SourceFolderRecord],
    resolutions: &LiveResolutions<'_>,
    source: &Path,
    plan_root: &Path,
    generation: u64,
    mode: OrganisationMode,
    own_sheets: Option<&BTreeSet<PathBuf>>,
    save_locations: &RetroArchSaveLocations,
    sidecars: &mut SidecarCache,
    saves: &mut SaveSearchIndex,
) -> Option<String> {
    // The library folder that holds the file bounds the sidecar search, as
    // in the one-game Rename. A file outside every library folder (a Quick
    // Rename root that is not a library folder) uses the plan's root, or
    // only its own folder when it is outside that too.
    let source_root = owning_source_root_in(folders, source)
        .ok()
        .or_else(|| {
            source
                .starts_with(plan_root)
                .then(|| plan_root.to_path_buf())
        })
        .or_else(|| source.parent().map(Path::to_path_buf))?;
    if mode_links_files(mode) {
        // A file of a complete disc set is linked with its own sheets
        // (`own_sheets`); a sheet outside the set, or a set that could not
        // be grouped, refuses only that file and its set.
        if super::disc_sets::sheet_kind(source).is_some() && own_sheets.is_none() {
            return Some(format!(
                "This is a disc list that is not part of a complete disc set here. {LINKED_DISC_SET}"
            ));
        }
        return match sidecars.referencing(source, &source_root, own_sheets) {
            Err(reason) => Some(reason),
            Ok(Some(referrer)) => Some(format!(
                "{} names this file and is not linked with it. {LINKED_DISC_SET}",
                referrer.display()
            )),
            Ok(None) => None,
        };
    }
    let resolution = resolutions.for_path(source, generation);
    if let Err(reason) = refuse_arcade_set_in(folders, &resolution, &source_root) {
        return Some(reason);
    }
    match sidecars.referencing(source, &source_root, own_sheets) {
        Err(reason) => return Some(reason),
        Ok(Some(referrer)) if mode == OrganisationMode::MoveRealFile => {
            return Some(format!(
                "{} names this file and does not move with it, so moving this file would stop the game launching. Nothing will change for this file.",
                referrer.display()
            ));
        }
        Ok(Some(referrer)) => {
            return Some(format!(
                "{} names this file, so renaming it would stop the game launching. Disc-set files are not renamed in bulk; use Move to move the whole set together (names unchanged), or rename it on its own. Nothing will change for this file.",
                referrer.display()
            ));
        }
        Ok(None) => {}
    }
    match find_retroarch_saves_with(source, save_locations, saves) {
        Err(reason) => Some(reason),
        Ok(saves) if !saves.is_empty() => Some(format!(
            "This game has {} RetroArch save{} or save state{} named after it. Bulk rename does not move saves yet: rename it on its own from Game Details, which moves its saves with it. Nothing will change for this file.",
            saves.len(),
            if saves.len() == 1 { "" } else { "s" },
            if saves.len() == 1 { "" } else { "s" },
        )),
        Ok(_) => None,
    }
}

/// Each folder's sidecars, read once per guard run.
#[derive(Default)]
struct SidecarCache {
    folders: HashMap<PathBuf, Result<Vec<(PathBuf, String)>, String>>,
}

impl SidecarCache {
    /// The first sidecar in the file's folder, or a folder above it up to and
    /// including `source_root`, whose text names the file (same walk as the
    /// one-game Rename). Sheets in `exclude` (the file's own disc set, which
    /// moves with it) are skipped.
    fn referencing(
        &mut self,
        source: &Path,
        source_root: &Path,
        exclude: Option<&BTreeSet<PathBuf>>,
    ) -> Result<Option<PathBuf>, String> {
        let Some(name) = source.file_name() else {
            return Ok(None);
        };
        let name = name.to_string_lossy().to_lowercase();
        let mut folder = source.parent();
        while let Some(dir) = folder {
            let listed = self
                .folders
                .entry(dir.to_path_buf())
                .or_insert_with(|| folder_sidecars(dir));
            match listed {
                Err(reason) => return Err(reason.clone()),
                Ok(found) => {
                    if let Some((path, _)) = found.iter().find(|(path, text)| {
                        path != source
                            && !exclude.is_some_and(|own| own.contains(path))
                            && text.contains(&name)
                    }) {
                        return Ok(Some(path.clone()));
                    }
                }
            }
            if dir == source_root || !dir.starts_with(source_root) {
                break;
            }
            folder = dir.parent();
        }
        Ok(None)
    }
}

/// Files of disc sets an earlier operation left not whole (read from the
/// journals in `journal_dir`, located on disk), with the set they belong to.
/// An unreadable journal folder yields nothing here; Apply cannot run
/// without that folder anyway.
fn split_disc_set_files(journal_dir: &Path) -> std::collections::BTreeMap<PathBuf, SplitSetLock> {
    if journal_dir.as_os_str().is_empty() {
        return std::collections::BTreeMap::new();
    }
    let (journals, _unreadable) = crate::dat::rename_apply::journal::list_journals(journal_dir);
    split_set_locks(&journals)
}

/// As [`split_disc_set_files`], but fails closed: History that cannot be
/// read completely refuses, because a split set in it cannot be ruled out.
fn split_disc_set_files_or_refuse(
    journal_dir: &Path,
) -> Result<std::collections::BTreeMap<PathBuf, SplitSetLock>, String> {
    let (journals, problems) = crate::dat::rename_apply::journal::list_journals(journal_dir);
    if let Some(problem) = problems.first() {
        let more = match problems.len() {
            1 => String::new(),
            n => format!(" (and {} more)", n - 1),
        };
        return Err(format!(
            "A History record could not be read: {problem}{more}. It may record a disc set an earlier operation left in two places, so nothing is moved until every History record can be read. Open History to see which record is affected; the records are files in {}. Nothing was changed.",
            journal_dir.display()
        ));
    }
    Ok(split_set_locks(&journals))
}

/// For moves outside Bulk Rename (Folder platform check): refuses if any of
/// `paths` (files to move, and where they would go) belongs to a disc set an
/// earlier operation left not whole, or if History cannot be read.
pub fn refuse_split_disc_set_paths<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    journal_dir: &Path,
) -> Result<(), String> {
    let files = split_disc_set_files_or_refuse(journal_dir)?;
    for path in paths {
        if let Some(lock) = files.get(path) {
            return Err(format!(
                "{}: {} Nothing was changed.",
                path.display(),
                lock.reason()
            ));
        }
    }
    Ok(())
}

/// For whole-folder moves: refuses if a file of a disc set an earlier
/// operation left not whole is inside `source` or `destination`, or if
/// History cannot be read.
pub fn refuse_split_disc_sets_in_folders(
    source: &Path,
    destination: &Path,
    journal_dir: &Path,
) -> Result<(), String> {
    let files = split_disc_set_files_or_refuse(journal_dir)?;
    if let Some((path, lock)) = files
        .iter()
        .find(|(path, _)| path.starts_with(source) || path.starts_with(destination))
    {
        return Err(format!(
            "{} was not moved: {}: {} Nothing was changed.",
            source.display(),
            path.display(),
            lock.reason()
        ));
    }
    Ok(())
}

/// Blocks, in any mode, every file of a disc set an earlier operation left
/// not whole. Returns how many rows were blocked.
pub fn block_split_disc_sets(plan: &mut OrganisationPlan, journal_dir: &Path) -> usize {
    let files = split_disc_set_files(journal_dir);
    if files.is_empty() {
        return 0;
    }
    let mut blocked = 0;
    for entry in &mut plan.entries {
        if let Some(lock) = files.get(&entry.source_path)
            && entry.status != OrganisationStatus::Blocked
        {
            entry.status = OrganisationStatus::Blocked;
            entry.reason = Some(lock.reason());
            blocked += 1;
        }
    }
    blocked
}

/// At Apply, read fresh: refuses the whole Apply if an approved file belongs
/// to a disc set an earlier operation left not whole.
pub fn refuse_split_disc_sets(
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
    journal_dir: &Path,
) -> Result<(), String> {
    let files = split_disc_set_files(journal_dir);
    for entry in &plan.entries {
        if approved.contains(entry.source_path.to_string_lossy().as_ref())
            && let Some(lock) = files.get(&entry.source_path)
        {
            return Err(format!(
                "{}: {} Nothing was changed.",
                entry.source_path.display(),
                lock.reason()
            ));
        }
    }
    Ok(())
}
