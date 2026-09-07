//! Bounded, read-only logical-track verification for CHD images.
//!
//! This is deliberately a verification layer, not a CHD writer or a second
//! optical decoder.  CHD metadata supplies the ordered track description and
//! [`crate::chd_logical_media`] supplies the existing streamed logical reader.
//! Only the proven single-track logical path is hashed here; unusual layouts
//! are reported as specialist/unsupported instead of being guessed.
//!
//! A MAME/CHD `<disk sha1>` remains the CHD combined identity, not a per-track
//! Redump hash. Callers must provide per-track expectations explicitly; this
//! module never repurposes a container hash as a track hash.

use std::fmt;
use std::path::Path;

use md5::Md5;
use sha1::digest::Digest;
use sha1::Sha1;

use crate::chd_identity::{
    needs_specialist_optical_backend, observe_chd_identity_file, CdromTrackFact, ChdMetadataFact,
    ChdMetadataOutcome, GdromTrackFact,
};
use crate::chd_logical_media::open_chd_track_logical_media_file;
use crate::dat::model::DatChecksum;
use crate::logical_media::{LogicalMedia, LogicalMediaError};
use crate::optical_fingerprint::{
    compare_optical_fingerprints, fingerprint_chd, fingerprint_cue_bin,
    OpticalFingerprintComparison,
};

