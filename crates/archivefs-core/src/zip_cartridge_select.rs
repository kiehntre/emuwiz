//! The one selector for "a ZIP holding exactly one cartridge ROM".
//!
//! Extracted from zipped-cartridge identity (#333) so every consumer
//! — identity inspection, launch staging and the optional Hasheous lookup —
//! accepts and refuses exactly the same archives and picks exactly the same
//! member. It reads only the central directory: no member is decompressed
//! here. Selection and hashing stay separate; each consumer hashes or stages
//! the selected member with its own bounded reader.
//!
//! One safety fix from #333's copy: a member name with a leading `/` is now
//! refused, as its own refusal message always said (`enclosed_name` alone
//! accepts it as relative).
//!
//! Refusals (all of the archive, never "skip and continue"): more than
//! [`MAX_ARCHIVE_MEMBERS`] entries, an unreadable entry, any encrypted entry,
//! an empty, over-long, NUL, backslash, non-UTF-8, absolute or traversing
//! name, duplicate names (case-insensitive), any symbolic-link entry, no
//! supported ROM for the system, more than one, a ROM declaring more than
//! [`MAX_LOOSE_ROM_BYTES`], or a declared decompression ratio above
//! [`MAX_ZIP_CARTRIDGE_RATIO`].
use crate::game_identity::{
    IdentityPlatform, IdentityStatus, MAX_ARCHIVE_MEMBERS, MAX_LOOSE_ROM_BYTES, MAX_PATH_BYTES,
    supported_loose_rom_format,
};
use std::fs::File;
use std::path::Path;
use zip::ZipArchive;

#[cfg(test)]
mod tests;

/// Largest decompression ratio accepted for a zipped cartridge member.
/// Deflate cannot exceed roughly 1032:1, so this only constrains other
/// methods; the absolute [`MAX_LOOSE_ROM_BYTES`] bound is the real limit.
pub const MAX_ZIP_CARTRIDGE_RATIO: u64 = 2_048;

/// Systems whose games may be a single cartridge ROM in a ZIP.
pub fn is_loose_cartridge_platform(platform: IdentityPlatform) -> bool {
    matches!(
        platform,
        IdentityPlatform::MegaDrive
            | IdentityPlatform::Snes
            | IdentityPlatform::Nes
            | IdentityPlatform::GameBoy
            | IdentityPlatform::GameBoyColor
            | IdentityPlatform::GameBoyAdvance
            | IdentityPlatform::VirtualBoy
            | IdentityPlatform::PokemonMini
            | IdentityPlatform::N64
            | IdentityPlatform::Atari2600
            | IdentityPlatform::Atari5200
            | IdentityPlatform::Atari7800
            | IdentityPlatform::Atari8Bit
            | IdentityPlatform::AtariLynx
            | IdentityPlatform::AtariJaguar
            | IdentityPlatform::Ngp
            | IdentityPlatform::Ngpc
    )
}

/// The selected member, by central-directory index and exact stored name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipCartridgeMember {
    pub index: usize,
    pub name: Vec<u8>,
    /// The member's declared uncompressed size.
    pub declared: u64,
    /// The loose-ROM format the member's name was accepted as.
    pub format: &'static str,
}

/// Validates the whole ZIP directory and selects its single supported ROM
/// member. Shared by identity inspection, launch-time staging and Hasheous so
/// all apply exactly the same refusals (encryption, names, duplicates,
/// symlinks, ambiguity, size and ratio bounds).
pub fn select_zip_cartridge_member(
    archive: &mut ZipArchive<File>,
    platform: IdentityPlatform,
    members_seen: &mut usize,
) -> Result<ZipCartridgeMember, (IdentityStatus, String)> {
    if archive.len() > MAX_ARCHIVE_MEMBERS {
        *members_seen = MAX_ARCHIVE_MEMBERS;
        return Err((
            IdentityStatus::ResourceLimitReached,
            ("ZIP member limit reached before identity inspection").to_string(),
        ));
    }
    let mut seen_names = std::collections::HashSet::new();
    let mut candidates: Vec<(usize, Vec<u8>, u64, u64, &'static str)> = Vec::new();
    for index in 0..archive.len() {
        *members_seen += 1;
        let raw = match archive.by_index_raw(index) {
            Ok(raw) => raw,
            Err(error) => {
                return Err((IdentityStatus::Invalid, error.to_string()));
            }
        };
        if raw.encrypted() {
            return Err((
                IdentityStatus::Unsupported,
                ("encrypted ZIP entries are refused").to_string(),
            ));
        }
        let name_raw = raw.name_raw().to_vec();
        let name_ok = !name_raw.is_empty()
            && name_raw.len() <= MAX_PATH_BYTES
            && !name_raw.contains(&0)
            && !name_raw.contains(&b'\\')
            // `enclosed_name` treats a leading `/` as relative, so an
            // absolute name is refused explicitly (the one change from #333).
            && !name_raw.starts_with(b"/")
            && std::str::from_utf8(&name_raw).is_ok()
            && raw.enclosed_name().is_some();
        if !name_ok {
            return Err((
                IdentityStatus::Invalid,
                ("ZIP contains an absolute, traversing, over-long or non-UTF-8 member name")
                    .to_string(),
            ));
        }
        if !seen_names.insert(name_raw.to_ascii_lowercase()) {
            return Err((
                IdentityStatus::Ambiguous,
                ("ZIP contains duplicate member names; none was selected implicitly").to_string(),
            ));
        }
        if raw.is_symlink() {
            return Err((
                IdentityStatus::Unsupported,
                ("ZIP symbolic-link members are refused").to_string(),
            ));
        }
        if raw.is_dir() {
            continue;
        }
        let member_path = Path::new(std::str::from_utf8(&name_raw).unwrap_or_default());
        if let Some(format) = supported_loose_rom_format(member_path, platform) {
            candidates.push((index, name_raw, raw.size(), raw.compressed_size(), format));
        }
    }
    if candidates.is_empty() {
        return Err((
            IdentityStatus::Missing,
            ("ZIP contains no ROM member supported for this platform").to_string(),
        ));
    }
    if candidates.len() != 1 {
        return Err((
            IdentityStatus::Ambiguous,
            ("ZIP contains multiple candidate ROM members; none was selected implicitly")
                .to_string(),
        ));
    }
    let (index, member_name, declared, compressed, format) = candidates.remove(0);
    if declared > MAX_LOOSE_ROM_BYTES {
        return Err((
            IdentityStatus::ResourceLimitReached,
            format!(
                "ROM member declares {declared} bytes; maximum supported size is {MAX_LOOSE_ROM_BYTES} bytes"
            ),
        ));
    }
    if declared > compressed.saturating_mul(MAX_ZIP_CARTRIDGE_RATIO).max(4096) {
        return Err((
            IdentityStatus::ResourceLimitReached,
            ("ROM member decompression ratio exceeds the safety limit").to_string(),
        ));
    }
    Ok(ZipCartridgeMember {
        index,
        name: member_name,
        declared,
        format,
    })
}
