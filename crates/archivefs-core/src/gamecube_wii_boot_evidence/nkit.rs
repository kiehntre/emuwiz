//! Read-only NKit v1 header evidence; never a reconstruction or apply token.
//!
//! Layout verified against Nanook/NKitv1 at 31ac768 (NStream and both
//! NkitReader implementations). See NKIT_PRESERVATION_AND_RECOVERY.md.
//! A recognized header proves neither a complete body nor recoverability.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::identity_source::hashing::Crc32;
use crate::safe_read::{TrustedRoots, open_bounded_read};

const HEADER_LEN: usize = 0x440;
const GC_SIZE: u64 = 0x57058000;
const WII_SIZE: u64 = 0x1fb4e0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NkitPlatform {
    GameCube,
    Wii,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NkitInspectionError {
    UnsafeOrUnreadable(String),
    NotNkit,
    Truncated,
    Malformed(&'static str),
    Unsupported(&'static str),
}

impl std::fmt::Display for NkitInspectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NKit inspection: {self:?}")
    }
}

impl std::error::Error for NkitInspectionError {}

/// Claimed fields from an unverified v1 raw image. Raw text survives exactly;
/// title bytes are not assumed to be UTF-8. Region is the numeric BI2/Wii word,
/// not a region inferred from the game ID or a filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NkitHeaderObservation {
    pub platform: NkitPlatform,
    pub stored_size: u64,
    pub claimed_original_size: u64,
    pub game_id: [u8; 6],
    pub disc_number: u8,
    pub revision: u8,
    pub title: [u8; 64],
    pub region_word: u32,
    pub claimed_original_crc32: u32,
    pub crc_forcing_patch: u32,
    pub forced_junk_id: [u8; 4],
    /// CRC of the removed partition/filler span, not necessarily the stored
    /// recovery file: upstream can omit trailing reconstructible filler.
    /// None means no such requirement declared, NOT all recovery data present.
    pub removed_update_crc32: Option<u32>,
    /// Hash of the inspected 0x440-byte prefix only, NOT source identity.
    pub header_sha256: [u8; 32],
}

fn be32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn open(path: &Path) -> Result<crate::safe_read::SafeFile, NkitInspectionError> {
    open_bounded_read(path, &TrustedRoots::none())
        .map_err(|error| NkitInspectionError::UnsafeOrUnreadable(error.detail()))
}

/// Reads at most 0x448 bytes through the existing safe-read policy. Refuses
/// symlinks (including ancestors) and special files. GCZ cannot be identified
/// as NKit without decoding its inner header, so it is explicitly unsupported.
/// Body truncation/corruption beyond the inspected ranges remains unchecked.
pub fn inspect(path: &Path) -> Result<NkitHeaderObservation, NkitInspectionError> {
    use NkitInspectionError as Error;
    let mut source = open(path)?;
    let magic = source.read_exact_at(0, 4, 4).ok_or(Error::Truncated)?;
    if magic == [0x01, 0xc0, 0x0b, 0xb1] {
        return Err(Error::Unsupported(
            "GCZ wrapper; inner NKit variant not inspected",
        ));
    }
    let header = source
        .read_exact_at(0, HEADER_LEN, HEADER_LEN)
        .ok_or(Error::Truncated)?;
    if &header[0x200..0x204] != b"NKIT" {
        return Err(Error::NotNkit);
    }
    if &header[0x204..0x208] != b" v01" {
        return Err(Error::Unsupported("NKit version"));
    }
    let gc = be32(&header, 0x1c) == 0xc2339f3d;
    let wii = be32(&header, 0x18) == 0x5d1c9ea3;
    let platform = match (gc, wii) {
        (true, false) => NkitPlatform::GameCube,
        (false, true) => NkitPlatform::Wii,
        _ => return Err(Error::Malformed("missing or conflicting disc magic")),
    };
    let (minimum, maximum, multiplier, region_offset) = match platform {
        NkitPlatform::GameCube => (0x2440, GC_SIZE, 1, 0x458),
        NkitPlatform::Wii => (0x50000, WII_SIZE, 4, 0x4e000),
    };
    let original_size = u64::from(be32(&header, 0x210)) * multiplier;
    if original_size < minimum {
        return Err(Error::Malformed("impossible original length"));
    }
    if original_size > maximum {
        return Err(Error::Unsupported(
            "original length exceeds supported disc geometry",
        ));
    }
    if source.len() < minimum {
        return Err(Error::Truncated);
    }
    let update_crc = be32(&header, 0x218);
    if gc && update_crc != 0 {
        return Err(Error::Malformed("Wii recovery marker on GameCube image"));
    }
    if wii && header[0x60..0x62] != [1, 1] {
        return Err(Error::Unsupported("Wii v1 encryption/hash flags"));
    }
    let region = source
        .read_exact_at(region_offset, 4, 4)
        .ok_or(Error::Truncated)?;
    Ok(NkitHeaderObservation {
        platform,
        stored_size: source.len(),
        claimed_original_size: original_size,
        game_id: header[..6].try_into().unwrap(),
        disc_number: header[6],
        revision: header[7],
        title: header[0x20..0x60].try_into().unwrap(),
        region_word: be32(&region, 0),
        claimed_original_crc32: be32(&header, 0x208),
        crc_forcing_patch: be32(&header, 0x20c),
        forced_junk_id: header[0x214..0x218].try_into().unwrap(),
        removed_update_crc32: (update_crc != 0).then_some(update_crc),
        header_sha256: Sha256::digest(&header).into(),
    })
}

