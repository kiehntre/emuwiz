//! Read-only ZIP previews with Apply temporarily blocked for preservation safety.
//!
//! Named staging and pathname publication do not establish ownership against
//! concurrent substitution. Both public Apply APIs refuse unconditionally before
//! inspecting paths or creating objects. Re-enablement requires independent
//! safety review; previews and unrelated archive workflows remain available.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipPreviewEntry {
    pub name: String,
    pub directory: bool,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipPreview {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub entries: Vec<ZipPreviewEntry>,
    pub total_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipOperationResult {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub files: usize,
    pub total_size: u64,
    pub verification: String,
}

type SourceEntry = (bool, PathBuf, u64, [u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZipError {
    /// Temporary preservation gate, returned before any Apply filesystem access.
    ApplyUnavailable,
    SourceMissing,
    SourceSymlink,
    UnsupportedSource,
    DestinationAlreadyExists,
    DestinationParentMissing,
    UnsafePath,
    DuplicatePath,
    SymlinkMember,
    InvalidName,
    CorruptZip,
    VerificationFailed(String),
    Io(String),
}

impl std::fmt::Display for ZipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplyUnavailable => write!(
                f,
                "ZIP Apply is temporarily unavailable for preservation safety: staging ownership and atomic destination no-clobber publication are not established. No output was created or extracted. Read-only preview remains available."
            ),
            Self::SourceMissing => write!(f, "The source could not be found."),
            Self::SourceSymlink => write!(f, "Symbolic-link sources are not supported."),
            Self::UnsupportedSource => write!(f, "Choose a regular file or folder."),
            Self::DestinationAlreadyExists => write!(f, "Destination already exists."),
            Self::DestinationParentMissing => write!(f, "The destination folder does not exist."),
            Self::UnsafePath => write!(f, "This ZIP contains an unsafe path."),
            Self::DuplicatePath => write!(f, "This ZIP contains a duplicate conflicting path."),
            Self::SymlinkMember => write!(f, "This ZIP contains an unsafe symbolic link."),
            Self::InvalidName => write!(f, "This ZIP contains an invalid name."),
            Self::CorruptZip => write!(f, "This ZIP is corrupt or could not be read."),
            Self::VerificationFailed(detail) => write!(f, "Verification failed: {detail}"),
            Self::Io(detail) => write!(f, "The archive operation could not finish: {detail}"),
        }
    }
}

impl std::error::Error for ZipError {}

fn io_error(error: io::Error) -> ZipError {
    ZipError::Io(error.to_string())
}

fn ensure_new_destination(destination: &Path) -> Result<(), ZipError> {
    if fs::symlink_metadata(destination).is_ok() {
        return Err(ZipError::DestinationAlreadyExists);
    }
    let Some(parent) = destination.parent() else {
        return Err(ZipError::DestinationParentMissing);
    };
    if !parent.is_dir() {
        return Err(ZipError::DestinationParentMissing);
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<PathBuf, ZipError> {
    let portable = name.replace('\\', "/");
    if portable.is_empty()
        || portable.len() > 4096
        || portable.starts_with('/')
        || portable.contains(':')
    {
        return Err(ZipError::UnsafePath);
    }
    let mut safe = PathBuf::new();
    for component in Path::new(&portable).components() {
        match component {
            Component::Normal(value) => {
                if value.is_empty() {
                    return Err(ZipError::InvalidName);
                }
                safe.push(value);
            }
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(ZipError::UnsafePath);
            }
        }
    }
    if safe.as_os_str().is_empty() {
        Err(ZipError::InvalidName)
    } else {
        Ok(safe)
    }
}

fn member_name(path: &Path) -> Result<String, ZipError> {
    let name = path
        .to_str()
        .ok_or(ZipError::InvalidName)?
        .replace('\\', "/");
    validate_name(&name)?;
    Ok(name)
}

fn digest_file(path: &Path) -> Result<(u64, [u8; 32]), ZipError> {
    let mut input = File::open(path).map_err(io_error)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| ZipError::VerificationFailed("file is too large".into()))?;
        hasher.update(&buffer[..count]);
    }
    Ok((size, hasher.finalize().into()))
}

