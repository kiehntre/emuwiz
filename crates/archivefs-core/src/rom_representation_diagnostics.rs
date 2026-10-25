//! Conservative, read-only diagnostics for ROM byte representations.
//!
//! This module explains a hash mismatch when an already-proven in-memory
//! representation transform produces the expected identity. It never changes
//! the source buffer, writes a converted file, or weakens DAT identity.

use serde::{Deserialize, Serialize};

use crate::dat::audit::KnownFileEvidence;
use crate::n64_byte_order::{detect_n64_byte_order, normalize_to_z64};
use crate::platform_evidence_fusion::dat_hash_representation::hash_bytes;

/// The platform/format context supplied by the caller. Extensions and
/// filenames are intentionally not represented here: they are not proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RomFormatContext {
    Snes,
    Nes,
    N64,
    Genesis,
    Unknown,
}

/// The explanation returned by the diagnostic layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RomRepresentationKind {
    ExactMatch,
    HeaderedEquivalent,
    HeaderlessEquivalent,
    ByteOrderEquivalent,
    InterleavedEquivalent,
    ContainerDifference,
    ModifiedContent,
    UnknownMismatch,
}

/// Whether the comparison proved exact or representation-equivalent content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationEquivalence {
    Exact,
    Equivalent,
    Modified,
    Unknown,
}

/// Confidence attached to one representation observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationDiagnosticConfidence {
    Exact,
    Strong,
    Weak,
    Unknown,
}

/// Hash algorithm used by the expected identity evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RomHashAlgorithm {
    Crc32,
    Md5,
    Sha1,
    Sha256,
}

impl RomHashAlgorithm {
    fn value<'a>(self, evidence: &'a KnownFileEvidence) -> Option<&'a str> {
        match self {
            Self::Crc32 => evidence.crc32.as_deref(),
            Self::Md5 => evidence.md5.as_deref(),
            Self::Sha1 => evidence.sha1.as_deref(),
            Self::Sha256 => evidence.sha256.as_deref(),
        }
    }
}

/// Expected identity evidence supplied by the caller, normally from a DAT or
/// an already trusted identity record. The diagnostic never creates identity
/// evidence; it only compares bytes against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedHashEvidence {
    pub algorithm: RomHashAlgorithm,
    pub expected_hash: String,
    pub expected_size: Option<u64>,
    pub identity_label: Option<String>,
}

/// Evidence for one physical or hypothetical normalized view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RomRepresentationEvidence {
    pub representation: RomRepresentationKind,
    pub source_hash: String,
    pub normalized_hash: Option<String>,
    pub expected_hash: String,
    pub normalization_operation: Option<String>,
    pub source_size: u64,
    pub normalized_size: Option<u64>,
    pub expected_size: Option<u64>,
    pub confidence: RepresentationDiagnosticConfidence,
    pub reason: String,
}

/// Read-only explanation of one ROM comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RomRepresentationDiagnostic {
    pub platform: RomFormatContext,
    pub kind: RomRepresentationKind,
    pub equivalence: RepresentationEquivalence,
    pub source_hash: String,
    pub expected_hash: String,
    pub normalized_hash: Option<String>,
    pub normalization_operation: Option<String>,
    pub evidence: Vec<RomRepresentationEvidence>,
    pub reason: String,
    pub source_unchanged: bool,
}

/// A representation-aware DAT comparison kept alongside the canonical DAT
/// verdict. `RepresentationEquivalent` is explanatory evidence only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatRepresentationState {
    RawHashMatch,
    RepresentationEquivalent,
    ContentMismatch,
    Conflicting,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatRepresentationVerification {
    pub state: DatRepresentationState,
    pub raw_hash_match: bool,
    pub diagnostic: Option<RomRepresentationDiagnostic>,
}

