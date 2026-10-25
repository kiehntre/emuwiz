//! Conservative FM Towns IPL4/TownsOS boot evidence.
//!
//! This is deliberately narrower than a filesystem parser.  The FM Towns ROM
//! recognises an IPL boot sector by the exact `IPL4` signature at offset zero,
//! followed by an x86 transfer instruction.  A coherent BPB is retained as
//! filesystem evidence, but generic FAT geometry never produces an FM Towns
//! claim by itself.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FmTownsEvidenceConfidence {
    Strong,
    GenericFat,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FmTownsBootEvidence {
    pub boot_sector_valid: bool,
    pub ipl4_present: bool,
    pub ipl_signature: Option<&'static str>,
    pub townsos_candidate: bool,
    pub confidence: FmTownsEvidenceConfidence,
    pub bytes_per_sector: Option<u16>,
    pub sectors_per_cluster: Option<u8>,
    pub reserved_sectors: Option<u16>,
    pub fat_count: Option<u8>,
    pub root_entries: Option<u16>,
    pub total_sectors: Option<u32>,
    pub media_descriptor: Option<u8>,
    pub sectors_per_track: Option<u16>,
    pub heads: Option<u16>,
    pub filesystem_candidate: Option<&'static str>,
    pub io_sys_start_sector: Option<u32>,
    pub io_sys_sector_count: Option<u32>,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
}

fn le16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
    ]))
}

fn le32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
        *bytes.get(offset + 2)?,
        *bytes.get(offset + 3)?,
    ]))
}

fn coherent_bpb(bytes: &[u8]) -> bool {
    let bps = le16(bytes, 0x0b);
    let spc = bytes.get(0x0d).copied().unwrap_or(0);
    let reserved = le16(bytes, 0x0e);
    let fats = bytes.get(0x10).copied().unwrap_or(0);
    let root = le16(bytes, 0x11);
    let total = le16(bytes, 0x13).filter(|v| *v != 0).map(u32::from);
    let media = bytes.get(0x15).copied().unwrap_or(0);
    let spt = le16(bytes, 0x18);
    let heads = le16(bytes, 0x1a);
    bps.is_some_and(|v| matches!(v, 512 | 1024))
        && spc.is_power_of_two()
        && spc > 0
        && reserved.is_some_and(|v| v > 0)
        && (1..=2).contains(&fats)
        && root.is_some_and(|v| v > 0)
        && total.is_some_and(|v| v > 0)
        && media != 0
        && spt.is_some_and(|v| (8..=36).contains(&v))
        && heads.is_some_and(|v| (1..=16).contains(&v))
}

/// Inspect a caller-provided logical boot sector. The caller controls the
/// read size; this function never reads beyond the supplied bounded slice.
pub fn inspect_fmtowns_boot_sector(bytes: &[u8]) -> FmTownsBootEvidence {
    let mut evidence = FmTownsBootEvidence {
        boot_sector_valid: false,
        ipl4_present: false,
        ipl_signature: None,
        townsos_candidate: false,
        confidence: FmTownsEvidenceConfidence::Malformed,
        bytes_per_sector: le16(bytes, 0x0b),
        sectors_per_cluster: bytes.get(0x0d).copied(),
        reserved_sectors: le16(bytes, 0x0e),
        fat_count: bytes.get(0x10).copied(),
        root_entries: le16(bytes, 0x11),
        total_sectors: le16(bytes, 0x13).filter(|v| *v != 0).map(u32::from),
        media_descriptor: bytes.get(0x15).copied(),
        sectors_per_track: le16(bytes, 0x18),
        heads: le16(bytes, 0x1a),
        filesystem_candidate: None,
        io_sys_start_sector: le32(bytes, 0x20),
        io_sys_sector_count: le32(bytes, 0x24),
        reasons: Vec::new(),
        warnings: Vec::new(),
    };

    if bytes.len() < 0x28 {
        evidence
            .warnings
            .push("IPL4/BPB structure is truncated".into());
        return evidence;
    }

    evidence.ipl4_present = bytes.get(0..4) == Some(b"IPL4");
    if evidence.ipl4_present {
        evidence.ipl_signature = Some("IPL4");
    }
    let transfer = bytes[4];
    let transfer_valid = matches!(transfer, 0xe9 | 0xeb);

    if coherent_bpb(bytes) {
        evidence.filesystem_candidate = Some(if evidence.total_sectors.unwrap_or(0) < 4096 {
            "FAT12"
        } else {
            "FAT16"
        });
    }

    if evidence.ipl4_present && transfer_valid {
        evidence.boot_sector_valid = true;
        evidence.confidence = FmTownsEvidenceConfidence::Strong;
        evidence
            .reasons
            .push("exact FM Towns IPL4 signature followed by a valid x86 boot transfer".into());
        if coherent_bpb(bytes) {
            evidence
                .reasons
                .push("coherent 512/1024-byte FAT BPB corroborates boot media".into());
        } else {
            evidence
                .warnings
                .push("no coherent FAT BPB was available; filesystem remains unknown".into());
        }
        let start = evidence.io_sys_start_sector.unwrap_or(0);
        let count = evidence.io_sys_sector_count.unwrap_or(0);
        if start > 0
            && count > 0
            && evidence
                .total_sectors
                .is_some_and(|total| start.checked_add(count).is_some_and(|end| end <= total))
        {
            evidence.townsos_candidate = true;
            evidence
                .reasons
                .push("TownsOS IO.SYS sector range is bounded by the boot BPB".into());
        }
    } else if !evidence.ipl4_present && coherent_bpb(bytes) {
        evidence.confidence = FmTownsEvidenceConfidence::GenericFat;
        evidence
            .warnings
            .push("generic FAT geometry has no valid FM Towns IPL4 evidence".into());
    } else {
        evidence
            .warnings
            .push("IPL4 signature or boot transfer structure is malformed".into());
    }
    evidence
}

