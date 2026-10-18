//! Bounded, read-only Cemu key-store syntax diagnostics, never launch authority.
//! No values, comments, digests or decoded keys leave the temporary input buffer.
//! A syntactically accepted entry cannot establish applicability without testing
//! decryption, which this module deliberately never performs.

use super::{
    WiiUContentProjection, WiiUContentRepresentation as Form, WiiUEncryptionState as Encryption,
};
use crate::{
    launch::process_spawn::CapturedFileIdentity,
    safe_read::{TrustedRoots, open_bounded_read},
};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const MAX_STORE_BYTES: usize = 64 * 1024;
pub const MAX_LINE_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    NotRequired,
    DiscKeyRequired,
    EncryptedTitleUnsupported,
    Unknown,
}

pub fn requirement(content: &WiiUContentProjection) -> Requirement {
    if !content.structural_complete {
        return Requirement::Unknown;
    }
    match (content.representation, content.encryption_state) {
        (Form::LoadiineExtracted | Form::RpxExtractedGame, Encryption::Decrypted) => {
            Requirement::NotRequired
        }
        (Form::Wud | Form::Wux, Encryption::Encrypted) => Requirement::DiscKeyRequired,
        (Form::Nus, Encryption::Encrypted) => Requirement::EncryptedTitleUnsupported,
        _ => Requirement::Unknown,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreState {
    Missing,
    Empty,
    Malformed { line: usize },
    LineTooLong { line: usize },
    WellFormedUnverified { entries: usize },
    TooLarge,
    UnsafePath,
    Unreadable,
    Changed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreDiagnostic {
    pub path: PathBuf,
    pub state: StoreState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileDiagnostics {
    pub profile_id: String,
    pub locations: Vec<StoreDiagnostic>,
}

/// Existing EmuWiz profile/portable candidate locations, not a claim about
/// Cemu's version-dependent user-data directory. An explicitly selected store
/// can be checked separately; neither source establishes a runtime binding.
pub fn inspect_candidates(
    profile_id: &str,
    configuration_path: &Path,
    executable: Option<&Path>,
) -> ProfileDiagnostics {
    let mut paths = vec![configuration_path.join("keys.txt")];
    if let Some(parent) = executable.and_then(Path::parent) {
        let path = parent.join("keys.txt");
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    let locations = paths.iter().map(|path| inspect_store(path)).collect();
    ProfileDiagnostics {
        profile_id: profile_id.into(),
        locations,
    }
}

fn fingerprint(meta: &fs::Metadata) -> (CapturedFileIdentity, i64, i64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (
            CapturedFileIdentity::capture(meta),
            meta.ctime(),
            meta.ctime_nsec(),
        )
    }
    #[cfg(not(unix))]
    {
        (CapturedFileIdentity::capture(meta), 0, 0)
    }
}

/// Exact path only; no directory scan, create, symlink following or key decoding.
/// All failures are typed: OS error strings and input excerpts are never returned.
pub fn inspect_store(path: &Path) -> StoreDiagnostic {
    StoreDiagnostic {
        path: path.to_path_buf(),
        state: inspect(path),
    }
}

fn inspect(path: &Path) -> StoreState {
    if !path.is_absolute()
        || path.file_name().is_none_or(|n| n != "keys.txt")
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        || path.components().count() > 128
    {
        return StoreState::UnsafePath;
    }
    // Distinguish a genuine absent path from an unsafe/uninspectable ancestor.
    for parent in path.ancestors().skip(1) {
        match fs::symlink_metadata(parent) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            Ok(_) => return StoreState::UnsafePath,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return StoreState::Unreadable,
        }
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => m,
        Ok(_) => return StoreState::UnsafePath,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return StoreState::Missing,
        Err(_) => return StoreState::Unreadable,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return StoreState::UnsafePath;
        }
    }
    if metadata.len() > MAX_STORE_BYTES as u64 {
        return StoreState::TooLarge;
    }
    let Ok(safe) = open_bounded_read(path, &TrustedRoots::none()) else {
        return StoreState::Unreadable;
    };
    let mut file = safe.into_file();
    let before = fingerprint(&metadata);
    if file.metadata().ok().as_ref().map(fingerprint) != Some(before.clone()) {
        return StoreState::Changed;
    }
    let mut bytes = Vec::with_capacity(MAX_STORE_BYTES + 1);
    let read = Read::by_ref(&mut file)
        .take(MAX_STORE_BYTES as u64 + 1)
        .read_to_end(&mut bytes);
    let stable = file.metadata().ok().as_ref().map(fingerprint) == Some(before.clone())
        && fs::symlink_metadata(path).ok().as_ref().map(fingerprint) == Some(before)
        && bytes.len() as u64 == metadata.len();
    let state = if read.is_err() {
        StoreState::Unreadable
    } else if bytes.len() > MAX_STORE_BYTES {
        StoreState::TooLarge
    } else if !stable {
        StoreState::Changed
    } else {
        syntax(&bytes)
    };
    bytes.fill(0);
    std::hint::black_box(&bytes);
    state
}

// Cemu KeyCache.cpp grammar: strip #/; comments and spaces/tabs/-/_; then
// accept exactly 32 ASCII hex characters. Only count/line-number evidence is
// retained. No String, decoded key, hash or source line appears in a result.
fn syntax(bytes: &[u8]) -> StoreState {
    let mut entries = 0;
    for (index, raw) in bytes.split(|b| *b == b'\n').enumerate() {
        if raw.len() > MAX_LINE_BYTES {
            return StoreState::LineTooLong { line: index + 1 };
        }
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        let mut length = 0;
        for byte in line
            .iter()
            .copied()
            .take_while(|b| !matches!(b, b'#' | b';'))
        {
            if matches!(byte, b' ' | b'\t' | b'-' | b'_') {
                continue;
            }
            if !byte.is_ascii_hexdigit() {
                return StoreState::Malformed { line: index + 1 };
            }
            length += 1;
        }
        if length == 0 {
            continue;
        }
        if length != 32 {
            return StoreState::Malformed { line: index + 1 };
        }
        entries += 1;
    }
    if entries == 0 {
        StoreState::Empty
    } else {
        StoreState::WellFormedUnverified { entries }
    }
}

#[cfg(test)]
mod tests;
