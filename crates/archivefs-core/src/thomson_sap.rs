//! Bounded, read-only Thomson SAP floppy evidence.
//!
//! SAP is a Thomson-specific floppy container: its first 66 bytes carry the
//! fixed Pukall S.A.P. header, followed by a bounded sequence of sector
//! records.  This proves the media format, not the MO/TO machine family or an
//! exact release.  Those remain explicit/DAT/hash-led decisions.

use crate::content_detector::{ContentDetectionOutcome, ContentDetector, ContentDiagnostic};
use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};

pub const SAP_HEADER_BYTES: usize = 66;
pub const SAP_MAX_BYTES: usize = SAP_HEADER_BYTES + 160 * 16 * (4 + 1024 + 2);
pub const SAP_HEADER_MAGIC: &[u8; 65] =
    b"SYSTEME D'ARCHIVAGE PUKALL S.A.P. (c) Alexandre PUKALL Avril 1998";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThomsonSapObservation {
    pub tracks: u16,
    pub sides: u8,
    pub sectors: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThomsonSapError {
    TooShort,
    TooLarge,
    InvalidFlags,
    InvalidSectorMode,
    TruncatedSector,
    TrailingBytes,
}

impl ThomsonSapError {
    fn diagnostic(self) -> ContentDiagnostic {
        let (category, message) = match self {
            Self::TooShort => ("truncated", "SAP header is incomplete"),
            Self::TooLarge => ("oversized", "SAP image exceeds the bounded observer limit"),
            Self::InvalidFlags => ("bad_header", "SAP header flags are invalid"),
            Self::InvalidSectorMode => ("bad_sector", "SAP sector mode is unsupported"),
            Self::TruncatedSector => ("truncated", "SAP sector record is incomplete"),
            Self::TrailingBytes => (
                "bad_length",
                "SAP image has bytes beyond its declared sectors",
            ),
        };
        ContentDiagnostic {
            detector_id: "thomson_sap",
            category,
            message: message.to_string(),
        }
    }
}

fn sector_payload_bytes(mode: u8) -> Option<usize> {
    match mode & 0x03 {
        0 => Some(256),
        1 => Some(128),
        2 => Some(1024),
        3 => Some(512),
        _ => None,
    }
}

/// Parses the fixed SAP header and every bounded sector record.
pub fn parse_thomson_sap(bytes: &[u8]) -> Result<ThomsonSapObservation, ThomsonSapError> {
    if bytes.len() < SAP_HEADER_BYTES {
        return Err(ThomsonSapError::TooShort);
    }
    if bytes.len() > SAP_MAX_BYTES {
        return Err(ThomsonSapError::TooLarge);
    }
    if &bytes[1..SAP_HEADER_BYTES] != SAP_HEADER_MAGIC {
        return Err(ThomsonSapError::InvalidFlags);
    }
    let flags = bytes[0];
    if flags & 0x7c != 0 {
        return Err(ThomsonSapError::InvalidFlags);
    }
    let tracks = if flags & 0x02 != 0 { 40 } else { 80 };
    let sides = if flags & 0x80 != 0 { 2 } else { 1 };
    let sector_count = usize::from(tracks) * usize::from(sides) * 16;
    let mut cursor = SAP_HEADER_BYTES;
    for _ in 0..sector_count {
        let mode = *bytes.get(cursor).ok_or(ThomsonSapError::TruncatedSector)?;
        let payload = sector_payload_bytes(mode).ok_or(ThomsonSapError::InvalidSectorMode)?;
        let record_bytes = 4usize
            .checked_add(payload)
            .and_then(|value| value.checked_add(2))
            .ok_or(ThomsonSapError::TooLarge)?;
        cursor = cursor
            .checked_add(record_bytes)
            .ok_or(ThomsonSapError::TooLarge)?;
        if cursor > bytes.len() {
            return Err(ThomsonSapError::TruncatedSector);
        }
    }
    if cursor != bytes.len() {
        return Err(ThomsonSapError::TrailingBytes);
    }
    Ok(ThomsonSapObservation {
        tracks,
        sides,
        sectors: sector_count as u32,
    })
}

pub fn observe_thomson_sap(fact: &ThomsonSapObservation) -> Vec<ContentEvidence> {
    vec![
        ContentEvidence::new(
            ContentEvidenceKind::DiskFormat,
            "Thomson SAP",
            ContentEvidenceConfidence::Strong,
            format!(
                "validated SAP header and {} tracks, {} side(s), {} bounded sectors",
                fact.tracks, fact.sides, fact.sectors
            ),
        ),
        ContentEvidence::new(
            ContentEvidenceKind::MediaClass,
            "Floppy",
            ContentEvidenceConfidence::Strong,
            "validated Thomson SAP floppy media structure",
        ),
    ]
}

pub struct ThomsonSapDetector;

impl ContentDetector for ThomsonSapDetector {
    fn id(&self) -> &'static str {
        "thomson_sap"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        match parse_thomson_sap(data) {
            Ok(fact) => ContentDetectionOutcome::Recognized {
                evidence: observe_thomson_sap(&fact),
            },
            Err(error)
                if data.len() >= SAP_HEADER_BYTES
                    && &data[1..SAP_HEADER_BYTES] == SAP_HEADER_MAGIC =>
            {
                ContentDetectionOutcome::Malformed {
                    evidence: vec![ContentEvidence::new(
                        ContentEvidenceKind::DiskFormat,
                        "Thomson SAP",
                        ContentEvidenceConfidence::Corroborated,
                        "fixed SAP header is present but the image failed bounded validation",
                    )],
                    diagnostic: error.diagnostic(),
                }
            }
            Err(_) => ContentDetectionOutcome::NotRecognized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_sap() -> Vec<u8> {
        let mut bytes = vec![0u8; SAP_HEADER_BYTES];
        bytes[1..SAP_HEADER_BYTES].copy_from_slice(SAP_HEADER_MAGIC);
        for _ in 0..(80 * 16) {
            bytes.extend_from_slice(&[0, 0, 0, 1]);
            bytes.extend(std::iter::repeat_n(0, 256 + 2));
        }
        bytes
    }

    #[test]
    fn valid_single_side_sap_is_recognized() {
        let parsed = parse_thomson_sap(&valid_sap()).expect("valid SAP");
        assert_eq!(parsed.tracks, 80);
        assert_eq!(parsed.sides, 1);
        assert_eq!(parsed.sectors, 1280);
        assert!(ThomsonSapDetector.detect(&valid_sap()).is_recognized());
    }

    #[test]
    fn random_bytes_and_extension_are_not_enough() {
        assert!(matches!(
            ThomsonSapDetector.detect(b"not a Thomson disk"),
            ContentDetectionOutcome::NotRecognized
        ));
    }

    #[test]
    fn fixed_header_with_short_sector_is_malformed() {
        let mut bytes = vec![0u8; SAP_HEADER_BYTES];
        bytes[1..SAP_HEADER_BYTES].copy_from_slice(SAP_HEADER_MAGIC);
        assert!(ThomsonSapDetector.detect(&bytes).is_malformed());
    }

    #[test]
    fn parser_is_bounded() {
        assert_eq!(
            parse_thomson_sap(&vec![0; SAP_MAX_BYTES + 1]),
            Err(ThomsonSapError::TooLarge)
        );
    }
}
