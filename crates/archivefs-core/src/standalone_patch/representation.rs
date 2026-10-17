//! Representation guidance only; never authorizes a new transform or patch route.
use super::*;
use crate::rom_representation_diagnostics::{
    NormalizedHashEvidence, RepresentationEquivalence, RomFormatContext, RomHashAlgorithm,
    RomRepresentationDiagnostic, diagnose_rom_representation,
};

/// Detailed cartridge diagnostics have a tighter budget than patch application.
pub const MAX_REPRESENTATION_BYTES: usize = 64 * 1024 * 1024;

/// Read-only patch-base explanation. Representation equivalence is retained
/// as separate evidence and never grants patch applicability.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PatchRepresentationPreflight {
    pub match_result: StandalonePatchMatch,
    pub representation: Option<RomRepresentationDiagnostic>,
    pub required_transformation: Option<String>,
}

/// Composes the embedded base CRC with the established platform parser. A
/// normalized match explains the mismatch but remains incompatible until a
/// caller explicitly selects an already-supported safe working-copy path.
pub fn preflight_patch_representation(
    inspection: &StandalonePatchInspection,
    base_bytes: &[u8],
    platform: RomFormatContext,
) -> PatchRepresentationPreflight {
    if inspection.state != PatchInspectionState::Valid
        || base_bytes.len() > MAX_REPRESENTATION_BYTES
    {
        return PatchRepresentationPreflight {
            match_result: match_patch_source(inspection, Some(base_bytes), false),
            representation: None,
            required_transformation: None,
        };
    }
    let Some(expected_crc) = inspection.source_crc32 else {
        return PatchRepresentationPreflight {
            match_result: match_patch_source(inspection, Some(base_bytes), false),
            representation: None,
            required_transformation: None,
        };
    };
    let expected = NormalizedHashEvidence {
        algorithm: RomHashAlgorithm::Crc32,
        expected_hash: format!("{expected_crc:08x}"),
        expected_size: inspection.source_size,
        identity_label: Some("patch embedded source CRC32".into()),
    };
    let mut diagnostic = diagnose_rom_representation(base_bytes, platform, &expected);
    for evidence in &mut diagnostic.evidence {
        evidence.confidence =
            crate::rom_representation_diagnostics::RepresentationDiagnosticConfidence::Weak;
    }
    diagnostic
        .reason
        .push_str("; matching CRC32/size is probable evidence, not cryptographic identity proof");
    let is_equivalent = diagnostic.equivalence == RepresentationEquivalence::Equivalent;
    let transform = diagnostic.normalization_operation.clone();
    PatchRepresentationPreflight {
        match_result: if is_equivalent {
            StandalonePatchMatch {
                compatibility: PatchCompatibility::Incompatible,
                reason: "patch base matches only after representation normalization; explicit safe normalization is required".into(),
            }
        } else {
            match_patch_source(inspection, Some(base_bytes), false)
        },
        representation: Some(diagnostic),
        required_transformation: transform,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inspection(base: &[u8]) -> StandalonePatchInspection {
        StandalonePatchInspection {
            path: "synthetic.bps".into(),
            format: StandalonePatchFormat::Bps,
            state: PatchInspectionState::Valid,
            patch_size: 1,
            patch_sha256: "fixture".into(),
            source_size: Some(base.len() as u64),
            target_size: Some(base.len() as u64),
            source_crc32: Some(crc32(base)),
            target_crc32: Some(crc32(base)),
            patch_crc32: None,
            metadata: None,
            warnings: vec![],
            error: None,
        }
    }
    #[test]
    fn patch_representation_raw_match_is_checksum_evidence_not_strong_identity() {
        let bytes = b"original bytes";
        let before = bytes.to_vec();
        let result =
            preflight_patch_representation(&inspection(bytes), bytes, RomFormatContext::Unknown);
        assert_eq!(
            result.match_result.compatibility,
            PatchCompatibility::Compatible
        );
        assert_eq!(
            result.representation.unwrap().evidence[0].confidence,
            crate::rom_representation_diagnostics::RepresentationDiagnosticConfidence::Weak
        );
        assert_eq!(bytes, before.as_slice());
    }
    #[test]
    fn patch_representation_header_match_never_grants_new_normalization_route() {
        let payload = vec![0x42; 32 * 1024];
        let mut bytes = vec![0xa5; 512];
        bytes.extend(&payload);
        let result =
            preflight_patch_representation(&inspection(&payload), &bytes, RomFormatContext::Snes);
        assert_eq!(
            result.match_result.compatibility,
            PatchCompatibility::Incompatible
        );
        let diagnostic = result.representation.unwrap();
        assert_eq!(
            diagnostic.equivalence,
            RepresentationEquivalence::Equivalent
        );
        assert!(result.required_transformation.is_some());
        assert!(
            !crate::conversion_planner::plan_representation_conversion(diagnostic).apply_supported
        );
        assert_eq!(
            match_patch_source_with_header(&inspection(&payload), Some(&bytes), false, true).1,
            HeaderAdjustment::Strip512ByteCopierHeader
        );
    }
    #[test]
    fn patch_representation_wrong_content_is_not_fixed_by_title_or_header_shape() {
        let payload = vec![0x42; 32 * 1024];
        let mut bytes = vec![0xa5; 512];
        bytes.extend(vec![0x43; 32 * 1024]);
        let result =
            preflight_patch_representation(&inspection(&payload), &bytes, RomFormatContext::Snes);
        assert_eq!(
            result.match_result.compatibility,
            PatchCompatibility::Incompatible
        );
        assert!(result.required_transformation.is_none());
        assert_eq!(
            match_patch_source(&inspection(&payload), Some(&bytes), true).compatibility,
            PatchCompatibility::Incompatible
        );
    }
    #[test]
    fn patch_representation_missing_identity_and_invalid_patch_keep_apply_unproven() {
        let mut i = inspection(b"base");
        i.source_crc32 = None;
        i.source_size = None;
        let result = preflight_patch_representation(&i, b"base", RomFormatContext::Unknown);
        assert_eq!(
            result.match_result.compatibility,
            PatchCompatibility::Unknown
        );
        assert!(result.representation.is_none());
        i = inspection(b"base");
        i.state = PatchInspectionState::Invalid;
        assert!(
            preflight_patch_representation(&i, b"base", RomFormatContext::Nes)
                .representation
                .is_none()
        );
    }
    #[test]
    fn patch_representation_n64_byte_order_is_advice_only() {
        let mut z64 = vec![0; 4096];
        z64[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        let swapped = crate::n64_byte_order::denormalize_from_z64(
            &z64,
            crate::n64_byte_order::N64ByteOrder::V64,
        )
        .unwrap();
        let result =
            preflight_patch_representation(&inspection(&z64), &swapped, RomFormatContext::N64);
        assert_eq!(
            result.match_result.compatibility,
            PatchCompatibility::Incompatible
        );
        assert_eq!(
            result.representation.unwrap().kind,
            crate::rom_representation_diagnostics::RomRepresentationKind::ByteOrderEquivalent
        );
    }
}
