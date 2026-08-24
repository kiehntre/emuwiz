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
//! a later PS1/Saturn/Dreamcast/Xbox firmware verifier could reuse the same
//! record type. Only [`ps2_bios_evidence_from_dat`] exists today - no
//! generic "any system" extraction framework, per this stage's own scope.

use super::model::{DatEcosystem, DatGameEntry, ParsedDat};

/// A system whose firmware/BIOS this record describes. Only [`PlayStation2`]
/// is produced anywhere in this codebase today; the enum exists so a later
/// PS1/Saturn/Dreamcast/Xbox firmware verifier can add its own variant
/// without reshaping [`FirmwareIdentityRecord`].
///
/// [`PlayStation2`]: FirmwareSystem::PlayStation2
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FirmwareSystem {
    PlayStation2,
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

/// Why a [`ParsedDat`] could not be treated as authoritative PS2 BIOS
/// evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirmwareEvidenceError {
    /// The DAT's own detected ecosystem is not [`DatEcosystem::Redump`] -
    /// an arbitrary DAT that merely happens to contain matching-looking
    /// hashes is never treated as authoritative.
    NotRedump,
    /// The DAT is a genuine Redump catalogue, but its header text does not
    /// identify it as the PS2 BIOS Images dataset specifically (e.g. it is
    /// Redump's PS2 *games* DAT, or a different system's BIOS DAT).
    NotPs2BiosDataset,
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
                 PS2 BIOS verification",
            ),
            Self::NotPs2BiosDataset => formatter.write_str(
                "the supplied Redump DAT does not identify itself as the PlayStation 2 BIOS \
                 Images dataset",
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
/// "Sony - PlayStation 2 - BIOS Images" dataset - checked across every
/// header field this crate's own DAT model preserves, exactly the way
/// [`crate::dat::parsers::logiqx`]'s own ecosystem detection already
/// checks name/author/description/version rather than just one field.
/// Requires both "playstation 2" and "bios" to appear (case-insensitively,
/// each anywhere across those fields) so Redump's separate PS2 *games* DAT
/// - which mentions "playstation 2" but never "bios" - is never mistaken
/// for this dataset.
fn header_identifies_ps2_bios_dataset(source: &super::model::DatSource) -> bool {
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
    let mentions_ps2 = joined.contains("playstation 2")
        || joined.contains("playstation2")
        || joined.contains("ps2");
    mentions_ps2 && joined.contains("bios")
}

/// A game entry's ROM records, kept only when every field this module
/// requires actually parsed - see [`FirmwareIdentityRecord::crc32`]'s own
/// doc comment for why an incomplete record is dropped, never partially
/// trusted.
fn firmware_records_from_game(
    game: &DatGameEntry,
    provider: DatEcosystem,
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
                system: FirmwareSystem::PlayStation2,
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

/// Extracts authoritative PS2 BIOS firmware evidence from an already-parsed
/// DAT catalogue.
///
/// Requires, in order:
/// 1. [`ParsedDat::source`]'s detected ecosystem is [`DatEcosystem::Redump`]
///    (never an arbitrary DAT that merely contains matching-looking
///    hashes).
/// 2. The header text identifies the PS2 BIOS Images dataset specifically
///    - see [`header_identifies_ps2_bios_dataset`].
/// 3. At least one game entry contributes a complete (size + CRC32 + MD5 +
///    SHA-1) ROM record.
///
/// Never opens, hashes, or reads any BIOS file itself - this is DAT-text
/// interpretation only.
pub fn ps2_bios_evidence_from_dat(
    parsed: &ParsedDat,
) -> Result<Vec<FirmwareIdentityRecord>, FirmwareEvidenceError> {
    if parsed.source.ecosystem != DatEcosystem::Redump {
        return Err(FirmwareEvidenceError::NotRedump);
    }
    if !header_identifies_ps2_bios_dataset(&parsed.source) {
        return Err(FirmwareEvidenceError::NotPs2BiosDataset);
    }
    let records: Vec<FirmwareIdentityRecord> = parsed
        .games
        .iter()
        .flat_map(|game| {
            firmware_records_from_game(
                game,
                parsed.source.ecosystem,
                parsed.source.version.as_deref(),
            )
        })
        .collect();
    if records.is_empty() {
        return Err(FirmwareEvidenceError::NoUsableEntries);
    }
    Ok(records)
}

#[cfg(test)]
mod tests;
