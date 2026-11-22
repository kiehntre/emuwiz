//! Reorganise existing links (owner policy 2026-11-22): an existing link is
//! named from its target game file, never from its own current name, so an
//! old unverified (for example RomM-derived) link name cannot survive a bulk
//! reorganise.
//!
//! The target is resolved from the link's own target text one step at a time,
//! starting from the link's real folder (or `/` for an absolute target). A
//! `..` step is allowed (a relative link such as `../games/x.iso` is fine):
//! every folder reached so far is a real folder, so going up one is exact.
//! Any link met on the way, including the final step, is refused (a link to
//! a link, a loop, or a folder that is a link could each be swapped to point
//! somewhere else). The result must be a regular file whose name can be used
//! as it is, inside one of the library's approved folders (the enabled,
//! registered source folders). Without approved folders every link is
//! refused: nothing is inferred or broadened.

use std::path::{Component, Path, PathBuf};

const BROKEN: &str =
    "the link is broken (its target game file does not exist); it is left as it is";
const THROUGH_A_LINK: &str = "the link's target is reached through another link (a link to a link, a linked folder, or a loop), so its game file is ambiguous; it is left as it is";

/// The game file an existing link points at, resolved as the module docs
/// describe but without the approved-folder check (used to look the game up
/// and to name the link; the planner adds [`existing_link_target`]'s check).
pub fn resolve_link_target(link: &Path) -> Result<PathBuf, String> {
    let metadata =
        std::fs::symlink_metadata(link).map_err(|_| "the link does not exist".to_string())?;
    if !metadata.file_type().is_symlink() {
        return Err("this is not a link".to_string());
    }
    let text = std::fs::read_link(link).map_err(|_| "the link cannot be read".to_string())?;
    let mut current = if text.is_absolute() {
        PathBuf::from("/")
    } else {
        let parent = link
            .parent()
            .ok_or_else(|| "the link has no folder".to_string())?;
        std::fs::canonicalize(parent)
            .map_err(|_| "the link's folder cannot be resolved".to_string())?
    };
    let parts: Vec<Component> = text.components().collect();
    let mut last_meta = None;
    for (index, part) in parts.iter().enumerate() {
        match part {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => {
                // `current` holds only real folders, so this is exact.
                current.pop();
                last_meta = None;
            }
            Component::Normal(name) => {
                current.push(name);
                let meta = std::fs::symlink_metadata(&current).map_err(|_| BROKEN.to_string())?;
                if meta.file_type().is_symlink() {
                    return Err(THROUGH_A_LINK.to_string());
                }
                let is_last = parts[index + 1..]
                    .iter()
                    .all(|rest| matches!(rest, Component::CurDir));
                if !is_last && !meta.is_dir() {
                    return Err(BROKEN.to_string());
                }
                last_meta = Some(meta);
            }
        }
    }
    match last_meta {
        Some(meta) if meta.is_file() => {}
        _ => {
            return Err(
                "the link does not point at a game file (it points at a folder or another kind of object); it is left as it is"
                    .to_string(),
            );
        }
    }
    // Defence in depth: the path walked is exactly the real path.
    match std::fs::canonicalize(&current) {
        Ok(real) if real == current => {}
        _ => return Err(THROUGH_A_LINK.to_string()),
    }
    match current.file_name().and_then(|name| name.to_str()) {
        Some(name) if super::plan::is_usable_link_basename(name) => Ok(current),
        _ => Err(
            "the link's target game file has no file name that can be used safely; it is left as it is"
                .to_string(),
        ),
    }
}

/// [`resolve_link_target`], and the game file must be inside one of
/// `approved_roots` (the library's enabled, registered source folders, each
/// a real folder). `None` or no usable root refuses every link.
pub fn existing_link_target(
    link: &Path,
    approved_roots: Option<&[PathBuf]>,
) -> Result<PathBuf, String> {
    let target = resolve_link_target(link)?;
    let roots: Vec<&PathBuf> = approved_roots
        .unwrap_or_default()
        .iter()
        // A root is used only as it is: a folder reached through a link is
        // not one (the scanner refuses those too).
        .filter(|root| std::fs::canonicalize(root).is_ok_and(|real| &real == *root))
        .collect();
    if roots.is_empty() {
        return Err(
            "the library's approved folders could not be read, so this link's game file cannot be checked; it is left as it is"
                .to_string(),
        );
    }
    if roots.iter().any(|root| target.starts_with(root)) {
        Ok(target)
    } else {
        Err(
            "the link's game file is outside the library's approved folders; it is left as it is"
                .to_string(),
        )
    }
}

