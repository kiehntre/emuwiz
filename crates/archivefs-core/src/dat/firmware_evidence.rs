//! Firmware/BIOS identity evidence extracted from an already-parsed DAT
//! catalogue.
//!
//! This module never fetches, downloads, or embeds anything: it only turns
//! a [`ParsedDat`] a caller already produced (via
//! [`crate::dat::parsers::parse_dat_file`], from a user-local file or a
//! future managed cache - this module never knows or cares which) into a
//! small, provider-neutral [`FirmwareIdentityRecord`] list. Verification
//! itself (hashing a local file and comparing) is a separate concern - see
//! `patch_manager::pcsx2_firmware` for the first (and, today, only)
//! consumer.
//!
//! # Why not embed the Redump PS2 BIOS hash table
//!
//! Redump's PS2 BIOS DAT redistribution license is unclear, so this crate
//! never bundles it. A caller who wants `Verified` PS2 BIOS evidence must
//! supply their own already-downloaded Redump DAT file, exactly the way
//! they already supply DAT files for game identity today.
//!
//! # Generic shape, narrow implementation
//!
//! [`FirmwareIdentityRecord`]/[`FirmwareSystem`] are deliberately shaped so
//! a later Saturn/Dreamcast firmware verifier could reuse the same record
//! type. [`redump_bios_evidence_from_dat`] extracts evidence for any of the
//! three systems Redump publishes a dedicated BIOS DAT for
//! ([`FirmwareSystem::PlayStation`], [`FirmwareSystem::PlayStation2`],
//! [`FirmwareSystem::Xbox`]); [`ps2_bios_evidence_from_dat`] is kept as a
//! thin, unchanged PS2-only wrapper so existing callers (PCSX2 BIOS
//! verification) never need to change. No generic "any system" extraction
//! framework beyond this exists - per this stage's own scope, only
//! *evidence extraction* for PS1/Xbox is implemented; no emulator
//! consumes it yet (see [`FirmwareSystem::Xbox`]'s own doc comment for the
//! Xbox-specific caveat).

use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use super::model::{DatEcosystem, DatGameEntry, DatSource, ParsedDat};
use crate::identity_source::hashing::Crc32;

/// A system whose firmware/BIOS this record describes.
///
/// [`Xbox`] deliberately names only the flash/kernel BIOS component: Redump
/// publishes a dedicated "Microsoft - Xbox - BIOS Images" DAT, but that
/// dataset covers the Xbox BIOS/flash image only. It says nothing about,
/// and must never be read as verifying, the MCPX boot ROM or EEPROM -
/// xemu's other two firmware components - which Redump does not publish
/// hashes for at all. A caller matching [`FirmwareSystem::Xbox`] evidence
/// must treat a match as "the BIOS/flash component is verified", never as
/// "xemu firmware is verified".
///
/// [`Xbox`]: FirmwareSystem::Xbox
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FirmwareSystem {
    PlayStation,
    PlayStation2,
    Xbox,
}

impl FirmwareSystem {
    /// The Redump BIOS dataset header text this system's DAT is expected to
    /// identify itself with, e.g. `"Sony - PlayStation - BIOS Images"` -
    /// used only for error messages and doc purposes; the actual match in
    /// [`header_identifies_redump_bios_dataset`] is a tolerant substring
    /// check, not an exact-string comparison.
    pub fn redump_dataset_label(self) -> &'static str {
        match self {
            Self::PlayStation => "Sony - PlayStation - BIOS Images",
            Self::PlayStation2 => "Sony - PlayStation 2 - BIOS Images",
            Self::Xbox => "Microsoft - Xbox - BIOS Images",
        }
    }
}

/// One authoritative firmware/BIOS dump record, as published by a DAT
/// catalogue - never hashed against anything itself, only carried as
/// reference evidence for a caller (`patch_manager::pcsx2_firmware`) to
/// match a local file against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareIdentityRecord {
    pub system: FirmwareSystem,
    /// Which catalogue ecosystem published this record - `Redump` for
    /// every record this module currently produces.
    pub provider: DatEcosystem,
    /// The DAT `<game name="...">` this record came from.
    pub name: String,
    /// The DAT `<game><description>`, when present.
    pub description: Option<String>,
    pub size_bytes: u64,
    /// Normalised lowercase hex - present because [`ps2_bios_evidence_from_dat`]
    /// only ever keeps records where every one of CRC32/MD5/SHA-1 parsed
    /// successfully (see that function's own doc comment for why an
    /// incomplete record is dropped rather than partially trusted).
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
    /// The publishing DAT's own `<header><version>`, when present -
    /// provenance for which exact catalogue revision authorized a match.
    pub dat_version: Option<String>,
}