/// Only the states this header-only foundation can actually establish.
/// Stronger states require a verified reconstructed stream, not a header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NkitRecoverability {
    Unknown,
    DependenciesMissing,
}

/// What the bounded header inspection establishes about representation.
/// Recognizing an NKit v1 header does not validate the rest of the image or
/// prove that its original disc can be reconstructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NkitRepresentationState {
    NkitV1HeaderRecognizedBodyUnverified,
}

/// Explicit, untrusted lookup metadata from the user's recovery inventory.
/// Agreement with the source requirement does not validate the object.
#[derive(Debug, Clone, Copy)]
pub struct UpdateRecoveryCandidate<'a> {
    pub path: &'a Path,
    pub declared_recovery_span_crc32: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRecoveryEvidence {
    NotDeclared,
    Missing {
        expected_crc32: u32,
    },
    /// File hashes cover stored bytes. The expected CRC covers a potentially
    /// larger reconstructed span; comparing these different domains is wrong.
    /// Structure, content compatibility and missing filler remain unchecked.
    Candidate {
        path: PathBuf,
        size: u64,
        measured_crc32: u32,
        measured_sha256: [u8; 32],
        expected_crc32: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NkitRecoveryPreview {
    pub source: PathBuf,
    pub header: NkitHeaderObservation,
    pub update_recovery: UpdateRecoveryEvidence,
    pub representation: NkitRepresentationState,
    pub recoverability: NkitRecoverability,
    pub blockers: Vec<&'static str>,
}

/// Read-only ISO recovery readiness, not an executable conversion plan. An
/// optional explicit user-supplied update object is hashed using 64 KiB memory;
/// nothing is discovered/downloaded or accepted on filename evidence. No DAT
/// or recovery bytes are parsed. Declared metadata is only a lookup hint;
/// a match never enables apply or authenticates the object's measured bytes.
pub fn preview_iso_recovery(
    source: &Path,
    update_candidate: Option<UpdateRecoveryCandidate<'_>>,
) -> Result<NkitRecoveryPreview, NkitInspectionError> {
    let header = inspect(source)?;
    let mut blockers = vec![
        "NKit v1 reconstruction is deferred; no apply or queue job exists",
        "header-only evidence: body, partition structure and source identity not verified",
        "no independently trusted full-output checksum witness",
    ];
    let mut recoverability = NkitRecoverability::Unknown;
    let update_recovery = match (header.removed_update_crc32, update_candidate) {
        (None, None) => UpdateRecoveryEvidence::NotDeclared,
        (None, Some(_)) => {
            return Err(NkitInspectionError::Malformed(
                "update recovery supplied without a matching header requirement",
            ));
        }
        (Some(expected_crc32), None) => {
            recoverability = NkitRecoverability::DependenciesMissing;
            blockers.push("required removed-update recovery object is missing");
            UpdateRecoveryEvidence::Missing { expected_crc32 }
        }
        (Some(expected_crc32), Some(candidate)) => {
            if candidate.declared_recovery_span_crc32 != expected_crc32 {
                return Err(NkitInspectionError::Malformed(
                    "recovery metadata does not match the source requirement",
                ));
            }
            let (size, measured_crc32, measured_sha256) = hash_candidate(candidate.path)?;
            blockers.push(
                "recovery metadata agrees; reconstructed span CRC and compatibility unchecked",
            );
            UpdateRecoveryEvidence::Candidate {
                path: candidate.path.to_path_buf(),
                size,
                measured_crc32,
                measured_sha256,
                expected_crc32,
            }
        }
    };
    Ok(NkitRecoveryPreview {
        source: source.to_path_buf(),
        header,
        update_recovery,
        representation: NkitRepresentationState::NkitV1HeaderRecognizedBodyUnverified,
        recoverability,
        blockers,
    })
}

fn hash_candidate(path: &Path) -> Result<(u64, u32, [u8; 32]), NkitInspectionError> {
    let source = open(path)?;
    let size = source.len();
    if size == 0 || size > WII_SIZE {
        return Err(NkitInspectionError::Malformed("recovery object length"));
    }
    let mut file = source.into_file();
    let before = file.metadata().map_err(io_error)?;
    let mut crc = Crc32::new();
    let mut sha = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = size;
    while remaining != 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count]).map_err(io_error)?;
        crc.update(&buffer[..count]);
        sha.update(&buffer[..count]);
        remaining -= count as u64;
    }
    let after = file.metadata().map_err(io_error)?;
    if after.len() != size || before.modified().ok() != after.modified().ok() {
        return Err(NkitInspectionError::Malformed(
            "recovery object changed during read",
        ));
    }
    // This measured stream is informational; future apply must capture and
    // revalidate the shared strong ObjectIdentity for source AND resources.
    Ok((size, crc.finish(), sha.finalize().into()))
}

fn io_error(error: io::Error) -> NkitInspectionError {
    NkitInspectionError::UnsafeOrUnreadable(error.to_string())
}

#[cfg(test)]
mod tests;