/// Compare bytes to an already-selected DAT record without changing the
/// normal DAT identity verdict. In particular, callers must not substitute
/// this result for `AuditVerdict` or treat a filename-only match as proof.
pub fn verify_dat_representation(
    bytes: &[u8],
    platform: RomFormatContext,
    expected: &KnownFileEvidence,
) -> DatRepresentationVerification {
    let unknown = || DatRepresentationVerification {
        state: DatRepresentationState::Unknown,
        raw_hash_match: false,
        diagnostic: None,
    };
    let candidates = [
        (RomHashAlgorithm::Sha256, expected.sha256.as_deref(), 64),
        (RomHashAlgorithm::Sha1, expected.sha1.as_deref(), 40),
        (RomHashAlgorithm::Md5, expected.md5.as_deref(), 32),
        (RomHashAlgorithm::Crc32, expected.crc32.as_deref(), 8),
    ];
    if bytes.len() > 64 * 1024 * 1024 {
        let actual = hash_bytes(bytes, "<memory>", "<memory>");
        let mut comparisons = Vec::new();
        for (algorithm, value, length) in candidates {
            let Some(value) = value else {
                continue;
            };
            if value.len() != length || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                return unknown();
            }
            comparisons.push(
                algorithm
                    .value(&actual)
                    .is_some_and(|h| h.eq_ignore_ascii_case(value)),
            );
        }
        let all = !comparisons.is_empty()
            && comparisons.iter().all(|same| *same)
            && expected
                .size_bytes
                .is_none_or(|size| size == bytes.len() as u64);
        let state = if all {
            DatRepresentationState::RawHashMatch
        } else if comparisons.iter().any(|same| *same) {
            DatRepresentationState::Conflicting
        } else {
            DatRepresentationState::Unknown
        };
        return DatRepresentationVerification {
            raw_hash_match: all,
            state,
            diagnostic: None,
        };
    }
    let mut diagnostics = Vec::new();
    for (algorithm, value, length) in candidates {
        let Some(value) = value else {
            continue;
        };
        if value.len() != length || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return unknown();
        }
        diagnostics.push(diagnose_rom_representation(
            bytes,
            platform,
            &NormalizedHashEvidence {
                algorithm,
                expected_hash: value.into(),
                expected_size: expected.size_bytes,
                identity_label: Some(expected.filename.clone()),
            },
        ));
    }
    let Some(first) = diagnostics.first() else {
        return unknown();
    };
    let matching = |d: &RomRepresentationDiagnostic| {
        matches!(
            d.equivalence,
            RepresentationEquivalence::Exact | RepresentationEquivalence::Equivalent
        )
    };
    let all_agree = diagnostics.iter().all(|d| {
        matching(d)
            && d.equivalence == first.equivalence
            && d.normalization_operation == first.normalization_operation
    });
    let state = if all_agree {
        if first.equivalence == RepresentationEquivalence::Exact {
            DatRepresentationState::RawHashMatch
        } else {
            DatRepresentationState::RepresentationEquivalent
        }
    } else if diagnostics.iter().any(matching) {
        DatRepresentationState::Conflicting
    } else if first.equivalence == RepresentationEquivalence::Modified {
        DatRepresentationState::ContentMismatch
    } else {
        DatRepresentationState::Unknown
    };
    DatRepresentationVerification {
        raw_hash_match: state == DatRepresentationState::RawHashMatch,
        state,
        diagnostic: Some(first.clone()),
    }
}

fn hash_value(bytes: &[u8], algorithm: RomHashAlgorithm) -> String {
    let evidence = hash_bytes(bytes, "<memory>", "<memory>");
    algorithm.value(&evidence).unwrap_or_default().to_string()
}

fn expected_size_matches(size: usize, expected: &NormalizedHashEvidence) -> bool {
    expected
        .expected_size
        .is_none_or(|expected_size| expected_size == size as u64)
}

fn physical_evidence(
    bytes: &[u8],
    expected: &NormalizedHashEvidence,
) -> (KnownFileEvidence, String) {
    let hashes = hash_bytes(bytes, "<memory>", "<memory>");
    let source_hash = expected
        .algorithm
        .value(&hashes)
        .unwrap_or_default()
        .to_string();
    (hashes, source_hash)
}

