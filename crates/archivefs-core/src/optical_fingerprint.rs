//! Representation-independent fingerprints for the deliberately narrow
//! optical-disc slice used by future equivalence review.
//!
//! The only supported representations here are one-file CUE sheets whose one
//! track is `MODE1/2048`, and standalone CHD files with exactly one
//! `MODE1_RAW` CD-ROM data track and zero pregap. The canonical byte stream is
//! the ordered concatenation of cooked 2048-byte user-data sectors. Container
//! hashes and CHD header SHA-1 values are intentionally not used.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::chd_identity::{ChdMetadataFact, ChdMetadataOutcome, observe_chd_identity_file};
use crate::chd_logical_media::open_chd_track_logical_media_file;
use crate::ingestion::cue_bin::{CueError, CueTrackMode, resolve_cue_layout};
use crate::logical_media::LogicalMedia;
use crate::raw_cd_logical_media::open_cooked_cd_file_logical_media;

pub const OPTICAL_FINGERPRINT_SCHEMA: &str = "emuwiz.optical-fingerprint.v1";
pub const LOGICAL_SECTOR_SIZE: u32 = 2048;
const HASH_CHUNK_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpticalRepresentation {
    CueBin,
    Chd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpticalTrackMode {
    Mode1_2048,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpticalDiscStructure {
    pub track_count: u32,
    pub logical_sector_size: u32,
    pub logical_sector_count: u64,
    pub track_mode: OpticalTrackMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalOpticalFingerprint {
    pub schema: &'static str,
    pub structure: OpticalDiscStructure,
    pub canonical_sha256: String,
    pub representation: OpticalRepresentation,
    pub source: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpticalFingerprintError {
    Cue(CueError),
    UnsupportedCueLayout(String),
    Io(String),
    InvalidLogicalSize,
    Chd(String),
    MalformedChdMetadata,
    UnsupportedChdLayout(String),
    LogicalRead(String),
}

impl std::fmt::Display for OpticalFingerprintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cue(error) => write!(f, "{error}"),
            Self::UnsupportedCueLayout(reason) => write!(f, "unsupported CUE layout: {reason}"),
            Self::Io(error) => write!(f, "optical image I/O error: {error}"),
            Self::InvalidLogicalSize => {
                f.write_str("logical image is not a non-empty 2048-byte-sector stream")
            }
            Self::Chd(error) => write!(f, "CHD error: {error}"),
            Self::MalformedChdMetadata => f.write_str("CHD metadata is malformed"),
            Self::UnsupportedChdLayout(reason) => write!(f, "unsupported CHD layout: {reason}"),
            Self::LogicalRead(error) => write!(f, "logical sector read failed: {error}"),
        }
    }
}

impl std::error::Error for OpticalFingerprintError {}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hash_logical_media<M: LogicalMedia>(
    media: &M,
) -> Result<(String, u64), OpticalFingerprintError> {
    if media.len() == 0 || !media.len().is_multiple_of(LOGICAL_SECTOR_SIZE as u64) {
        return Err(OpticalFingerprintError::InvalidLogicalSize);
    }
    let mut hasher = Sha256::new();
    let mut offset = 0u64;
    let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
    while offset < media.len() {
        let count = (media.len() - offset).min(buffer.len() as u64) as usize;
        media
            .read_at(offset, &mut buffer[..count])
            .map_err(|error| OpticalFingerprintError::LogicalRead(error.to_string()))?;
        hasher.update(&buffer[..count]);
        offset = offset
            .checked_add(count as u64)
            .ok_or(OpticalFingerprintError::InvalidLogicalSize)?;
    }
    Ok((
        hex(hasher.finalize()),
        media.len() / LOGICAL_SECTOR_SIZE as u64,
    ))
}

pub fn fingerprint_cue_bin(
    path: &Path,
) -> Result<CanonicalOpticalFingerprint, OpticalFingerprintError> {
    let layout = resolve_cue_layout(path).map_err(OpticalFingerprintError::Cue)?;
    let track = layout
        .supported_single_mode1_2048()
        .map_err(|error| match error {
            CueError::AmbiguousDataTracks => {
                OpticalFingerprintError::UnsupportedCueLayout("multiple tracks or files".into())
            }
            other => OpticalFingerprintError::Cue(other),
        })?;
    if !matches!(
        track.mode,
        CueTrackMode::Data(crate::ingestion::cue_bin::CueDataTrackMode::Mode1_2048)
    ) {
        return Err(OpticalFingerprintError::UnsupportedCueLayout(
            "only MODE1/2048 is supported".into(),
        ));
    }
    let media = open_cooked_cd_file_logical_media(&track.path)
        .map_err(|error| OpticalFingerprintError::Io(error.to_string()))?;
    let (canonical_sha256, sectors) = hash_logical_media(&media)?;
    Ok(CanonicalOpticalFingerprint {
        schema: OPTICAL_FINGERPRINT_SCHEMA,
        structure: OpticalDiscStructure {
            track_count: 1,
            logical_sector_size: LOGICAL_SECTOR_SIZE,
            logical_sector_count: sectors,
            track_mode: OpticalTrackMode::Mode1_2048,
        },
        canonical_sha256,
        representation: OpticalRepresentation::CueBin,
        source: path.to_path_buf(),
    })
}

pub fn fingerprint_chd(
    path: &Path,
) -> Result<CanonicalOpticalFingerprint, OpticalFingerprintError> {
    let identity = observe_chd_identity_file(path)
        .map_err(|error| OpticalFingerprintError::Chd(error.to_string()))?;
    let ChdMetadataOutcome::Observed(metadata) = identity.metadata else {
        return Err(OpticalFingerprintError::MalformedChdMetadata);
    };
    let tracks: Vec<_> = metadata
        .entries
        .iter()
        .filter_map(|entry| match &entry.fact {
            ChdMetadataFact::CdromTrack(track) => Some(track),
            _ => None,
        })
        .collect();
    if tracks.len() != 1 {
        return Err(OpticalFingerprintError::UnsupportedChdLayout(
            "exactly one CD-ROM track is required".into(),
        ));
    }
    let track = tracks[0];
    if track.track != 1 || track.track_type != "MODE1_RAW" || track.pregap != Some(0) {
        return Err(OpticalFingerprintError::UnsupportedChdLayout(
            "requires track 1 MODE1_RAW with zero pregap".into(),
        ));
    }
    let media = open_chd_track_logical_media_file(path)
        .map_err(|error| OpticalFingerprintError::Chd(error.to_string()))?;
    let (canonical_sha256, sectors) = hash_logical_media(&media)?;
    if sectors != u64::from(track.frames) {
        return Err(OpticalFingerprintError::UnsupportedChdLayout(
            "declared frame count does not match logical length".into(),
        ));
    }
    Ok(CanonicalOpticalFingerprint {
        schema: OPTICAL_FINGERPRINT_SCHEMA,
        structure: OpticalDiscStructure {
            track_count: 1,
            logical_sector_size: LOGICAL_SECTOR_SIZE,
            logical_sector_count: sectors,
            track_mode: OpticalTrackMode::Mode1_2048,
        },
        canonical_sha256,
        representation: OpticalRepresentation::Chd,
        source: path.to_path_buf(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpticalFingerprintComparison {
    Equivalent,
    Different,
}

pub fn compare_optical_fingerprints(
    left: &CanonicalOpticalFingerprint,
    right: &CanonicalOpticalFingerprint,
) -> OpticalFingerprintComparison {
    if left.structure == right.structure && left.canonical_sha256 == right.canonical_sha256 {
        OpticalFingerprintComparison::Equivalent
    } else {
        OpticalFingerprintComparison::Different
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chd_logical_media::{MODE1_USER_DATA_OFFSET, RAW_SECTOR_BYTES};

    fn sector(value: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; RAW_SECTOR_BYTES];
        bytes[..12].copy_from_slice(&crate::raw_cd_sector::SYNC_PATTERN);
        bytes[15] = 1;
        for byte in &mut bytes[MODE1_USER_DATA_OFFSET..MODE1_USER_DATA_OFFSET + 2048] {
            *byte = value;
        }
        bytes
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    fn chd_for(sectors: &[Vec<u8>], pregap: u32) -> Vec<u8> {
        let unit = RAW_SECTOR_BYTES as u32;
        let hunk = unit * sectors.len() as u32;
        let payload = format!(
            "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:{} PREGAP:{pregap} PGTYPE:NONE PGSUB:NONE POSTGAP:0",
            sectors.len()
        );
        let meta_offset = 124u64;
        let map_offset = meta_offset + 16 + payload.len() as u64;
        let data_offset = (map_offset + 4).div_ceil(hunk as u64) * hunk as u64;
        let mut chd = vec![0u8; data_offset as usize];
        chd[..8].copy_from_slice(b"MComprHD");
        put_u32(&mut chd, 8, 124);
        put_u32(&mut chd, 12, 5);
        put_u64(&mut chd, 32, hunk as u64);
        put_u64(&mut chd, 40, map_offset);
        put_u64(&mut chd, 48, meta_offset);
        put_u32(&mut chd, 56, hunk);
        put_u32(&mut chd, 60, unit);
        let p = meta_offset as usize;
        chd[p..p + 4].copy_from_slice(b"CHT2");
        chd[p + 5..p + 8].copy_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
        chd[p + 16..p + 16 + payload.len()].copy_from_slice(payload.as_bytes());
        chd[map_offset as usize..map_offset as usize + 4]
            .copy_from_slice(&(data_offset / hunk as u64).to_be_bytes()[4..]);
        for sector in sectors {
            chd.extend_from_slice(sector);
        }
        chd
    }

    fn cue_fixture(values: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let cue = dir.path().join("disc with ü.cue");
        let bin = dir.path().join("track with ü.bin");
        let payload = values
            .iter()
            .flat_map(|value| vec![*value; 2048])
            .collect::<Vec<_>>();
        std::fs::write(&bin, payload).unwrap();
        std::fs::write(
            &cue,
            "FILE \"track with ü.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        (dir, cue)
    }

    #[test]
    fn fn_cue_and_chd_share_the_same_canonical_cooked_sector_fingerprint() {
        let (dir, cue) = cue_fixture(&[0x11, 0x22]);
        let chd = dir.path().join("disc.chd");
        std::fs::write(&chd, chd_for(&[sector(0x11), sector(0x22)], 0)).unwrap();
        let left = fingerprint_cue_bin(&cue).unwrap();
        let right = fingerprint_chd(&chd).unwrap();
        assert_eq!(left.canonical_sha256, right.canonical_sha256);
        assert_eq!(left.structure, right.structure);
        assert_eq!(
            compare_optical_fingerprints(&left, &right),
            OpticalFingerprintComparison::Equivalent
        );
    }

    #[test]
    fn changed_sector_changes_the_fingerprint() {
        let (dir, cue) = cue_fixture(&[0x11, 0x22]);
        let chd = dir.path().join("changed.chd");
        std::fs::write(&chd, chd_for(&[sector(0x11), sector(0x23)], 0)).unwrap();
        let left = fingerprint_cue_bin(&cue).unwrap();
        let right = fingerprint_chd(&chd).unwrap();
        assert_eq!(
            compare_optical_fingerprints(&left, &right),
            OpticalFingerprintComparison::Different
        );
    }

    #[test]
    fn unsupported_cue_and_chd_layouts_fail_closed() {
        let (dir, cue) = cue_fixture(&[0x11]);
        std::fs::write(
            &cue,
            "FILE \"track with ü.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        assert!(fingerprint_cue_bin(&cue).is_err());
        let chd = dir.path().join("pregap.chd");
        std::fs::write(&chd, chd_for(&[sector(0x11)], 1)).unwrap();
        assert!(fingerprint_chd(&chd).is_err());
    }

    #[test]
    fn truncated_bin_is_refused() {
        let (dir, cue) = cue_fixture(&[0x11]);
        std::fs::write(dir.path().join("track with ü.bin"), [1u8; 2047]).unwrap();
        assert!(matches!(
            fingerprint_cue_bin(&cue),
            Err(OpticalFingerprintError::Io(_))
        ));
    }
}