/// Why a [`ParsedDat`] could not be treated as authoritative Redump BIOS
/// evidence for a given [`FirmwareSystem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirmwareEvidenceError {
    /// The DAT's own detected ecosystem is not [`DatEcosystem::Redump`] -
    /// an arbitrary DAT that merely happens to contain matching-looking
    /// hashes is never treated as authoritative.
    NotRedump,
    /// The DAT is a genuine Redump catalogue, but its header text does not
    /// identify it as the requested system's BIOS Images dataset
    /// specifically (e.g. it is that system's *games* DAT, or a different
    /// system's BIOS DAT).
    NotBiosDataset,
    /// The DAT matched every check above, but contained no game entry with
    /// a complete, usable ROM record (size plus all three of CRC32/MD5/
    /// SHA-1) - so there is nothing to verify against.
    NoUsableEntries,
}

impl std::fmt::Display for FirmwareEvidenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRedump => formatter.write_str(
                "the supplied DAT's detected ecosystem is not Redump, so it cannot authorize \
                 firmware/BIOS verification",
            ),
            Self::NotBiosDataset => formatter.write_str(
                "the supplied Redump DAT does not identify itself as the expected system's \
                 BIOS Images dataset",
            ),
            Self::NoUsableEntries => formatter.write_str(
                "the supplied DAT contained no game entry with a complete size/CRC32/MD5/SHA-1 \
                 ROM record",
            ),
        }
    }
}

impl std::error::Error for FirmwareEvidenceError {}

/// Whether `parsed`'s header text identifies it as Redump's own
/// `<system>` BIOS Images dataset - checked across every header field this
/// crate's own DAT model preserves, exactly the way
/// [`crate::dat::parsers::logiqx`]'s own ecosystem detection already
/// checks name/author/description/version rather than just one field.
///
/// Each system requires its own distinguishing substring plus "bios"
/// (case-insensitively, anywhere across those fields):
/// - [`FirmwareSystem::PlayStation2`] requires "playstation 2"/"playstation2"/"ps2".
/// - [`FirmwareSystem::PlayStation`] requires "playstation" but *not* also
///   "playstation 2"/"playstation2"/"ps2" - Redump's PS2 BIOS DAT header
///   text also contains the substring "playstation", so this exclusion is
///   what keeps a PS2 BIOS DAT from being misidentified as PS1 evidence.
/// - [`FirmwareSystem::Xbox`] requires "xbox" but *not* "360" - there is no
///   Redump Xbox 360 BIOS dataset, but this guards against ever accepting
///   one as if it were the original Xbox's.
pub fn header_identifies_redump_bios_dataset(source: &DatSource, system: FirmwareSystem) -> bool {
    let fields = [
        &source.name,
        &source.description,
        &source.author,
        &source.version,
    ];
    let joined: String = fields
        .iter()
        .filter_map(|field| field.as_deref())
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if !joined.contains("bios") {
        return false;
    }
    let mentions_ps2 = joined.contains("playstation 2")
        || joined.contains("playstation2")
        || joined.contains("ps2");
    match system {
        FirmwareSystem::PlayStation2 => mentions_ps2,
        FirmwareSystem::PlayStation => joined.contains("playstation") && !mentions_ps2,
        FirmwareSystem::Xbox => joined.contains("xbox") && !joined.contains("360"),
    }
}

/// A game entry's ROM records, kept only when every field this module
/// requires actually parsed - see [`FirmwareIdentityRecord::crc32`]'s own
/// doc comment for why an incomplete record is dropped, never partially
/// trusted.
fn firmware_records_from_game(
    game: &DatGameEntry,
    provider: DatEcosystem,
    system: FirmwareSystem,
    dat_version: Option<&str>,
) -> Vec<FirmwareIdentityRecord> {
    game.roms
        .iter()
        .filter_map(|rom| {
            let size_bytes = rom.size_bytes?;
            let crc32 = rom.crc32.clone()?;
            let md5 = rom.md5.clone()?;
            let sha1 = rom.sha1.clone()?;
            Some(FirmwareIdentityRecord {
                system,
                provider,
                name: game.name.clone(),
                description: game.description.clone(),
                size_bytes,
                crc32,
                md5,
                sha1,
                dat_version: dat_version.map(str::to_string),
            })
        })
        .collect()
}

/// Extracts authoritative Redump BIOS firmware evidence for `system` from an
/// already-parsed DAT catalogue.
///
/// Requires, in order:
/// 1. [`ParsedDat::source`]'s detected ecosystem is [`DatEcosystem::Redump`]
///    (never an arbitrary DAT that merely contains matching-looking
///    hashes).
/// 2. The header text identifies `system`'s BIOS Images dataset
///    specifically - see [`header_identifies_redump_bios_dataset`].
/// 3. At least one game entry contributes a complete (size + CRC32 + MD5 +
///    SHA-1) ROM record.
///
/// Never opens, hashes, or reads any BIOS file itself - this is DAT-text
/// interpretation only. For [`FirmwareSystem::Xbox`], a record produced
/// here describes only the BIOS/flash component - see that variant's own
/// doc comment for why it must never be read as verifying MCPX/EEPROM.
pub fn redump_bios_evidence_from_dat(
    parsed: &ParsedDat,
    system: FirmwareSystem,
) -> Result<Vec<FirmwareIdentityRecord>, FirmwareEvidenceError> {
    if parsed.source.ecosystem != DatEcosystem::Redump {
        return Err(FirmwareEvidenceError::NotRedump);
    }
    if !header_identifies_redump_bios_dataset(&parsed.source, system) {
        return Err(FirmwareEvidenceError::NotBiosDataset);
    }
    let records: Vec<FirmwareIdentityRecord> = parsed
        .games
        .iter()
        .flat_map(|game| {
            firmware_records_from_game(
                game,
                parsed.source.ecosystem,
                system,
                parsed.source.version.as_deref(),
            )
        })
        .collect();
    if records.is_empty() {
        return Err(FirmwareEvidenceError::NoUsableEntries);
    }
    Ok(records)
}