fn collect_source(source: &Path) -> Result<BTreeMap<String, SourceEntry>, ZipError> {
    fn visit(
        root: &Path,
        current: &Path,
        result: &mut BTreeMap<String, SourceEntry>,
    ) -> Result<(), ZipError> {
        let mut children = fs::read_dir(current)
            .map_err(io_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io_error)?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let path = child.path();
            let metadata = fs::symlink_metadata(&path).map_err(io_error)?;
            if metadata.file_type().is_symlink() {
                return Err(ZipError::SourceSymlink);
            }
            let relative = path.strip_prefix(root).map_err(|_| ZipError::InvalidName)?;
            let name = member_name(relative)?;
            if metadata.is_dir() {
                result.insert(name, (true, path.clone(), 0, [0; 32]));
                visit(root, &path, result)?;
            } else if metadata.is_file() {
                let (size, hash) = digest_file(&path)?;
                result.insert(name, (false, path, size, hash));
            } else {
                return Err(ZipError::UnsupportedSource);
            }
        }
        Ok(())
    }
    let metadata = fs::symlink_metadata(source).map_err(|_| ZipError::SourceMissing)?;
    if metadata.file_type().is_symlink() {
        return Err(ZipError::SourceSymlink);
    }
    let mut result = BTreeMap::new();
    if metadata.is_file() {
        let name = member_name(Path::new(source.file_name().ok_or(ZipError::InvalidName)?))?;
        let (size, hash) = digest_file(source)?;
        result.insert(name, (false, source.to_path_buf(), size, hash));
    } else if metadata.is_dir() {
        let root_name = member_name(Path::new(source.file_name().ok_or(ZipError::InvalidName)?))?;
        result.insert(root_name.clone(), (true, source.to_path_buf(), 0, [0; 32]));
        let containing = source.parent().unwrap_or_else(|| Path::new("."));
        visit(containing, source, &mut result)?;
    } else {
        return Err(ZipError::UnsupportedSource);
    }
    Ok(result)
}

fn preview_from_manifest(
    source: &Path,
    destination: &Path,
    manifest: &BTreeMap<String, SourceEntry>,
) -> ZipPreview {
    let entries = manifest
        .iter()
        .map(|(name, (directory, _, size, _))| ZipPreviewEntry {
            name: name.clone(),
            directory: *directory,
            size: *size,
        })
        .collect();
    ZipPreview {
        source: source.to_path_buf(),
        destination: destination.to_path_buf(),
        entries,
        total_size: manifest.values().map(|(_, _, size, _)| *size).sum(),
    }
}

pub fn preview_compress(source: &Path, destination: &Path) -> Result<ZipPreview, ZipError> {
    ensure_new_destination(destination)?;
    let manifest = collect_source(source)?;
    Ok(preview_from_manifest(source, destination, &manifest))
}

/// Temporarily unavailable: refuses before inspecting source or destination.
/// The historical name is retained for API compatibility, not a success claim.
pub fn compress_verified(
    _source: &Path,
    _destination: &Path,
) -> Result<ZipOperationResult, ZipError> {
    Err(ZipError::ApplyUnavailable)
}

#[derive(Clone)]
struct ExtractEntry {
    name: String,
    directory: bool,
    size: u64,
    hash: [u8; 32],
}

fn read_extract_manifest(path: &Path) -> Result<Vec<ExtractEntry>, ZipError> {
    let mut archive = zip::ZipArchive::new(File::open(path).map_err(|_| ZipError::CorruptZip)?)
        .map_err(|_| ZipError::CorruptZip)?;
    let mut entries = Vec::new();
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).map_err(|_| ZipError::CorruptZip)?;
        if member
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(ZipError::SymlinkMember);
        }
        let name = validate_name(member.name())?.to_string_lossy().into_owned();
        if !names.insert(name.clone()) {
            return Err(ZipError::DuplicatePath);
        }
        let directory = member.is_dir();
        let (size, hash) = if directory {
            (0, [0; 32])
        } else {
            let mut hasher = Sha256::new();
            let mut size = 0u64;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = member.read(&mut buffer).map_err(|_| ZipError::CorruptZip)?;
                if count == 0 {
                    break;
                }
                size += count as u64;
                hasher.update(&buffer[..count]);
            }
            (size, hasher.finalize().into())
        };
        entries.push(ExtractEntry {
            name,
            directory,
            size,
            hash,
        });
    }
    for entry in &entries {
        let mut parent = Path::new(&entry.name).parent();
        while let Some(path) = parent.filter(|p| !p.as_os_str().is_empty()) {
            if entries
                .iter()
                .any(|other| other.name == path.to_string_lossy() && !other.directory)
            {
                return Err(ZipError::DuplicatePath);
            }
            parent = path.parent();
        }
    }
    Ok(entries)
}

