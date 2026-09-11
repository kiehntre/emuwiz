use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::model::{BootstrapContext, BootstrapError};

pub const MAX_HASH_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_MARKER_BYTES: u64 = 64 * 1024;

pub fn safe_path(path: &Path) -> Result<(), BootstrapError> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || path
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .any(|byte| *byte == 0 || *byte < 32)
    {
        return Err(BootstrapError::UnsafePath(path.display().to_string()));
    }
    for ancestor in path
        .ancestors()
        .filter(|candidate| candidate.parent().is_some())
    {
        if let Ok(metadata) = fs::symlink_metadata(ancestor) {
            if metadata.file_type().is_symlink() || (!metadata.is_dir() && ancestor != path) {
                return Err(BootstrapError::UnsafePath(ancestor.display().to_string()));
            }
        }
    }
    Ok(())
}

pub fn file_hash(path: &Path, limit: u64) -> Result<Option<String>, BootstrapError> {
    safe_path(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(BootstrapError::Policy(error.to_string())),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
        return Err(BootstrapError::UnsafePath(path.display().to_string()));
    }
    let mut file = File::open(path).map_err(|error| BootstrapError::Policy(error.to_string()))?;
    let mut hash = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| BootstrapError::Policy(error.to_string()))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > limit {
            return Err(BootstrapError::UnsafePath(path.display().to_string()));
        }
        hash.update(&buffer[..read]);
    }
    Ok(Some(
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}

pub fn validate_context(context: &BootstrapContext) -> Result<(), BootstrapError> {
    safe_path(&context.root)?;
    safe_path(&context.destination)?;
    if let Some(executable) = &context.executable {
        safe_path(executable)?;
    }
    for path in context.filesystem.keys() {
        safe_path(path)?;
    }
    if context.host.is_empty() || context.arch.is_empty() {
        return Err(BootstrapError::Policy("host identity is incomplete".into()));
    }
    Ok(())
}

pub fn marker_bytes<R: Read>(reader: R) -> Result<Vec<u8>, BootstrapError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_MARKER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| BootstrapError::Policy(error.to_string()))?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err(BootstrapError::Policy(
            "provenance marker exceeds its bound".into(),
        ));
    }
    Ok(bytes)
}

pub fn validate_marker_binding(
    bytes: &[u8],
    target: &str,
    release: &str,
    digest: &str,
    source: &str,
) -> Result<(), BootstrapError> {
    let marker = std::str::from_utf8(bytes)
        .map_err(|_| BootstrapError::Policy("provenance marker is not UTF-8".into()))?;
    for (key, expected) in [
        ("target", target),
        ("release", release),
        ("digest", digest),
        ("source", source),
    ] {
        let actual = marker
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .ok_or_else(|| BootstrapError::Policy(format!("provenance marker lacks {key}")))?;
        if actual != expected {
            return Err(BootstrapError::Policy(format!(
                "provenance marker {key} does not match reviewed state"
            )));
        }
    }
    Ok(())
}

pub fn validate_appimage_header(path: &Path, minimum_size: u64) -> Result<(), BootstrapError> {
    let metadata = fs::metadata(path).map_err(|error| BootstrapError::Policy(error.to_string()))?;
    if metadata.len() < minimum_size {
        return Err(BootstrapError::Policy(
            "AppImage is below the minimum size".into(),
        ));
    }
    let mut header = [0_u8; 20];
    File::open(path)
        .map_err(|error| BootstrapError::Policy(error.to_string()))?
        .read_exact(&mut header)
        .map_err(|error| BootstrapError::Policy(error.to_string()))?;
    if &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || &header[8..11] != b"AI\x02"
        || u16::from_le_bytes([header[18], header[19]]) != 62
    {
        return Err(BootstrapError::Policy(
            "not a Linux x86-64 type-2 AppImage".into(),
        ));
    }
    Ok(())
}