fn diagnostic(
    platform: RomFormatContext,
    kind: RomRepresentationKind,
    equivalence: RepresentationEquivalence,
    source_hash: String,
    expected: &NormalizedHashEvidence,
    normalized_hash: Option<String>,
    operation: Option<&str>,
    confidence: RepresentationDiagnosticConfidence,
    reason: impl Into<String>,
    bytes: &[u8],
) -> RomRepresentationDiagnostic {
    let reason = reason.into();
    RomRepresentationDiagnostic {
        platform,
        kind,
        equivalence,
        source_hash: source_hash.clone(),
        expected_hash: expected.expected_hash.clone(),
        normalized_hash: normalized_hash.clone(),
        normalization_operation: operation.map(str::to_string),
        evidence: vec![RomRepresentationEvidence {
            representation: kind,
            source_hash,
            normalized_hash,
            expected_hash: expected.expected_hash.clone(),
            normalization_operation: operation.map(str::to_string),
            source_size: bytes.len() as u64,
            normalized_size: None,
            expected_size: expected.expected_size,
            confidence,
            reason: reason.clone(),
        }],
        reason,
        source_unchanged: true,
    }
}

/// Diagnoses a ROM representation against expected hash evidence.
///
/// Only a physical hash match or a proven in-memory transform that hashes to
/// the expected identity can produce an equivalence result. Weak shape
/// detection (SNES copier headers and SMD) is never sufficient by itself.
pub fn diagnose_rom_representation(
    bytes: &[u8],
    platform: RomFormatContext,
    expected: &NormalizedHashEvidence,
) -> RomRepresentationDiagnostic {
    let original = bytes.to_vec();
    let (_physical, source_hash) = physical_evidence(bytes, expected);
    let physical_match = source_hash.eq_ignore_ascii_case(&expected.expected_hash)
        && expected_size_matches(bytes.len(), expected);
    if physical_match {
        return diagnostic(
            platform,
            RomRepresentationKind::ExactMatch,
            RepresentationEquivalence::Exact,
            source_hash,
            expected,
            None,
            None,
            RepresentationDiagnosticConfidence::Exact,
            "the physical source hash matches the expected identity",
            bytes,
        );
    }

    let mut result = match platform {
        RomFormatContext::N64 => diagnose_n64(bytes, expected, &source_hash),
        RomFormatContext::Snes => diagnose_header(bytes, platform, expected, &source_hash, true),
        RomFormatContext::Nes => diagnose_header(bytes, platform, expected, &source_hash, false),
        RomFormatContext::Genesis => diagnose_genesis(bytes, expected, &source_hash),
        RomFormatContext::Unknown => None,
    }
    .unwrap_or_else(|| {
        let kind = match platform {
            RomFormatContext::N64 => match detect_n64_byte_order(bytes) {
                Some(order) if normalize_to_z64(bytes, order).is_ok() => {
                    RomRepresentationKind::ModifiedContent
                }
                _ => RomRepresentationKind::UnknownMismatch,
            },
            _ => RomRepresentationKind::UnknownMismatch,
        };
        diagnostic(
            platform,
            kind,
            if kind == RomRepresentationKind::ModifiedContent {
                RepresentationEquivalence::Modified
            } else {
                RepresentationEquivalence::Unknown
            },
            source_hash,
            expected,
            None,
            None,
            RepresentationDiagnosticConfidence::Unknown,
            "the physical hash does not match and no proven representation transform matched",
            bytes,
        )
    });
    result.source_unchanged = bytes == original.as_slice();
    result
}

