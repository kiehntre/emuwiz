//! Bounded Human68k/X68000 boot and filesystem evidence.
//!
//! This module deliberately consumes a caller-provided logical sector.  XDF,
//! DIM and D88 container parsing remains in `disk_format`; callers must use
//! those parsers' sector maps rather than guessing flat offsets.  Generic FAT
//! geometry is retained as context, never promoted to an X68000 identity.

use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Human68kEvidenceConfidence {
    Strong,
    Corroborated,
    GenericFat,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Human68kBootEvidence {
    pub boot_sector_valid: bool,
    pub human68k_candidate: bool,
    pub human68k_signature: Option<&'static str>,
    pub filesystem_candidate: Option<&'static str>,
    pub confidence: Human68kEvidenceConfidence,
    pub bytes_per_sector: Option<u16>,
    pub sectors_per_cluster: Option<u8>,
    pub reserved_sectors: Option<u16>,
    pub fat_count: Option<u8>,
    pub root_entries: Option<u16>,
    pub total_sectors: Option<u32>,
    pub media_descriptor: Option<u8>,
    pub sectors_per_track: Option<u16>,
    pub heads: Option<u16>,
    pub hidden_sectors: Option<u32>,
    pub partition_evidence: Option<&'static str>,
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
    let spt = le16(bytes, 0x18);
    let heads = le16(bytes, 0x1a);
    bps == Some(1024)
        && spc.is_power_of_two()
        && spc > 0
        && reserved.is_some_and(|v| v > 0)
        && fats == 2
        && root.is_some_and(|v| v > 0)
        && total.is_some_and(|v| v > 0)
        && spt.is_some_and(|v| v == 8 || v == 9 || v == 15 || v == 18)
        && heads.is_some_and(|v| (1..=2).contains(&v))
}

/// Inspect one bounded logical boot sector.  A strong result requires the
/// X68000 IPL branch opcode and the documented 1024-byte Human68k BPB shape;
/// FAT-like sectors from PC-98, DOS or other machines remain generic/unknown.
pub fn inspect_human68k_boot_sector(bytes: &[u8]) -> Human68kBootEvidence {
    let mut evidence = Human68kBootEvidence {
        boot_sector_valid: false,
        human68k_candidate: false,
        human68k_signature: None,
        filesystem_candidate: None,
        confidence: Human68kEvidenceConfidence::Malformed,
        bytes_per_sector: le16(bytes, 0x0b),
        sectors_per_cluster: bytes.get(0x0d).copied(),
        reserved_sectors: le16(bytes, 0x0e),
        fat_count: bytes.get(0x10).copied(),
        root_entries: le16(bytes, 0x11),
        total_sectors: le16(bytes, 0x13).filter(|v| *v != 0).map(u32::from),
        media_descriptor: bytes.get(0x15).copied(),
        sectors_per_track: le16(bytes, 0x18),
        heads: le16(bytes, 0x1a),
        hidden_sectors: le32(bytes, 0x1c),
        partition_evidence: None,
        reasons: Vec::new(),
        warnings: Vec::new(),
    };
    if bytes.len() < 0x1c {
        evidence
            .warnings
            .push("Human68k IPL/BPB is truncated".into());
        return evidence;
    }
    let ipl = bytes[0] == 0x60;
    if ipl {
        evidence.human68k_signature = Some("X68000 IPL branch (0x60)");
    }
    let bpb = coherent_bpb(bytes);
    if bpb {
        evidence.boot_sector_valid = true;
        evidence.filesystem_candidate = Some("Human68k FAT");
    }
    if ipl && bpb {
        evidence.human68k_candidate = true;
        evidence.confidence = Human68kEvidenceConfidence::Strong;
        evidence
            .reasons
            .push("X68000 IPL branch agrees with the 1024-byte Human68k BPB geometry".into());
    } else if bpb {
        evidence.confidence = Human68kEvidenceConfidence::GenericFat;
        evidence
            .warnings
            .push("coherent 1024-byte FAT geometry lacks an X68000 IPL marker".into());
    } else {
        evidence
            .warnings
            .push("Human68k BPB geometry is incomplete or inconsistent".into());
    }
    evidence
}

/// Project strong structural evidence into the shared, platform-neutral
/// evidence vocabulary.  Exact software identity remains DAT/hash-led.
pub fn content_observations(evidence: &Human68kBootEvidence) -> Vec<ContentEvidence> {
    if evidence.confidence != Human68kEvidenceConfidence::Strong {
        return Vec::new();
    }
    vec![
        ContentEvidence::new(
            ContentEvidenceKind::BootStructure,
            "Human68k IPL/BPB",
            ContentEvidenceConfidence::Strong,
            evidence.reasons.join("; "),
        ),
        ContentEvidence::new(
            ContentEvidenceKind::Filesystem,
            "Human68k FAT",
            ContentEvidenceConfidence::Strong,
            "1024-byte-sector Human68k filesystem candidate",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sector(ipl: bool) -> Vec<u8> {
        let mut b = vec![0; 1024];
        b[0] = if ipl { 0x60 } else { 0x90 };
        b[0x0b..0x0d].copy_from_slice(&1024u16.to_le_bytes());
        b[0x0d] = 1;
        b[0x0e..0x10].copy_from_slice(&2u16.to_le_bytes());
        b[0x10] = 2;
        b[0x11..0x13].copy_from_slice(&192u16.to_le_bytes());
        b[0x13..0x15].copy_from_slice(&1232u16.to_le_bytes());
        b[0x15] = 0xfe;
        b[0x18..0x1a].copy_from_slice(&8u16.to_le_bytes());
        b[0x1a..0x1c].copy_from_slice(&2u16.to_le_bytes());
        b
    }

    #[test]
    fn x68000_ipl_and_bpb_are_strong() {
        let e = inspect_human68k_boot_sector(&sector(true));
        assert_eq!(e.confidence, Human68kEvidenceConfidence::Strong);
        assert!(e.human68k_candidate);
        assert_eq!(content_observations(&e).len(), 2);
    }

    #[test]
    fn generic_1024_byte_fat_is_not_x68000() {
        let e = inspect_human68k_boot_sector(&sector(false));
        assert_eq!(e.confidence, Human68kEvidenceConfidence::GenericFat);
        assert!(!e.human68k_candidate);
        assert!(content_observations(&e).is_empty());
    }

    #[test]
    fn pc98_style_512_byte_fat_is_not_x68000() {
        let mut b = vec![0; 512];
        b[0x0b..0x0d].copy_from_slice(&512u16.to_le_bytes());
        b[0x0d] = 1;
        b[0x0e..0x10].copy_from_slice(&1u16.to_le_bytes());
        b[0x10] = 2;
        b[0x11..0x13].copy_from_slice(&224u16.to_le_bytes());
        b[0x13..0x15].copy_from_slice(&1440u16.to_le_bytes());
        b[0x18..0x1a].copy_from_slice(&9u16.to_le_bytes());
        b[0x1a..0x1c].copy_from_slice(&2u16.to_le_bytes());
        assert_ne!(
            inspect_human68k_boot_sector(&b).confidence,
            Human68kEvidenceConfidence::Strong
        );
    }

    #[test]
    fn truncation_fails_soft() {
        let e = inspect_human68k_boot_sector(&[0; 8]);
        assert!(!e.boot_sector_valid);
        assert!(!e.human68k_candidate);
    }
}
