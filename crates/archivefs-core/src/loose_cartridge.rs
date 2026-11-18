//! Loose `.bin` cartridge ROMs in a system's own folder.
//!
//! `.bin` is shared by most cartridge systems and by countless non-game
//! files, so on its own it is only ever a Mega Drive corroboration candidate.
//! A `.bin` sitting directly in a folder named for one of [`PLATFORMS`] is
//! catalogued when its size (and, for Atari 8-bit, its `CART` header) fits
//! that system's cartridge dumps. Files nested deeper (game data, arcade chip
//! sets, PS3/ScummVM internals) are never considered.
//!
//! Many such folders also hold the same ROM zipped. A loose file whose bytes
//! are proven identical (size, CRC-32 and a full byte comparison) to the only
//! file of a sibling `.zip` is an alternate copy of that catalogued ZIP: it is
//! reported, not catalogued a second time. Neither file is touched. A loose
//! file that merely shares a name with a ZIP's ROM but differs in content is
//! catalogued as its own entry and reported as a possible variant. Nothing
//! here is ever merged, renamed or deleted.

use crate::identity_source::hashing::Crc32;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use zip::ZipArchive;

/// Systems whose loose `.bin` cartridges are recognised from folder evidence.
pub const PLATFORMS: [&str; 5] = [
    "Atari2600",
    "Atari 8-bit",
    "AmstradGX4000",
    "CasioLoopy",
    "Videopac",
];

/// Largest loose cartridge considered (and largest ZIP member compared).
const MAX_CARTRIDGE_BYTES: u64 = 8 * 1024 * 1024;
/// Members a sibling ZIP may list before it is ignored for comparison.
const MAX_ZIP_MEMBERS: usize = 64;
/// Folders whose ZIP index is kept between files of one scan.
const MAX_CACHED_FOLDERS: usize = 64;

/// What a loose `.bin` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LooseCartridge {
    /// Not a loose cartridge of a supported system (left to other rules).
    NotCartridge,
    /// A cartridge with no catalogued counterpart: catalogue it.
    New { platform: &'static str },
    /// A cartridge whose name matches a sibling ZIP's ROM but whose bytes
    /// differ: catalogue it and report it as a possible variant.
    PossibleVariant {
        platform: &'static str,
        counterpart: PathBuf,
    },
    /// Byte-identical to the only file of a sibling ZIP: an alternate copy,
    /// reported and not catalogued again.
    IdenticalCopy {
        platform: &'static str,
        zip: PathBuf,
    },
}

impl LooseCartridge {
    /// The system, when this is a cartridge at all.
    pub fn platform(&self) -> Option<&'static str> {
        match self {
            Self::NotCartridge => None,
            Self::New { platform }
            | Self::PossibleVariant { platform, .. }
            | Self::IdenticalCopy { platform, .. } => Some(platform),
        }
    }

    /// Whether the scan catalogues the file as its own entry.
    pub fn is_catalogued(&self) -> bool {
        matches!(self, Self::New { .. } | Self::PossibleVariant { .. })
    }
}

/// The system a `.bin` names by the folder directly holding it, when that
/// system is one of [`PLATFORMS`]. Name-only: no file is read.
pub fn candidate_platform(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?;
    if !extension.eq_ignore_ascii_case("bin") {
        return None;
    }
    let folder = path.parent()?.file_name()?.to_str()?;
    let platform = crate::folder_platform_alias(folder)?;
    PLATFORMS.contains(&platform).then_some(platform)
}

/// Whether `size` bytes starting with `header` fit `platform`'s cartridge
/// dumps. Deliberately strict: an odd size is left unrecognised.
pub fn plausible_dump(platform: &str, size: u64, header: &[u8]) -> bool {
    const KIB: u64 = 1024;
    match platform {
        // 2 KiB banks, or Supercharger 8448-byte loads.
        "Atari2600" => {
            (2 * KIB..=512 * KIB).contains(&size) && (size % (2 * KIB) == 0 || size % 8448 == 0)
        }
        // A `CART` header (16 bytes) over 4 KiB multiples, or a raw 8 KiB
        // multiple.
        "Atari 8-bit" => {
            if header.starts_with(b"CART") {
                size > 16 && (size - 16) % (4 * KIB) == 0 && size - 16 <= 1024 * KIB
            } else {
                (8 * KIB..=1024 * KIB).contains(&size) && size % (8 * KIB) == 0
            }
        }
        "AmstradGX4000" => (16 * KIB..=512 * KIB).contains(&size) && size % (16 * KIB) == 0,
        "CasioLoopy" => {
            (512 * KIB..=MAX_CARTRIDGE_BYTES).contains(&size) && size % (512 * KIB) == 0
        }
        "Videopac" => matches!(size, 2048 | 4096 | 8192 | 12288 | 16384),
        _ => false,
    }
}

