//! How a piece of content is stored, independent of what it is.
//!
//! Detection here is read-only: opening a zip/tar to list member *names*
//! never decompresses anything, and folder classification is one bounded,
//! non-recursive `read_dir`. Nothing in this file writes, renames, moves,
//! or extracts a file.

use super::content_registry::content_kind_for_extension;
use std::fs::File;
use std::path::{Path, PathBuf};

/// The archive format a [`ContainerKind::Archive`] wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    SevenZip,
    Rar,
    Tar,
}

impl ArchiveFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Zip => "ZIP archive",
            Self::SevenZip => "7-Zip archive",
            Self::Rar => "RAR archive",
            Self::Tar => "TAR archive",
        }
    }

    fn for_extension(extension: &str) -> Option<Self> {
        match extension {
            "zip" => Some(Self::Zip),
            "7z" => Some(Self::SevenZip),
            "rar" => Some(Self::Rar),
            "tar" => Some(Self::Tar),
            _ => None,
        }
    }
}

/// What role a folder plays in discovery. A folder is either a leaf game
/// item on its own (never recursed into further) or a plain container to
/// walk through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderRole {
    /// Contains at least one `.slave` file directly inside it - a WHDLoad
    /// installation.
    WhdloadInstall,
    /// Contains recognisable game content directly inside it (no wrapper),
    /// e.g. an already-extracted or emulator-ready game folder.
    ExtractedGame,
    /// Neither of the above - a plain directory to recurse into.
    Plain,
}

/// How a piece of content is stored on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerKind {
    Archive(ArchiveFormat),
    Folder(FolderRole),
    DirectFile,
}

impl ContainerKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Archive(format) => format.label(),
            Self::Folder(FolderRole::WhdloadInstall) => "WHDLoad folder",
            Self::Folder(FolderRole::ExtractedGame) => "Game folder",
            Self::Folder(FolderRole::Plain) => "Folder",
            Self::DirectFile => "File",
        }
    }
}

/// Classify a path's container by extension/file-type alone. Callers
/// decide separately whether to recurse (for `Folder(Plain)`) or to treat
/// the path as a single discovery item.
pub fn detect_container(path: &Path, is_dir: bool) -> ContainerKind {
    if is_dir {
        return ContainerKind::Folder(classify_folder(path));
    }
    let extension = extension_lowercase(path);
    match extension.as_deref().and_then(ArchiveFormat::for_extension) {
        Some(format) => ContainerKind::Archive(format),
        None => ContainerKind::DirectFile,
    }
}

/// Bounded, non-recursive directory listing to decide a folder's role.
/// Never reads file contents; only inspects names.
const MAX_FOLDER_PEEK_ENTRIES: usize = 4096;

fn classify_folder(path: &Path) -> FolderRole {
    let Ok(entries) = std::fs::read_dir(path) else {
        return FolderRole::Plain;
    };
    let mut saw_recognized_content = false;
    for (count, entry) in entries.enumerate() {
        if count >= MAX_FOLDER_PEEK_ENTRIES {
            break;
        }
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let entry_path = entry.path();
        if let Some(extension) = extension_lowercase(&entry_path) {
            if extension == "slave" {
                return FolderRole::WhdloadInstall;
            }
            if content_kind_for_extension(&extension).is_some() {
                saw_recognized_content = true;
            }
        }
    }
    if saw_recognized_content {
        FolderRole::ExtractedGame
    } else {
        FolderRole::Plain
    }
}

pub fn extension_lowercase(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
}

/// A member name found while listing an archive's entries. Never
/// decompressed - names only.
pub struct ArchiveEntryName(pub String);

/// The maximum number of member names read from one archive during
/// discovery, to bound work on pathological archives.
const MAX_LISTED_ENTRIES: usize = 50_000;

/// List entry names for the archive formats discovery can inspect cheaply
/// without a subprocess: ZIP and TAR are both pure-Rust, name-listing-only
/// reads here (ZIP via `by_index_raw`, never inflating a member; TAR via
/// its streaming header reader, never unpacking).
///
/// RAR and 7-Zip are recognised as containers (see [`ArchiveFormat`]) but
/// are not listed here: RAR requires shelling out to an external tool
/// (see [`crate::dat::archive::rar`]) and 7-Zip's reader is built around
/// full member verification, not name-only listing. Both are deliberately
/// out of scope for this pass - see the ingestion module docs. Discovery
/// still reports them as recognised archive containers; it just cannot
/// say what content is inside one yet.
pub fn list_archive_entry_names(
    path: &Path,
    format: ArchiveFormat,
) -> Option<Vec<ArchiveEntryName>> {
    match format {
        ArchiveFormat::Zip => list_zip_entry_names(path),
        ArchiveFormat::Tar => list_tar_entry_names(path),
        ArchiveFormat::Rar | ArchiveFormat::SevenZip => None,
    }
}

fn list_zip_entry_names(path: &Path) -> Option<Vec<ArchiveEntryName>> {
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut names = Vec::new();
    for index in 0..archive.len().min(MAX_LISTED_ENTRIES) {
        let entry = archive.by_index_raw(index).ok()?;
        names.push(ArchiveEntryName(entry.name().to_string()));
    }
    Some(names)
}

fn list_tar_entry_names(path: &Path) -> Option<Vec<ArchiveEntryName>> {
    let file = File::open(path).ok()?;
    let mut archive = tar::Archive::new(file);
    let mut names = Vec::new();
    for entry in archive.entries().ok()?.take(MAX_LISTED_ENTRIES) {
        let entry = entry.ok()?;
        let path = entry.path().ok()?;
        names.push(ArchiveEntryName(path.to_string_lossy().into_owned()));
    }
    Some(names)
}

/// A slave file (`.slave`, case-insensitive) found directly inside a
/// [`FolderRole::WhdloadInstall`] folder, bounded and read-only.
const MAX_SLAVE_FILES_PER_FOLDER: usize = 8;

pub fn find_slave_files(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries {
        if found.len() >= MAX_SLAVE_FILES_PER_FOLDER {
            break;
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if extension_lowercase(&path).as_deref() == Some("slave") {
            found.push(path);
        }
    }
    found
}