fn diagnose_n64(
    bytes: &[u8],
    expected: &NormalizedHashEvidence,
    source_hash: &str,
) -> Option<RomRepresentationDiagnostic> {
    let order = detect_n64_byte_order(bytes)?;
    let normalized = normalize_to_z64(bytes, order).ok()?;
    let normalized_hash = hash_value(&normalized.bytes, expected.algorithm);
    if !normalized_hash.eq_ignore_ascii_case(&expected.expected_hash)
        || !expected_size_matches(normalized.bytes.len(), expected)
    {
        return None;
    }
    Some(diagnostic(
        RomFormatContext::N64,
        RomRepresentationKind::ByteOrderEquivalent,
        RepresentationEquivalence::Equivalent,
        source_hash.to_string(),
        expected,
        Some(normalized_hash),
        Some(normalized.transform),
        RepresentationDiagnosticConfidence::Exact,
        format!(
            "{} byte-order bytes normalize to the expected canonical N64 identity",
            order.label()
        ),
        bytes,
    ))
}

fn diagnose_header(
    bytes: &[u8],
    platform: RomFormatContext,
    expected: &NormalizedHashEvidence,
    source_hash: &str,
    snes_only: bool,
) -> Option<RomRepresentationDiagnostic> {
    use crate::header_normalization::{
        HeaderNormalizationKind, recognize_header_normalization, strip_known_header,
    };
    let kind = recognize_header_normalization(bytes)
        .into_iter()
        .find(|kind| {
            if snes_only {
                *kind == HeaderNormalizationKind::SnesCopier512
            } else {
                *kind == HeaderNormalizationKind::INes16
            }
        })?;
    let normalized = strip_known_header(bytes, kind).ok()?;
    let normalized_hash = hash_value(&normalized.bytes, expected.algorithm);
    if !normalized_hash.eq_ignore_ascii_case(&expected.expected_hash)
        || !expected_size_matches(normalized.bytes.len(), expected)
    {
        return None;
    }
    Some(diagnostic(
        platform,
        RomRepresentationKind::HeaderedEquivalent,
        RepresentationEquivalence::Equivalent,
        source_hash.to_string(),
        expected,
        Some(normalized_hash),
        Some(normalized.transform_id),
        RepresentationDiagnosticConfidence::Exact,
        format!(
            "{} normalizes to the expected headerless identity",
            kind.label()
        ),
        bytes,
    ))
}