/// Classifies a loose `.bin`. Reads only the file itself and the central
/// directories of sibling ZIPs (plus one member when CRC and size match).
pub fn classify(path: &Path) -> LooseCartridge {
    let Some(platform) = candidate_platform(path) else {
        return LooseCartridge::NotCartridge;
    };
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return LooseCartridge::NotCartridge;
    };
    if !metadata.is_file() || metadata.len() > MAX_CARTRIDGE_BYTES {
        return LooseCartridge::NotCartridge;
    }
    let Ok(bytes) = read_bounded(path, MAX_CARTRIDGE_BYTES) else {
        return LooseCartridge::NotCartridge;
    };
    if !plausible_dump(platform, bytes.len() as u64, &bytes) {
        return LooseCartridge::NotCartridge;
    }
    let Some(folder) = path.parent() else {
        return LooseCartridge::New { platform };
    };
    let mut crc = Crc32::new();
    crc.update(&bytes);
    let crc = crc.finish();
    let index = zip_index(folder);
    for member in index.iter() {
        if member.only_member
            && member.size == bytes.len() as u64
            && member.crc == crc
            && member_bytes_equal(&member.zip, &bytes)
        {
            return LooseCartridge::IdenticalCopy {
                platform,
                zip: member.zip.clone(),
            };
        }
    }
    // Same game name, ignoring region/revision tags, case and punctuation
    // (`Asterix (PAL).bin` ~ `Asterix.zip`), but different bytes.
    if let Some(own) = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(title_key)
        && let Some(member) = index.iter().find(|member| {
            title_key(&member.name).as_deref() == Some(own.as_str())
                || member
                    .zip
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(title_key)
                    .as_deref()
                    == Some(own.as_str())
        })
    {
        return LooseCartridge::PossibleVariant {
            platform,
            counterpart: member.zip.clone(),
        };
    }
    LooseCartridge::New { platform }
}

/// A file name's title for possible-variant matching: the stem without any
/// `(...)`/`[...]` tags, lowercased, letters and digits only. `None` when
/// nothing is left.
pub fn title_key(file_name: &str) -> Option<String> {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let stem = Path::new(base)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(base);
    let mut key = String::new();
    let mut depth = 0usize;
    for ch in stem.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 && ch.is_alphanumeric() => key.extend(ch.to_lowercase()),
            _ => {}
        }
    }
    (!key.is_empty()).then_some(key)
}

#[derive(Debug)]
struct ZipMember {
    zip: PathBuf,
    name: String,
    size: u64,
    crc: u32,
    /// The ZIP holds exactly this one file (directories aside).
    only_member: bool,
}

type ZipIndex = Arc<Vec<ZipMember>>;

fn zip_cache() -> &'static Mutex<HashMap<PathBuf, (Option<SystemTime>, ZipIndex)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Option<SystemTime>, ZipIndex)>>> =
        OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The members of every ZIP directly in `folder`, read from central
/// directories only. Cached per folder while its modification time is
/// unchanged; a positive match is always re-proven against the bytes.
fn zip_index(folder: &Path) -> ZipIndex {
    let mtime = fs::metadata(folder).and_then(|meta| meta.modified()).ok();
    if let Ok(cache) = zip_cache().lock()
        && let Some((cached_mtime, index)) = cache.get(folder)
        && *cached_mtime == mtime
        && mtime.is_some()
    {
        return Arc::clone(index);
    }
    let mut members = Vec::new();
    if let Ok(entries) = fs::read_dir(folder) {
        let mut zips: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
            })
            .collect();
        zips.sort();
        for zip in zips {
            members.extend(read_zip_members(&zip));
        }
    }
    let index = Arc::new(members);
    if let Ok(mut cache) = zip_cache().lock() {
        if cache.len() >= MAX_CACHED_FOLDERS {
            cache.clear();
        }
        cache.insert(folder.to_path_buf(), (mtime, Arc::clone(&index)));
    }
    index
}

fn read_zip_members(zip: &Path) -> Vec<ZipMember> {
    let Ok(file) = File::open(zip) else {
        return Vec::new();
    };
    let Ok(mut archive) = ZipArchive::new(file) else {
        return Vec::new();
    };
    if archive.len() > MAX_ZIP_MEMBERS {
        return Vec::new();
    }
    let mut found = Vec::new();
    for index in 0..archive.len() {
        let Ok(member) = archive.by_index_raw(index) else {
            return Vec::new();
        };
        if member.is_dir() {
            continue;
        }
        if member.encrypted() {
            return Vec::new();
        }
        let Ok(name) = member.name() else {
            return Vec::new();
        };
        let name = name.to_string();
        found.push(ZipMember {
            zip: zip.to_path_buf(),
            name,
            size: member.size(),
            crc: member.crc32(),
            only_member: false,
        });
    }
    if found.len() == 1 {
        found[0].only_member = true;
    }
    found
}

/// Whether the ZIP's only file decompresses to exactly `expected`.
fn member_bytes_equal(zip: &Path, expected: &[u8]) -> bool {
    let Ok(file) = File::open(zip) else {
        return false;
    };
    let Ok(mut archive) = ZipArchive::new(file) else {
        return false;
    };
    let Some(index) = (0..archive.len()).find(|&index| {
        archive
            .by_index_raw(index)
            .is_ok_and(|member| !member.is_dir())
    }) else {
        return false;
    };
    let Ok(member) = archive.by_index(index) else {
        return false;
    };
    let mut actual = Vec::with_capacity(expected.len());
    if member
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut actual)
        .is_err()
    {
        return false;
    }
    actual == expected
}

fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other("too large for a cartridge"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
