//! Conservative PC-98 boot-sector evidence.
//!
//! D88/HDI/NHD container validation remains in `disk_format`. This module only
//! evaluates a caller-provided logical 512-byte boot sector; it never guesses
//! flat offsets or treats a container extension as a machine identity.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pc98EvidenceConfidence {
    Strong,
    Corroborated,
    GenericFat,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pc98BootEvidence {
    pub boot_sector_valid: bool,
    pub confidence: Pc98EvidenceConfidence,
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
    pub filesystem_candidate: Option<&'static str>,
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

/// Inspect one logical PC-compatible boot sector. A PC-98 conclusion requires
/// the Japanese NEC OEM marker plus internally coherent FAT geometry; generic
/// FAT sectors are returned as `GenericFat` and never upgraded by geometry.
pub fn inspect_pc98_boot_sector(bytes: &[u8]) -> Pc98BootEvidence {
    let mut evidence = Pc98BootEvidence {
        boot_sector_valid: false,
        confidence: Pc98EvidenceConfidence::Malformed,
        bytes_per_sector: None,
        sectors_per_cluster: None,
        reserved_sectors: None,
        fat_count: None,
        root_entries: None,
        total_sectors: None,
        media_descriptor: None,
        sectors_per_track: None,
        heads: None,
        hidden_sectors: None,
        filesystem_candidate: None,
        reasons: Vec::new(),
        warnings: Vec::new(),
    };
    if bytes.len() < 512 {
        evidence.warnings.push("boot sector is truncated".into());
        return evidence;
    }
    let bps = le16(bytes, 0x0b);
    let spc = bytes[0x0d];
    let reserved = le16(bytes, 0x0e);
    let fats = bytes[0x10];
    let root = le16(bytes, 0x11);
    let short_total = le16(bytes, 0x13).unwrap_or(0);
    let media = bytes[0x15];
    let spt = le16(bytes, 0x18);
    let heads = le16(bytes, 0x1a);
    let hidden = le32(bytes, 0x1c);
    let total = if short_total != 0 {
        Some(u32::from(short_total))
    } else {
        le32(bytes, 0x20)
    };
    evidence.bytes_per_sector = bps;
    evidence.sectors_per_cluster = Some(spc);
    evidence.reserved_sectors = reserved;
    evidence.fat_count = Some(fats);
    evidence.root_entries = root;
    evidence.total_sectors = total;
    evidence.media_descriptor = Some(media);
    evidence.sectors_per_track = spt;
    evidence.heads = heads;
    evidence.hidden_sectors = hidden;

    let coherent = bps == Some(512)
        && spc.is_power_of_two()
        && spc > 0
        && reserved.is_some_and(|v| v > 0)
        && fats > 0
        && root.is_some_and(|v| v > 0)
        && total.is_some_and(|v| v > 0)
        && spt.is_some_and(|v| matches!(v, 8 | 9 | 15 | 18))
        && heads.is_some_and(|v| (1..=4).contains(&v));
    if !coherent {
        evidence.confidence = Pc98EvidenceConfidence::Malformed;
        evidence
            .warnings
            .push("FAT/BPB geometry is internally inconsistent".into());
        return evidence;
    }
    evidence.boot_sector_valid = true;
    evidence.filesystem_candidate = Some(if total.unwrap_or(0) < 4096 {
        "FAT12"
    } else {
        "FAT16"
    });
    let oem = &bytes[3..11];
    if oem.starts_with(b"NEC") {
        evidence.confidence = Pc98EvidenceConfidence::Strong;
        evidence
            .reasons
            .push("NEC OEM identifier agrees with coherent Japanese FAT geometry".into());
    } else {
        evidence.confidence = Pc98EvidenceConfidence::GenericFat;
        evidence
            .warnings
            .push("valid FAT geometry has no PC-98-specific OEM/IPL marker".into());
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sector(oem: &[u8]) -> Vec<u8> {
        let mut b = vec![0; 512];
        b[3..11].copy_from_slice(&[oem, b"       "].concat()[..8]);
        b[0x0b..0x0d].copy_from_slice(&512u16.to_le_bytes());
        b[0x0d] = 1;
        b[0x0e..0x10].copy_from_slice(&1u16.to_le_bytes());
        b[0x10] = 2;
        b[0x11..0x13].copy_from_slice(&224u16.to_le_bytes());
        b[0x13..0x15].copy_from_slice(&1440u16.to_le_bytes());
        b[0x15] = 0xf0;
        b[0x18..0x1a].copy_from_slice(&9u16.to_le_bytes());
        b[0x1a..0x1c].copy_from_slice(&2u16.to_le_bytes());
        b
    }

    #[test]
    fn nec_bpb_is_strong_pc98_evidence() {
        let e = inspect_pc98_boot_sector(&sector(b"NEC     "));
        assert_eq!(e.confidence, Pc98EvidenceConfidence::Strong);
    }

    #[test]
    fn generic_fat_stays_generic() {
        let e = inspect_pc98_boot_sector(&sector(b"MSDOS5.0"));
        assert_eq!(e.confidence, Pc98EvidenceConfidence::GenericFat);
    }

    #[test]
    fn truncation_fails_soft() {
        let e = inspect_pc98_boot_sector(&[0; 16]);
        assert!(!e.boot_sector_valid);
    }
}