fn diagnose_genesis(
    bytes: &[u8],
    expected: &NormalizedHashEvidence,
    source_hash: &str,
) -> Option<RomRepresentationDiagnostic> {
    use crate::smd_normalization::{detect_smd_candidate, normalize_smd_to_bin};
    if !detect_smd_candidate(bytes) {
        return None;
    }
    let normalized = normalize_smd_to_bin(bytes).ok()?;
    let normalized_hash = hash_value(&normalized.bytes, expected.algorithm);
    if !normalized_hash.eq_ignore_ascii_case(&expected.expected_hash)
        || !expected_size_matches(normalized.bytes.len(), expected)
    {
        return None;
    }
    Some(diagnostic(
        RomFormatContext::Genesis,
        RomRepresentationKind::InterleavedEquivalent,
        RepresentationEquivalence::Equivalent,
        source_hash.to_string(),
        expected,
        Some(normalized_hash),
        Some(normalized.transform_id),
        RepresentationDiagnosticConfidence::Exact,
        "SMD/interleaved bytes normalize to the expected raw Mega Drive identity",
        bytes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header_normalization::HeaderNormalizationKind;
    use crate::n64_byte_order::{N64ByteOrder, denormalize_from_z64};
    use crate::smd_normalization::{SMD_BLOCK_LEN, SMD_HEADER_LEN, interleave_bin_to_smd};

    fn expected(bytes: &[u8]) -> NormalizedHashEvidence {
        let hashes = hash_bytes(bytes, "<memory>", "fixture");
        NormalizedHashEvidence {
            algorithm: RomHashAlgorithm::Sha256,
            expected_hash: hashes.sha256.unwrap(),
            expected_size: Some(bytes.len() as u64),
            identity_label: Some("synthetic fixture".into()),
        }
    }

    fn canonical_n64() -> Vec<u8> {
        [
            N64ByteOrder::Z64.magic().as_slice(),
            &[1, 2, 3, 4, 5, 6, 7, 8],
        ]
        .concat()
    }

    fn canonical_bin() -> Vec<u8> {
        (0..SMD_BLOCK_LEN)
            .map(|index| (index % 251) as u8)
            .collect()
    }

    #[test]
    fn exact_hash_match_is_exact() {
        let bytes = b"exact fixture";
        let result =
            diagnose_rom_representation(bytes, RomFormatContext::Unknown, &expected(bytes));
        assert_eq!(result.kind, RomRepresentationKind::ExactMatch);
        assert_eq!(result.equivalence, RepresentationEquivalence::Exact);
    }

    #[test]
    fn snes_header_is_equivalent_only_when_hash_proves_it() {
        let payload = vec![0x42; 32 * 1024];
        let mut headered = vec![0xa5; 512];
        headered.extend_from_slice(&payload);
        let result =
            diagnose_rom_representation(&headered, RomFormatContext::Snes, &expected(&payload));
        assert_eq!(result.kind, RomRepresentationKind::HeaderedEquivalent);
        assert_eq!(
            result.normalization_operation,
            Some(HeaderNormalizationKind::SnesCopier512.transform_id().into())
        );
    }

    #[test]
    fn random_extra_prefix_is_not_accepted_without_hash_proof() {
        let mut bytes = vec![0x11; 512];
        bytes.extend_from_slice(&vec![0x42; 32 * 1024]);
        let result =
            diagnose_rom_representation(&bytes, RomFormatContext::Snes, &expected(b"different"));
        assert_eq!(result.kind, RomRepresentationKind::UnknownMismatch);
    }

    #[test]
    fn n64_z64_exact_and_v64_n64_equivalent() {
        let z64 = canonical_n64();
        let v64 = denormalize_from_z64(&z64, N64ByteOrder::V64).unwrap();
        let n64 = denormalize_from_z64(&z64, N64ByteOrder::N64).unwrap();
        assert_eq!(
            diagnose_rom_representation(&z64, RomFormatContext::N64, &expected(&z64)).kind,
            RomRepresentationKind::ExactMatch
        );
        assert_eq!(
            diagnose_rom_representation(&v64, RomFormatContext::N64, &expected(&z64)).kind,
            RomRepresentationKind::ByteOrderEquivalent
        );
        assert_eq!(
            diagnose_rom_representation(&n64, RomFormatContext::N64, &expected(&z64)).kind,
            RomRepresentationKind::ByteOrderEquivalent
        );
    }

    #[test]
    fn malformed_n64_is_unknown_and_wrong_normalized_content_is_modified() {
        let malformed = [N64ByteOrder::N64.magic().as_slice(), &[1, 2]].concat();
        assert_eq!(
            diagnose_rom_representation(&malformed, RomFormatContext::N64, &expected(b"wrong"))
                .kind,
            RomRepresentationKind::UnknownMismatch
        );
        let mut wrong = canonical_n64();
        wrong[7] ^= 0xff;
        assert_eq!(
            diagnose_rom_representation(&wrong, RomFormatContext::N64, &expected(&canonical_n64()))
                .kind,
            RomRepresentationKind::ModifiedContent
        );
    }

    #[test]
    fn genesis_raw_and_smd_are_distinguished_safely() {
        let raw = canonical_bin();
        let mut smd = vec![0; SMD_HEADER_LEN];
        smd.extend_from_slice(&interleave_bin_to_smd(&raw).unwrap());
        assert_eq!(
            diagnose_rom_representation(&raw, RomFormatContext::Genesis, &expected(&raw)).kind,
            RomRepresentationKind::ExactMatch
        );
        assert_eq!(
            diagnose_rom_representation(&smd, RomFormatContext::Genesis, &expected(&raw)).kind,
            RomRepresentationKind::InterleavedEquivalent
        );
    }

    #[test]
    fn unknown_format_and_source_bytes_remain_unchanged() {
        let bytes = b"not a known ROM format".to_vec();
        let before = bytes.clone();
        let result =
            diagnose_rom_representation(&bytes, RomFormatContext::Unknown, &expected(b"other"));
        assert_eq!(result.kind, RomRepresentationKind::UnknownMismatch);
        assert_eq!(bytes, before);
        assert!(result.source_unchanged);
    }
}

#[cfg(test)]
mod recovered_readiness_tests {
    use super::*;
    #[test]
    fn dat_representation_all_available_hashes_must_agree() {
        let bytes = b"tiny synthetic cartridge";
        let hashes = hash_bytes(bytes, "", "");
        let expected = KnownFileEvidence::new("rom.bin", "rom.bin")
            .with_sha256(hashes.sha256.unwrap())
            .with_sha1(hashes.sha1.unwrap())
            .with_md5(hashes.md5.unwrap())
            .with_crc32(hashes.crc32.unwrap());
        assert_eq!(
            verify_dat_representation(bytes, RomFormatContext::Unknown, &expected).state,
            DatRepresentationState::RawHashMatch
        );
        let mut conflict = expected.clone();
        conflict.sha1 = Some("0".repeat(40));
        assert_eq!(
            verify_dat_representation(bytes, RomFormatContext::Unknown, &conflict).state,
            DatRepresentationState::Conflicting
        );
        conflict = expected.clone();
        conflict.crc32 = Some("00000000".into());
        assert_eq!(
            verify_dat_representation(bytes, RomFormatContext::Unknown, &conflict).state,
            DatRepresentationState::Conflicting
        );
        conflict = expected;
        conflict.sha256 = Some("0".repeat(64));
        assert_eq!(
            verify_dat_representation(bytes, RomFormatContext::Unknown, &conflict).state,
            DatRepresentationState::Conflicting
        );
    }
    #[test]
    fn dat_representation_header_equivalence_never_changes_raw_dat_identity() {
        let payload = vec![0x42; 32 * 1024];
        let mut headered = vec![0xa5; 512];
        headered.extend(&payload);
        let expected = KnownFileEvidence::new("named.sfc", "named.sfc")
            .with_size(payload.len() as u64)
            .with_sha256(hash_bytes(&payload, "", "").sha256.unwrap());
        let result = verify_dat_representation(&headered, RomFormatContext::Snes, &expected);
        assert_eq!(
            result.state,
            DatRepresentationState::RepresentationEquivalent
        );
        assert!(!result.raw_hash_match);
        assert_eq!(expected.filename, "named.sfc");
    }
    #[test]
    fn dat_representation_filename_missing_and_malformed_hashes_remain_unknown() {
        let mut expected = KnownFileEvidence::new("title.sfc", "title.sfc");
        assert_eq!(
            verify_dat_representation(b"bytes", RomFormatContext::Snes, &expected).state,
            DatRepresentationState::Unknown
        );
        expected.sha256 = Some("not-a-hash".into());
        assert_eq!(
            verify_dat_representation(b"bytes", RomFormatContext::Snes, &expected).state,
            DatRepresentationState::Unknown
        );
    }
    #[test]
    fn snes_context_never_suggests_stripping_an_ines_header() {
        let payload = vec![0x42; 16384];
        let mut bytes = vec![0; 16];
        bytes[..4].copy_from_slice(b"NES\x1a");
        bytes[4] = 1;
        bytes.extend(&payload);
        let expected = NormalizedHashEvidence {
            algorithm: RomHashAlgorithm::Sha256,
            expected_hash: hash_bytes(&payload, "", "").sha256.unwrap(),
            expected_size: Some(payload.len() as u64),
            identity_label: None,
        };
        assert_eq!(
            diagnose_rom_representation(&bytes, RomFormatContext::Snes, &expected).equivalence,
            RepresentationEquivalence::Unknown
        );
        assert_eq!(
            diagnose_rom_representation(&bytes, RomFormatContext::Nes, &expected).equivalence,
            RepresentationEquivalence::Equivalent
        );
    }
}