/// A moved link keeps its exact target text, so a relative link moved to
/// `destination` must still reach the same game file from there. Accepted
/// only in the plain form `../../folder/game.ext` (any `..` steps first,
/// then folder names), whose meaning from the new folder is exact. Absolute
/// links always qualify. Anything else is refused and left as it is.
pub fn moved_link_still_resolves(link: &Path, destination: &Path) -> Result<(), String> {
    const ELSEWHERE: &str = "this relative link would point at a different file from its new folder (a moved link keeps its exact target text), so it is left as it is";
    let text = std::fs::read_link(link).map_err(|_| "the link cannot be read".to_string())?;
    if text.is_absolute() {
        return Ok(());
    }
    let target = resolve_link_target(link)?;
    let mut seen_name = false;
    for part in text.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if !seen_name => {}
            Component::Normal(_) => seen_name = true,
            _ => {
                return Err(
                    "this relative link's target text goes down and back up again, so its meaning from a new folder cannot be checked; it is left as it is"
                        .to_string(),
                );
            }
        }
    }
    // The new folder as it will really be: its nearest existing ancestor,
    // resolved, plus the folders Apply creates (real folders).
    let mut existing = destination
        .parent()
        .ok_or_else(|| ELSEWHERE.to_string())?
        .to_path_buf();
    let mut created = Vec::new();
    while std::fs::symlink_metadata(&existing).is_err() {
        created.push(
            existing
                .file_name()
                .ok_or_else(|| ELSEWHERE.to_string())?
                .to_os_string(),
        );
        existing = existing
            .parent()
            .ok_or_else(|| ELSEWHERE.to_string())?
            .to_path_buf();
    }
    let mut from = std::fs::canonicalize(&existing).map_err(|_| ELSEWHERE.to_string())?;
    from.extend(created.iter().rev());
    for part in text.components() {
        match part {
            Component::ParentDir => {
                from.pop();
            }
            Component::Normal(name) => from.push(name),
            _ => {}
        }
    }
    if from == target {
        Ok(())
    } else {
        Err(ELSEWHERE.to_string())
    }
}

/// The target game file's own name, used for an existing link with no
/// verified catalogue name.
pub fn existing_link_target_name(link: &Path) -> Result<String, String> {
    resolve_link_target(link).map(|target| {
        target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

/// The path whose library identity and catalogue name describe `source`: for
/// an existing link (reorganise existing links) its resolved target game
/// file, otherwise the source itself. A link whose target cannot be resolved
/// keeps its own path (the planner refuses it anyway).
pub fn identity_path(mode: super::OrganisationMode, source: &Path) -> PathBuf {
    match mode {
        super::OrganisationMode::OrganiseSymlinkOnly => {
            resolve_link_target(source).unwrap_or_else(|_| source.to_path_buf())
        }
        _ => source.to_path_buf(),
    }
}

/// The library's approved folders: the enabled, registered source folders.
pub fn approved_library_roots(database: &crate::Database) -> Result<Vec<PathBuf>, String> {
    crate::database::organisation_paths::enabled_source_roots(database)
}

/// Reorganise existing links needs its own destination folder (owner
/// decision 2026-11-22). A destination that is one of the library's game
/// folders, inside one, or holding one could put a link where a game file
/// already is, so it is refused before anything is planned; nothing is
/// redirected. Both sides are compared as real paths. Returns the
/// destination's real path.
pub fn separate_link_destination(
    destination: &Path,
    approved_roots: &[PathBuf],
) -> Result<PathBuf, String> {
    const CHOOSE: &str = "Choose a separate, empty folder for the reorganised links (not one of your game folders). Nothing was changed.";
    let real = std::fs::canonicalize(destination)
        .ok()
        .filter(|real| real.is_dir())
        .ok_or_else(|| {
            format!(
                "The folder for reorganised links, {}, does not exist or is not a folder. {CHOOSE}",
                destination.display()
            )
        })?;
    if approved_roots.is_empty() {
        return Err(format!(
            "The library's game folders could not be read, so the folder for reorganised links cannot be checked. {CHOOSE}"
        ));
    }
    for root in approved_roots {
        let Ok(root_real) = std::fs::canonicalize(root) else {
            return Err(format!(
                "The game folder {} could not be checked, so the folder for reorganised links cannot be confirmed as separate. {CHOOSE}",
                root.display()
            ));
        };
        let relation = if real == root_real {
            "is your game folder"
        } else if real.starts_with(&root_real) {
            "is inside your game folder"
        } else if root_real.starts_with(&real) {
            "holds your game folder"
        } else {
            continue;
        };
        return Err(format!(
            "The folder for reorganised links, {}, {relation} {}, so a link could be placed where a game file already is. {CHOOSE}",
            destination.display(),
            root.display()
        ));
    }
    Ok(real)
}

/// Why an occupied destination is a conflict for an existing link: the
/// object there is the very game file the link points at (its own game
/// already sits in the destination folder), or something else.
pub(crate) fn occupied_destination_reason(source: &Path, destination: &Path) -> &'static str {
    let own_game = resolve_link_target(source).is_ok_and(|target| {
        std::fs::canonicalize(destination).is_ok_and(|occupant| occupant == target)
    });
    if own_game {
        "this link's new place is its own game file (the game already sits in the destination folder); nothing is overwritten. Choose a separate folder for reorganised links"
    } else {
        "the destination already exists; nothing is overwritten"
    }
}