/// Extracts authoritative PS2 BIOS firmware evidence from an already-parsed
/// DAT catalogue. A thin, behavior-preserving wrapper over
/// [`redump_bios_evidence_from_dat`] fixed to [`FirmwareSystem::PlayStation2`] -
/// kept so existing callers (PCSX2 BIOS verification) never need to
/// change.
pub fn ps2_bios_evidence_from_dat(
    parsed: &ParsedDat,
) -> Result<Vec<FirmwareIdentityRecord>, FirmwareEvidenceError> {
    redump_bios_evidence_from_dat(parsed, FirmwareSystem::PlayStation2)
}

// ---------------------------------------------------------------------------
// Shared local-file hashing/matching primitives
// ---------------------------------------------------------------------------
//
// The remainder of this module is the one shared implementation of "hash a
// local firmware/BIOS file and compare it against `FirmwareIdentityRecord`
// evidence" - originally written once for PCSX2 BIOS verification
// (`patch_manager::pcsx2_firmware`) and extracted here so DuckStation BIOS
// verification (`patch_manager::duckstation_firmware`) reuses the exact
// same safe-open/streamed-hash/deterministic-match code rather than a
// second, parallel copy. Every emulator-specific module still owns its own
// selection policy (which file counts as "the configured BIOS") - only the
// hashing and matching are shared here.

/// One local file's computed size/CRC32/MD5/SHA-1 - the same four fields
/// every [`FirmwareIdentityRecord`] carries, so a caller can compare them
/// directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputedFirmwareDigests {
    pub size_bytes: u64,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
}

/// Safely opens (refusing a symlink via `O_NOFOLLOW` on Unix, and refusing
/// a non-regular file even where that flag is unavailable), then streams
/// `path` in `chunk_bytes`-sized reads to compute CRC32/MD5/SHA-1
/// simultaneously. Refuses a file above `max_bytes` before reading its
/// contents, and refuses a file whose size changed while being read.
pub fn hash_firmware_file(
    path: &Path,
    max_bytes: u64,
    chunk_bytes: usize,
) -> Result<ComputedFirmwareDigests, String> {
    use md5::Md5;
    use sha1::Sha1;
    use sha1::digest::Digest;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(path)
        .map_err(|error| format!("firmware file could not be opened safely: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("firmware file could not be inspected: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("firmware file changed identity before it could be safely hashed".to_string());
    }
    let size_bytes = metadata.len();
    if size_bytes > max_bytes {
        return Err(format!(
            "firmware file is {size_bytes} bytes, above the {max_bytes}-byte bound for this \
             kind of firmware dump"
        ));
    }

    let mut crc = Crc32::new();
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut buffer = vec![0_u8; chunk_bytes.max(1)];
    let mut total: u64 = 0;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("firmware file could not be read: {error}"))?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        crc.update(chunk);
        md5.update(chunk);
        sha1.update(chunk);
        total += read as u64;
    }
    if total != size_bytes {
        return Err("firmware file changed size while it was being read".to_string());
    }

    Ok(ComputedFirmwareDigests {
        size_bytes: total,
        crc32: crc.finish_hex(),
        md5: firmware_hex(&md5.finalize()),
        sha1: firmware_hex(&sha1.finalize()),
    })
}

fn firmware_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Every evidence record whose size/CRC32/MD5/SHA-1 all agree with
/// `digests`, sorted deterministically by `(name, description)` so that if
/// more than one record happens to be identical (the same physical dump
/// catalogued under more than one entry), the same one is always chosen.
pub fn matching_firmware_records<'a>(
    digests: &ComputedFirmwareDigests,
    evidence: &'a [FirmwareIdentityRecord],
) -> Vec<&'a FirmwareIdentityRecord> {
    let mut matches: Vec<&FirmwareIdentityRecord> = evidence
        .iter()
        .filter(|record| {
            record.size_bytes == digests.size_bytes
                && record.crc32.eq_ignore_ascii_case(&digests.crc32)
                && record.md5.eq_ignore_ascii_case(&digests.md5)
                && record.sha1.eq_ignore_ascii_case(&digests.sha1)
        })
        .collect();
    matches.sort_by(|left, right| {
        (&left.name, &left.description).cmp(&(&right.name, &right.description))
    });
    matches
}

#[cfg(test)]
mod tests;