/// Fixed memory bound for logical hashing.  CHD hunks are separately bounded
/// by the existing logical reader; this buffer does not grow with disc size.
pub const LOGICAL_HASH_CHUNK_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackVerificationStatus {
    Match,
    Mismatch,
    Unverified,
    Unsupported,
    Incomplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubchannelState {
    Present,
    Absent,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackHashes {
    pub crc32: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
}

impl TrackHashes {
    pub fn from_dat_checksums(checksums: &[DatChecksum]) -> Self {
        let mut result = Self::default();
        for checksum in checksums {
            match checksum.algorithm {
                crate::dat::model::ChecksumAlgorithm::Crc32 => {
                    result.crc32 = Some(checksum.value.to_ascii_lowercase())
                }
                crate::dat::model::ChecksumAlgorithm::Md5 => {
                    result.md5 = Some(checksum.value.to_ascii_lowercase())
                }
                crate::dat::model::ChecksumAlgorithm::Sha1 => {
                    result.sha1 = Some(checksum.value.to_ascii_lowercase())
                }
                crate::dat::model::ChecksumAlgorithm::Sha256 => {}
            }
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedTrack {
    pub number: u32,
    pub track_type: Option<String>,
    pub mode: Option<String>,
    pub form: Option<String>,
    pub frames: Option<u32>,
    pub pregap: Option<u32>,
    pub subchannel: SubchannelState,
    pub hashes: TrackHashes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedTrack {
    pub number: u32,
    pub track_type: String,
    pub mode: String,
    pub form: Option<String>,
    pub frames: u32,
    pub pregap: Option<u32>,
    pub subchannel: SubchannelState,
    pub hashes: Option<TrackHashes>,
    pub unsupported_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackVerification {
    pub number: u32,
    pub status: TrackVerificationStatus,
    pub expected: Option<ExpectedTrack>,
    pub observed: Option<ObservedTrack>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChdRedumpReport {
    pub tracks: Vec<TrackVerification>,
    pub specialist_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChdRedumpError {
    Header(String),
    MetadataUnavailable,
    SpecialistBackendRequired,
    LogicalReader(String),
    Io(String),
}

impl fmt::Display for ChdRedumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(error) => write!(f, "CHD header error: {error}"),
            Self::MetadataUnavailable => f.write_str("CHD has no complete track metadata"),
            Self::SpecialistBackendRequired => {
                f.write_str("CHD layout requires the specialist optical backend")
            }
            Self::LogicalReader(error) => write!(f, "logical CHD reader unavailable: {error}"),
            Self::Io(error) => write!(f, "CHD verification I/O error: {error}"),
        }
    }
}

impl std::error::Error for ChdRedumpError {}

/// Compare ordered track declarations without selecting a winner when DAT
/// metadata disagrees. Missing and extra tracks are explicit `Incomplete`.
pub fn compare_track_sets(
    expected: &[ExpectedTrack],
    observed: &[ObservedTrack],
) -> Vec<TrackVerification> {
    let mut numbers: Vec<u32> = expected
        .iter()
        .map(|track| track.number)
        .chain(observed.iter().map(|track| track.number))
        .collect();
    numbers.sort_unstable();
    numbers.dedup();

    numbers
        .into_iter()
        .map(|number| {
            let expected_track = expected
                .iter()
                .find(|track| track.number == number)
                .cloned();
            let observed_track = observed
                .iter()
                .find(|track| track.number == number)
                .cloned();
            match (expected_track, observed_track) {
                (None, Some(observed)) => TrackVerification {
                    number,
                    status: TrackVerificationStatus::Incomplete,
                    expected: None,
                    observed: Some(observed),
                    reasons: vec!["observed track is not present in the expectation".into()],
                },
                (Some(expected), None) => TrackVerification {
                    number,
                    status: TrackVerificationStatus::Incomplete,
                    expected: Some(expected),
                    observed: None,
                    reasons: vec!["expected track is missing from the CHD".into()],
                },
                (Some(expected), Some(observed)) => compare_track(number, expected, observed),
                (None, None) => unreachable!(),
            }
        })
        .collect()
}

fn compare_track(
    number: u32,
    expected: ExpectedTrack,
    observed: ObservedTrack,
) -> TrackVerification {
    let mut reasons = Vec::new();
    if expected
        .track_type
        .as_deref()
        .is_some_and(|value| !value.eq_ignore_ascii_case(&observed.track_type))
    {
        reasons.push("track type differs".into());
    }
    if expected
        .mode
        .as_deref()
        .is_some_and(|value| !value.eq_ignore_ascii_case(&observed.mode))
    {
        reasons.push("track mode differs".into());
    }
    if expected.form.as_deref() != observed.form.as_deref() && expected.form.is_some() {
        reasons.push("track form differs".into());
    }
    if expected
        .frames
        .is_some_and(|value| value != observed.frames)
    {
        reasons.push("frame count differs".into());
    }
    if expected
        .pregap
        .is_some_and(|value| Some(value) != observed.pregap)
    {
        reasons.push("pregap differs".into());
    }
    if expected.subchannel != SubchannelState::Unknown
        && expected.subchannel != SubchannelState::Unavailable
        && expected.subchannel != observed.subchannel
    {
        reasons.push("subchannel state differs".into());
    }

    // Copy the small hash bundle out before moving `observed` into the
    // result.  Keeping a borrow of a field here would prevent that move.
    let Some(actual_hashes) = observed.hashes.clone() else {
        let unsupported_reason = observed.unsupported_reason.clone();
        let unsupported = unsupported_reason.is_some();
        return TrackVerification {
            number,
            status: if unsupported {
                TrackVerificationStatus::Unsupported
            } else if reasons.is_empty() {
                TrackVerificationStatus::Unverified
            } else {
                TrackVerificationStatus::Mismatch
            },
            expected: Some(expected),
            observed: Some(observed),
            reasons: unsupported_reason.into_iter().chain(reasons).collect(),
        };
    };
    for (label, wanted, actual) in [
        ("CRC32", &expected.hashes.crc32, &actual_hashes.crc32),
        ("MD5", &expected.hashes.md5, &actual_hashes.md5),
        ("SHA-1", &expected.hashes.sha1, &actual_hashes.sha1),
    ] {
        if let Some(wanted) = wanted {
            if actual.as_deref() != Some(wanted.as_str()) {
                reasons.push(format!("{label} differs"));
            }
        }
    }
    let status = if !reasons.is_empty() {
        TrackVerificationStatus::Mismatch
    } else if expected.hashes.crc32.is_none()
        && expected.hashes.md5.is_none()
        && expected.hashes.sha1.is_none()
    {
        TrackVerificationStatus::Unverified
    } else {
        TrackVerificationStatus::Match
    };
    TrackVerification {
        number,
        status,
        expected: Some(expected),
        observed: Some(observed),
        reasons,
    }
}

/// Verify a CHD's declared tracks and hash the proven logical track stream.
/// The existing reader currently exposes only track 1 with zero pregap, so
/// other tracks remain visible but `UNVERIFIED`; no bytes are guessed.
pub fn verify_chd_file(
    path: &Path,
    expected: &[ExpectedTrack],
) -> Result<ChdRedumpReport, ChdRedumpError> {
    let identity = observe_chd_identity_file(path)
        .map_err(|error| ChdRedumpError::Header(error.to_string()))?;
    let ChdMetadataOutcome::Observed(metadata) = identity.metadata else {
        return Err(ChdRedumpError::MetadataUnavailable);
    };
    if needs_specialist_optical_backend(&metadata) {
        return Err(ChdRedumpError::SpecialistBackendRequired);
    }
    let observed = metadata
        .entries
        .iter()
        .filter_map(|entry| match &entry.fact {
            ChdMetadataFact::CdromTrack(track) => Some(observed_from_cdrom(track)),
            ChdMetadataFact::GdromTrack(track) => Some(observed_from_gdrom(track)),
            _ => None,
        })
        .map(|mut track| {
            if track.number == 1
                && track.pregap == Some(0)
                && track.track_type != "AUDIO"
                && matches!(
                    track.track_type.as_str(),
                    "MODE1" | "MODE1_RAW" | "MODE2_RAW"
                )
            {
                match open_chd_track_logical_media_file(path)
                    .map_err(|error| error.to_string())
                    .and_then(|media| hash_logical_media(&media))
                {
                    Ok(hashes) => track.hashes = Some(hashes),
                    Err(error) => {
                        track.unsupported_reason = Some(error);
                    }
                }
            }
            Ok(track)
        })
        .collect::<Result<Vec<_>, ChdRedumpError>>()?;
    Ok(ChdRedumpReport {
        tracks: compare_track_sets(expected, &observed),
        specialist_required: false,
    })
}

fn observed_from_cdrom(track: &CdromTrackFact) -> ObservedTrack {
    observed_track(
        track.track,
        &track.track_type,
        &track.subtype,
        track.frames,
        track.pregap,
        track.pregap_subtype.as_deref(),
    )
}

fn observed_from_gdrom(track: &GdromTrackFact) -> ObservedTrack {
    observed_track(
        track.track,
        &track.track_type,
        &track.subtype,
        track.frames,
        track.pregap,
        track.pregap_subtype.as_deref(),
    )
}

fn observed_track(
    number: u32,
    track_type: &str,
    subtype: &str,
    frames: u32,
    pregap: Option<u32>,
    pregap_subtype: Option<&str>,
) -> ObservedTrack {
    let mode = if track_type == "AUDIO" {
        "AUDIO"
    } else {
        track_type
    };
    let form = if track_type.starts_with("MODE2") {
        [subtype, pregap_subtype.unwrap_or("")]
            .into_iter()
            .find(|value| value.to_ascii_uppercase().contains("FORM"))
            .map(str::to_string)
    } else {
        None
    };
    let subchannel = match pregap_subtype.map(str::to_ascii_uppercase).as_deref() {
        Some("NONE") => SubchannelState::Absent,
        Some(value) if value.contains("RW") => SubchannelState::Present,
        Some(_) => SubchannelState::Unknown,
        None => SubchannelState::Unavailable,
    };
    ObservedTrack {
        number,
        track_type: track_type.to_string(),
        mode: mode.to_string(),
        form,
        frames,
        pregap,
        subchannel,
        hashes: None,
        unsupported_reason: None,
    }
}

fn hash_logical_media<M: LogicalMedia>(media: &M) -> Result<TrackHashes, String> {
    let mut crc = Crc32::new();
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut offset = 0_u64;
    let mut buffer = vec![0_u8; LOGICAL_HASH_CHUNK_BYTES];
    while offset < media.len() {
        let count = (media.len() - offset).min(buffer.len() as u64) as usize;
        media
            .read_at(offset, &mut buffer[..count])
            .map_err(|error| match error {
                LogicalMediaError::DecodeFailed { detail } => detail,
                other => other.to_string(),
            })?;
        crc.update(&buffer[..count]);
        md5.update(&buffer[..count]);
        sha1.update(&buffer[..count]);
        offset += count as u64;
    }
    Ok(TrackHashes {
        crc32: Some(crc.finish_hex()),
        md5: Some(hex(md5.finalize())),
        sha1: Some(hex(sha1.finalize())),
    })
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Compare the existing strong single-track CUE/BIN and CHD logical views.
/// Unsupported multi-track, pregap, audio and specialist layouts are refused
/// by the existing fingerprint adapters rather than declared equivalent.
pub fn compare_cue_bin_chd_logical(
    cue: &Path,
    chd: &Path,
) -> Result<TrackVerificationStatus, String> {
    let left = fingerprint_cue_bin(cue).map_err(|error| error.to_string())?;
    let right = fingerprint_chd(chd).map_err(|error| error.to_string())?;
    Ok(
        if compare_optical_fingerprints(&left, &right) == OpticalFingerprintComparison::Equivalent {
            TrackVerificationStatus::Match
        } else {
            TrackVerificationStatus::Mismatch
        },
    )
}

#[derive(Debug, Clone, Copy)]
struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Self(0xffff_ffff)
    }

    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u32::from(byte);
            for _ in 0..8 {
                self.0 = if self.0 & 1 != 0 {
                    (self.0 >> 1) ^ 0xedb8_8320
                } else {
                    self.0 >> 1
                };
            }
        }
    }

    fn finish_hex(self) -> String {
        format!("{:08x}", !self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected(number: u32, mode: &str) -> ExpectedTrack {
        ExpectedTrack {
            number,
            track_type: Some(mode.to_string()),
            mode: Some(mode.to_string()),
            form: None,
            frames: Some(2),
            pregap: Some(0),
            subchannel: SubchannelState::Absent,
            hashes: TrackHashes::default(),
        }
    }

    fn observed(number: u32, mode: &str) -> ObservedTrack {
        ObservedTrack {
            number,
            track_type: mode.to_string(),
            mode: mode.to_string(),
            form: None,
            frames: 2,
            pregap: Some(0),
            subchannel: SubchannelState::Absent,
            hashes: Some(TrackHashes::default()),
            unsupported_reason: None,
        }
    }

    #[test]
    fn mode1_audio_and_mode2_form1_are_compared_by_structure() {
        for (number, mode) in [(1, "MODE1_RAW"), (2, "AUDIO"), (3, "MODE2_RAW")] {
            let result = compare_track_sets(&[expected(number, mode)], &[observed(number, mode)]);
            assert_eq!(result[0].status, TrackVerificationStatus::Unverified);
        }
    }

    #[test]
    fn mode2_form2_can_be_refused_by_missing_hash_evidence() {
        let mut wanted = expected(1, "MODE2_RAW");
        wanted.form = Some("FORM2".into());
        let mut actual = observed(1, "MODE2_RAW");
        actual.form = Some("FORM2".into());
        actual.hashes = None;
        actual.unsupported_reason =
            Some("MODE2 Form 2 is not supported by the logical reader".into());
        assert_eq!(
            compare_track_sets(&[wanted], &[actual])[0].status,
            TrackVerificationStatus::Unsupported
        );
    }

    #[test]
    fn hash_mismatch_and_missing_extra_tracks_are_explicit() {
        let mut wanted = expected(1, "MODE1_RAW");
        wanted.hashes.sha1 = Some("00".repeat(20));
        let mut actual = observed(1, "MODE1_RAW");
        actual.hashes.as_mut().unwrap().sha1 = Some("11".repeat(20));
        let mut results = compare_track_sets(
            &[wanted, expected(3, "AUDIO")],
            &[actual, observed(2, "AUDIO")],
        );
        results.sort_by_key(|result| result.number);
        assert_eq!(results[0].status, TrackVerificationStatus::Mismatch);
        assert_eq!(results[1].status, TrackVerificationStatus::Incomplete);
        assert_eq!(results[2].status, TrackVerificationStatus::Incomplete);
    }

    #[test]
    fn pregap_unknown_and_subchannel_unavailable_do_not_claim_match() {
        let mut wanted = expected(1, "MODE1_RAW");
        wanted.pregap = Some(150);
        wanted.subchannel = SubchannelState::Present;
        let mut actual = observed(1, "MODE1_RAW");
        actual.pregap = None;
        actual.subchannel = SubchannelState::Unavailable;
        let result = compare_track_sets(&[wanted], &[actual]);
        assert_eq!(result[0].status, TrackVerificationStatus::Mismatch);
    }

    #[test]
    fn same_title_is_not_part_of_logical_matching() {
        let mut left = expected(1, "MODE1_RAW");
        left.hashes.sha1 = Some("00".repeat(20));
        let mut right = observed(1, "MODE1_RAW");
        right.hashes.as_mut().unwrap().sha1 = Some("11".repeat(20));
        assert_eq!(
            compare_track_sets(&[left], &[right])[0].status,
            TrackVerificationStatus::Mismatch
        );
    }
}