pub fn preview_extract(source: &Path, destination: &Path) -> Result<ZipPreview, ZipError> {
    ensure_new_destination(destination)?;
    let entries = read_extract_manifest(source)?;
    let total_size = entries.iter().map(|entry| entry.size).sum();
    Ok(ZipPreview {
        source: source.to_path_buf(),
        destination: destination.to_path_buf(),
        entries: entries
            .iter()
            .map(|entry| ZipPreviewEntry {
                name: entry.name.clone(),
                directory: entry.directory,
                size: entry.size,
            })
            .collect(),
        total_size,
    })
}

/// Temporarily unavailable: refuses before inspecting source or destination.
/// No staging, publication, capability probe or abandoned-object cleanup runs.
pub fn extract_verified(
    _source: &Path,
    _destination: &Path,
) -> Result<ZipOperationResult, ZipError> {
    Err(ZipError::ApplyUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for (name, bytes) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    #[ignore = "Temporarily superseded by the P0 ZIP Apply safety gate; retain for independently reviewed re-enablement"]
    fn single_file_round_trip_is_hash_verified() {
        let root = tempdir().unwrap();
        let source = root.path().join("game.bin");
        let zip = root.path().join("game.zip");
        let output = root.path().join("out");
        fs::write(&source, b"verified bytes").unwrap();
        let preview = preview_compress(&source, &zip).unwrap();
        assert_eq!(preview.entries[0].name, "game.bin");
        compress_verified(&source, &zip).unwrap();
        extract_verified(&zip, &output).unwrap();
        assert_eq!(
            fs::read(&source).unwrap(),
            fs::read(output.join("game.bin")).unwrap()
        );
    }

    #[test]
    fn folder_preview_preserves_nested_and_empty_directories() {
        let root = tempdir().unwrap();
        let source = root.path().join("collection");
        fs::create_dir_all(source.join("nested/empty")).unwrap();
        fs::write(source.join("nested/game.bin"), b"game").unwrap();
        let preview = preview_compress(&source, &root.path().join("collection.zip")).unwrap();
        assert!(
            preview
                .entries
                .iter()
                .any(|entry| entry.name == "collection" && entry.directory)
        );
        assert!(
            preview
                .entries
                .iter()
                .any(|entry| entry.name == "collection/nested/empty" && entry.directory)
        );
    }

    #[test]
    fn traversal_and_absolute_members_are_rejected_without_output() {
        let root = tempdir().unwrap();
        for (name, expected) in [
            ("../escape", ZipError::UnsafePath),
            ("/escape", ZipError::UnsafePath),
        ] {
            let zip = root.path().join(format!("{}.zip", name.replace('/', "_")));
            write_zip(&zip, &[(name, b"bad")]);
            let result = preview_extract(&zip, &root.path().join(format!("out-{}", name.len())));
            assert_eq!(result.unwrap_err(), expected);
        }
    }

    #[test]
    fn destination_collision_stops_before_mutation() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let zip = root.path().join("source.zip");
        fs::write(&source, b"source").unwrap();
        fs::write(&zip, b"keep me").unwrap();
        assert_eq!(
            preview_compress(&source, &zip).unwrap_err(),
            ZipError::DestinationAlreadyExists
        );
        assert_eq!(fs::read(&zip).unwrap(), b"keep me");
    }

    #[test]
    fn corrupt_zip_and_duplicate_conflict_previews_create_nothing() {
        let root = tempdir().unwrap();
        let corrupt = root.path().join("corrupt.zip");
        fs::write(&corrupt, b"not a zip").unwrap();
        let output = root.path().join("output");
        assert_eq!(
            preview_extract(&corrupt, &output).unwrap_err(),
            ZipError::CorruptZip
        );
        assert!(!output.exists());

        let duplicate = root.path().join("duplicate.zip");
        let file = File::create(&duplicate).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("same", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"one").unwrap();
        writer
            .start_file("same/child.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"two").unwrap();

        writer.finish().unwrap();
        let output = root.path().join("duplicate-output");
        assert_eq!(
            preview_extract(&duplicate, &output).unwrap_err(),
            ZipError::DuplicatePath
        );
        assert!(!output.exists());
    }
}