/// Convert only strong IPL4 evidence into the existing discovery lineage.
pub fn structural_observation(
    evidence: &FmTownsBootEvidence,
) -> Option<crate::platform_evidence_fusion::evidence_lineage::EvidenceObservation> {
    if evidence.confidence != FmTownsEvidenceConfidence::Strong {
        return None;
    }
    use crate::platform_evidence_fusion::evidence_lineage::{
        ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope,
        LineageRelation, Provenance, Representation, SourceFamily,
    };
    Some(EvidenceObservation {
        provenance: Provenance {
            channel: EvidenceChannel::LocalStructural,
            upstream_source: SourceFamily::Unknown,
            upstream_version: None,
            source_artifact: None,
            imported_at_unix: None,
            retrieved_at_unix: None,
            generator_version: None,
            lineage: LineageRelation::Independent,
            representation: Representation::StructuralMetadata,
        },
        claim: ClaimType::PlatformCandidate,
        claim_strength: ClaimStrength::Strong,
        identity_scope: IdentityScope::PlatformIdentity,
        hash_or_value: None,
        platform_candidate: Some("FM Towns".to_string()),
        release_candidate: None,
        notes: Some(evidence.reasons.join("; ")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sector() -> Vec<u8> {
        let mut b = vec![0; 512];
        b[0..4].copy_from_slice(b"IPL4");
        b[4] = 0xeb;
        b[0x0b..0x0d].copy_from_slice(&512u16.to_le_bytes());
        b[0x0d] = 1;
        b[0x0e..0x10].copy_from_slice(&1u16.to_le_bytes());
        b[0x10] = 2;
        b[0x11..0x13].copy_from_slice(&224u16.to_le_bytes());
        b[0x13..0x15].copy_from_slice(&1440u16.to_le_bytes());
        b[0x15] = 0xf0;
        b[0x18..0x1a].copy_from_slice(&9u16.to_le_bytes());
        b[0x1a..0x1c].copy_from_slice(&2u16.to_le_bytes());
        b[0x20..0x24].copy_from_slice(&10u32.to_le_bytes());
        b[0x24..0x28].copy_from_slice(&2u32.to_le_bytes());
        b
    }

    #[test]
    fn valid_ipl4_is_strong_and_townsos_candidate() {
        let e = inspect_fmtowns_boot_sector(&sector());
        assert_eq!(e.confidence, FmTownsEvidenceConfidence::Strong);
        assert!(e.boot_sector_valid && e.ipl4_present && e.townsos_candidate);
        assert!(structural_observation(&e).is_some());
    }

    #[test]
    fn generic_fat_is_not_fm_towns() {
        let mut b = sector();
        b[0..4].copy_from_slice(b"DOS ");
        let e = inspect_fmtowns_boot_sector(&b);
        assert_eq!(e.confidence, FmTownsEvidenceConfidence::GenericFat);
        assert!(structural_observation(&e).is_none());
    }

    #[test]
    fn pc98_x68000_and_geometry_only_evidence_stays_non_towns() {
        for marker in [b"NEC     ".as_slice(), b"X68K    ".as_slice()] {
            let mut b = sector();
            b[3..11].copy_from_slice(marker);
            b[0..4].fill(0);
            assert_eq!(
                inspect_fmtowns_boot_sector(&b).confidence,
                FmTownsEvidenceConfidence::GenericFat
            );
        }
        let mut geometry = sector();
        geometry[0..4].fill(0);
        geometry[4] = 0;
        assert!(structural_observation(&inspect_fmtowns_boot_sector(&geometry)).is_none());
    }

    #[test]
    fn malformed_ipl_near_match_and_names_do_not_upgrade_identity() {
        let mut near = sector();
        near[0..4].copy_from_slice(b"IPL3");
        assert!(structural_observation(&inspect_fmtowns_boot_sector(&near)).is_none());
        let mut extension_named = vec![0u8; 512];
        extension_named[0..4].copy_from_slice(b"DOS ");
        assert!(structural_observation(&inspect_fmtowns_boot_sector(&extension_named)).is_none());
    }

    #[test]
    fn malformed_and_truncated_fail_soft() {
        let mut b = sector();
        b[4] = 0;
        assert_eq!(
            inspect_fmtowns_boot_sector(&b).confidence,
            FmTownsEvidenceConfidence::Malformed
        );
        assert!(!inspect_fmtowns_boot_sector(&[0; 8]).boot_sector_valid);
    }
}
