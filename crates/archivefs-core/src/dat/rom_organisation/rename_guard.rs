//! Bulk rename guard: a bulk Quick Rename / Organise plan refuses every file
//! the one-game Rename (Blue Game Details) refuses.
//!
//! The organiser plan only knows names, folders and collisions. Renaming or
//! moving the real file can still break a game in ways the plan cannot see:
//!
//! - an arcade romset is found by its set name (MAME, FBNeo);
//! - a `.cue`/`.m3u`/`.m3u8`/`.gdi` names the file, so renaming it alone
//!   stops the disc (or the whole multi-disc set) launching;
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
    RetroArchSaveLocations, find_retroarch_saves, folder_sidecars, owning_source_root,
    refuse_arcade_set,
};
use super::model::{OrganisationMode, OrganisationPlan, OrganisationStatus};

/// Whether `mode` changes the real file's path (and so can break a game).
/// Symlink-only and linked-library modes never touch the real file.
pub fn mode_changes_real_files(mode: OrganisationMode) -> bool {
    matches!(
        mode,
        OrganisationMode::RenameInPlace | OrganisationMode::MoveRealFile
    )
}

/// Marks every `Suggested` entry that the one-game Rename would refuse as
/// `Blocked`, with the reason. Returns how many entries were blocked. Plans in
/// modes that leave the real file alone are returned unchanged.
pub fn guard_bulk_rename_plan(
    database: &crate::Database,
    plan: &mut OrganisationPlan,
    save_locations: &RetroArchSaveLocations,
) -> usize {
    if !mode_changes_real_files(plan.mode) {
        return 0;
    }
    let mut sidecars = SidecarCache::default();
    let mut blocked = 0;
    for entry in &mut plan.entries {
        if entry.status != OrganisationStatus::Suggested
            || entry.destination_path == entry.source_path
        {
            continue;
        }
        if let Some(reason) = refusal_for(
            database,
            &entry.source_path,
            &plan.master_root,
            plan.generation,
            save_locations,
            &mut sidecars,
        ) {
            entry.status = OrganisationStatus::Blocked;
            entry.reason = Some(reason);
            blocked += 1;
        }
    }
    blocked
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
    if !mode_changes_real_files(plan.mode) {
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
fn refusal_for(
    database: &crate::Database,
    source: &Path,
    plan_root: &Path,
    generation: u64,
    save_locations: &RetroArchSaveLocations,
    sidecars: &mut SidecarCache,
) -> Option<String> {
    // The library folder that holds the file bounds the sidecar search, as
    // in the one-game Rename. A file outside every library folder (a Quick
    // Rename root that is not a library folder) uses the plan's root, or
    // only its own folder when it is outside that too.
    let source_root = owning_source_root(database, source)
        .ok()
        .or_else(|| {
            source
                .starts_with(plan_root)
                .then(|| plan_root.to_path_buf())
        })
        .or_else(|| source.parent().map(Path::to_path_buf))?;
    if let Err(reason) = refuse_arcade_set(database, source, &source_root, generation) {
        return Some(reason);
    }
    match sidecars.referencing(source, &source_root) {
        Err(reason) => return Some(reason),
        Ok(Some(referrer)) => {
            return Some(format!(
                "{} names this file, so renaming it alone would stop the game launching. Disc sets and cue files are not renamed in bulk yet. Nothing will change for this file.",
                referrer.display()
            ));
        }
        Ok(None) => {}
    }
    match find_retroarch_saves(source, save_locations) {
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
    /// one-game Rename).
    fn referencing(
        &mut self,
        source: &Path,
        source_root: &Path,
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
                    if let Some((path, _)) = found
                        .iter()
                        .find(|(path, text)| path != source && text.contains(&name))
                    {
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
