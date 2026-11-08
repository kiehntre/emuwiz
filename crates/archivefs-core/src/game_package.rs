//! Where one game ends: the boundary of a game *package* before anything
//! proposes to move it.
//!
//! A package is one of:
//!
//! - a **single file** (a cartridge ROM, a ZIP, a CHD, an ISO);
//! - a **file set**: a `.cue`, `.gdi` or `.m3u` descriptor plus every file it
//!   references (an `.m3u`'s `.cue`/`.gdi` discs bring their own tracks);
//! - a **game folder**: one directory that is the game (a PS3 disc folder
//!   with `PS3_GAME/PARAM.SFO`, or a folder the person picked).
//!
//! Boundaries come from the descriptors' own references and from validated
//! layout files, never from "every parent folder is one game" and never from
//! hashing large disc images. Anything that cannot be bounded exactly is
//! refused with a plain reason, and the source is not touched:
//! a descriptor naming a missing, unsafe or sub-folder file, a file another
//! descriptor also uses, a track that belongs to a set (choose the set), a
//! folder holding other library games, a system or library folder, links,
//! special files and mount points inside a folder.
//!
//! Read-only: this module only reads descriptors (bounded) and directory
//! metadata.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Descriptor extensions that define a file set.
const DESCRIPTORS: &[&str] = &["cue", "gdi", "m3u", "m3u8"];
/// Largest descriptor read (cue sheets and playlists are tiny).
const MAX_DESCRIPTOR_BYTES: u64 = 256 * 1024;
/// Largest game folder walked when capturing its layout.
pub const MAX_FOLDER_ENTRIES: usize = 200_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageShape {
    SingleFile,
    /// The descriptor is the lead; `members` are its other files, all in the
    /// descriptor's own folder.
    FileSet {
        members: Vec<PathBuf>,
    },
    GameFolder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamePackage {
    pub lead: PathBuf,
    pub shape: PackageShape,
}

impl GamePackage {
    /// Every path that moves, lead first.
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut all = vec![self.lead.clone()];
        if let PackageShape::FileSet { members } = &self.shape {
            all.extend(members.iter().cloned());
        }
        all
    }

    /// Plain description for a preview line.
    pub fn describe(&self) -> String {
        match &self.shape {
            PackageShape::SingleFile => "single file".into(),
            PackageShape::FileSet { members } => format!(
                "set of {} files ({} and {} track/disc file{})",
                members.len() + 1,
                self.lead
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                members.len(),
                if members.len() == 1 { "" } else { "s" }
            ),
            PackageShape::GameFolder => "whole game folder".into(),
        }
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_descriptor(path: &Path) -> bool {
    DESCRIPTORS.contains(&extension(path).as_str())
}

fn regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The files a descriptor references, strictly: every reference must resolve
/// to an existing regular file directly in the descriptor's folder.
fn descriptor_references(descriptor: &Path) -> Result<Vec<PathBuf>, String> {
    let meta = std::fs::symlink_metadata(descriptor).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() {
        return Err(format!("{} is not a regular file", name(descriptor)));
    }
    if meta.len() > MAX_DESCRIPTOR_BYTES {
        return Err(format!(
            "{} is too large to be a descriptor",
            name(descriptor)
        ));
    }
    let parent = descriptor.parent().ok_or("descriptor has no folder")?;
    let ext = extension(descriptor);
    let raw: Vec<PathBuf> = match ext.as_str() {
        "cue" => crate::ingestion::cue_bin::resolve_cue_all_files(descriptor)
            .map_err(|e| format!("{} cannot be read as a complete set: {e}", name(descriptor)))?,
        "gdi" => crate::ingestion::gdi::resolve_gdi_all_tracks(descriptor)
            .map_err(|e| format!("{} cannot be read as a complete set: {e}", name(descriptor)))?,
        _ => {
            let text = std::fs::read_to_string(descriptor)
                .map_err(|e| format!("{} cannot be read: {e}", name(descriptor)))?;
            let mut files = Vec::new();
            for reference in crate::platform_evidence_fusion::cue_m3u_parsing::parse_m3u_references(
                descriptor, &text,
            ) {
                let resolved = reference.resolved.ok_or_else(|| {
                    format!(
                        "{} names \"{}\", which is not a safe relative file name",
                        name(descriptor),
                        reference.raw
                    )
                })?;
                files.push(resolved);
            }
            files
        }
    };
    if raw.is_empty() {
        return Err(format!("{} names no files", name(descriptor)));
    }
    let canonical_parent = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
    let mut members = Vec::new();
    for file in raw {
        // The CUE/GDI resolvers return canonical paths; the M3U parser
        // returns the descriptor folder's spelling. Both must sit directly in
        // the descriptor's folder.
        let in_folder = if file.starts_with(&canonical_parent) {
            file.strip_prefix(&canonical_parent)
                .ok()
                .map(Path::to_path_buf)
        } else {
            file.strip_prefix(parent).ok().map(Path::to_path_buf)
        };
        let Some(relative) = in_folder else {
            return Err(format!(
                "{} names a file outside its own folder",
                name(descriptor)
            ));
        };
        if relative.components().count() != 1 {
            return Err(format!(
                "{} names \"{}\" in a sub-folder; sets that use sub-folders are not moved yet",
                name(descriptor),
                relative.display()
            ));
        }
        let member = parent.join(&relative);
        if !regular_file(&member) {
            return Err(format!(
                "{} names \"{}\", which is missing or not a plain file",
                name(descriptor),
                relative.display()
            ));
        }
        if !members.contains(&member) && member != descriptor {
            members.push(member);
        }
    }
    Ok(members)
}

/// Resolves package boundaries, caching each folder's descriptors so a large
/// folder is listed and its descriptors read once.
#[derive(Default)]
pub struct PackageResolver {
    /// Per folder: member file -> descriptors in that folder that use it.
    folders: BTreeMap<PathBuf, BTreeMap<PathBuf, Vec<PathBuf>>>,
}

impl PackageResolver {
    pub fn new() -> Self {
        Self::default()
    }

    fn users_in(&mut self, folder: &Path) -> &BTreeMap<PathBuf, Vec<PathBuf>> {
        self.folders.entry(folder.to_path_buf()).or_insert_with(|| {
            let mut users: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
            let Ok(entries) = std::fs::read_dir(folder) else {
                return users;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !is_descriptor(&path) || !regular_file(&path) {
                    continue;
                }
                // A descriptor that cannot be read cannot claim files; it is
                // refused on its own when someone tries to move it.
                if let Ok(members) = descriptor_references(&path) {
                    for member in members {
                        users.entry(member).or_default().push(path.clone());
                    }
                }
            }
            users
        })
    }

    /// The package `path` leads, or why it cannot move on its own.
    pub fn package_of(&mut self, path: &Path) -> Result<GamePackage, String> {
        let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if meta.file_type().is_dir() {
            return Ok(GamePackage {
                lead: path.to_path_buf(),
                shape: PackageShape::GameFolder,
            });
        }
        if !meta.file_type().is_file() {
            return Err(format!("{} is not a plain file or folder", name(path)));
        }
        let folder = path.parent().ok_or("file has no folder")?.to_path_buf();
        // Is this file itself part of someone else's set?
        if let Some(users) = self.users_in(&folder).get(path)
            && let Some(first) = users.first()
        {
            return Err(format!(
                "{} belongs to {}; choose that set instead so it moves complete",
                name(path),
                name(first)
            ));
        }
        if !is_descriptor(path) {
            return Ok(GamePackage {
                lead: path.to_path_buf(),
                shape: PackageShape::SingleFile,
            });
        }
        let direct = descriptor_references(path)?;
        let mut members = Vec::new();
        for member in &direct {
            members.push(member.clone());
            // An .m3u disc that is itself a .cue/.gdi brings its tracks.
            if is_descriptor(member) {
                for track in descriptor_references(member)? {
                    if !members.contains(&track) && track != path {
                        members.push(track);
                    }
                }
            }
        }
        // Shared files: every member may only be used by descriptors inside
        // this package.
        let inside: Vec<PathBuf> = std::iter::once(path.to_path_buf())
            .chain(members.iter().filter(|m| is_descriptor(m)).cloned())
            .collect();
        let users = self.users_in(&folder).clone();
        for member in &members {
            if let Some(other) = users
                .get(member)
                .and_then(|u| u.iter().find(|d| !inside.contains(d)))
            {
                return Err(format!(
                    "{} is also used by {}; shared files are never split",
                    name(member),
                    name(other)
                ));
            }
        }
        Ok(GamePackage {
            lead: path.to_path_buf(),
            shape: PackageShape::FileSet { members },
        })
    }
}

/// One entry of a game folder's layout. Renaming a folder on one filesystem
/// keeps every inode, size and time, so an identical layout after the move
/// proves nothing inside changed without reading any bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FolderEntry {
    pub relative: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ns: i128,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FolderLayout {
    pub device: u64,
    pub entries: Vec<FolderEntry>,
    pub total_bytes: u64,
}

/// Walks a game folder without following links. Refuses links, special
/// files, nested mount points and oversized trees.
pub fn capture_folder_layout(folder: &Path) -> Result<FolderLayout, String> {
    use std::os::unix::fs::MetadataExt;
    let root = std::fs::symlink_metadata(folder).map_err(|e| e.to_string())?;
    if !root.file_type().is_dir() {
        return Err(format!("{} is not a folder", name(folder)));
    }
    let device = root.dev();
    let mut entries = Vec::new();
    let mut total_bytes = 0u64;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let mut children: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{} cannot be read: {e}", dir.display()))?
            .flatten()
            .collect();
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let path = child.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            let kind = meta.file_type();
            if kind.is_symlink() {
                return Err(format!(
                    "{} contains a link ({}); folders with links are not moved",
                    name(folder),
                    path.display()
                ));
            }
            if !kind.is_dir() && !kind.is_file() {
                return Err(format!(
                    "{} contains a special file ({})",
                    name(folder),
                    path.display()
                ));
            }
            if meta.dev() != device {
                return Err(format!(
                    "{} contains another mounted filesystem ({})",
                    name(folder),
                    path.display()
                ));
            }
            if entries.len() >= MAX_FOLDER_ENTRIES {
                return Err(format!(
                    "{} has more than {MAX_FOLDER_ENTRIES} entries",
                    name(folder)
                ));
            }
            let relative = path
                .strip_prefix(folder)
                .map_err(|e| e.to_string())?
                .to_path_buf();
            let modified_ns =
                i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
            if kind.is_dir() {
                pending.push(path);
            } else {
                total_bytes = total_bytes.saturating_add(meta.len());
            }
            entries.push(FolderEntry {
                relative,
                is_dir: kind.is_dir(),
                size: if kind.is_dir() { 0 } else { meta.len() },
                modified_ns: if kind.is_dir() { 0 } else { modified_ns },
                inode: meta.ino(),
            });
        }
    }
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(FolderLayout {
        device,
        entries,
        total_bytes,
    })
}

/// A PS3 disc folder: `PS3_GAME/PARAM.SFO` parses as a PARAM.SFO with a
/// `TITLE_ID`, and `PS3_GAME/USRDIR/EBOOT.BIN` exists. Returns the title ID.
/// Reads only the PARAM.SFO (bounded).
pub fn ps3_game_folder(folder: &Path) -> Option<String> {
    let sfo = folder.join("PS3_GAME").join("PARAM.SFO");
    let meta = std::fs::symlink_metadata(&sfo).ok()?;
    if !meta.file_type().is_file() || meta.len() > 64 * 1024 {
        return None;
    }
    if !regular_file(&folder.join("PS3_GAME").join("USRDIR").join("EBOOT.BIN")) {
        return None;
    }
    let bytes = std::fs::read(&sfo).ok()?;
    let parsed = crate::param_sfo::parse_param_sfo(&bytes)?;
    let title = parsed.get_text("TITLE_ID")?.trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// A game folder found under a library root by its validated layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundFolder {
    pub path: PathBuf,
    pub platform: String,
    pub evidence: String,
}

/// Looks up to `max_depth` folders below each root for PS3 disc folders.
/// Read-only; does not descend into a found game folder.
pub fn find_game_folders(roots: &[PathBuf], max_depth: usize) -> Vec<FoundFolder> {
    let mut found = Vec::new();
    for root in roots {
        let mut pending = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if !kind.is_dir() {
                    continue;
                }
                let path = entry.path();
                if let Some(title_id) = ps3_game_folder(&path) {
                    found.push(FoundFolder {
                        path,
                        platform: "PS3".into(),
                        evidence: format!(
                            "PS3 disc layout: PS3_GAME/PARAM.SFO (title ID {title_id}) and PS3_GAME/USRDIR/EBOOT.BIN"
                        ),
                    });
                } else if depth + 1 < max_depth {
                    pending.push((path, depth + 1));
                }
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

#[cfg(test)]
pub(crate) mod tests;
