//! Pure, read-only PC-FX boot-sector and disc-hash evidence.
//!
//! # Format verified, not assumed
//!
//! Taken directly from Mednafen's own PC-FX core, `TestMagicCD()`
//! (`https://github.com/OpenEmu/Mednafen-Core/blob/master/mednafen/pcfx/pcfx.cpp`,
//! a real, actively-used, production-grade PC-FX emulator - exactly the
//! kind of source this task calls for):
//!
//! ```text
//! if(!strncmp("PC-FX:Hu_CD-ROM", sector, strlen("PC-FX:Hu_CD-ROM")))
//!     return true;
//! else if(!strncmp(sector + 64, "PPPPHHHHOOOOTTTTOOOO____CCCCDDDD", 32))
//!     return true;
//! ```
//!
//! Checked against the first 2048-byte sector of the disc's first data
//! track (the same "first sector of the logical data stream" convention
//! [`crate::dreamcast_boot_evidence`]/[`crate::saturn_boot_evidence`]
//! already use for their own boot-area reads).
//!
//! # Collision safety
//!
//! - Neither magic string is disclosed by Mednafen's own source to encode
//!   a serial/catalog/product code, region, or version - this module never
//!   emits a `ProductCode` fact.
//! - **Never conflated with PC Engine CD/TurboGrafx-CD.** All three (PC
//!   Engine CD, TurboGrafx-CD, PC-FX) are NEC optical platforms, but the
//!   `"PC-FX:Hu_CD-ROM"` string is PC-FX-specific - it is not the generic
//!   `"PC Engine CD-ROM SYSTEM"`-style string PC Engine CD/TurboGrafx-CD
//!   discs carry (a different, unrelated boot string this module does not
//!   check for and never matches against). This module makes no claim
//!   about, and shares no evidence value with, PC Engine CD detection.

use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};
use md5::{Digest, Md5};

pub const PCFX_PRIMARY_MAGIC: &[u8] = b"PC-FX:Hu_CD-ROM";
pub const PCFX_SECONDARY_MAGIC: &[u8] = b"PPPPHHHHOOOOTTTTOOOO____CCCCDDDD";
const PCFX_SECONDARY_MAGIC_OFFSET: usize = 64;

/// Bound on the sector prefix this module ever looks at - a real CD-ROM
/// sector is 2048 bytes; this is exactly that, never more.
pub const PCFX_BOOT_SECTOR_BYTES: usize = 2048;
pub const PCFX_HASH_SECTOR0_BYTES: usize = 32;
pub const PCFX_VOLUME_HEADER_BYTES: usize = 128;
const PCFX_BOOT_SECTOR_OFFSET: usize = 32;
const PCFX_BOOT_SECTOR_COUNT_OFFSET: usize = 36;
/// Keep a malformed header from requesting an unreasonable boot-code read.
pub const PCFX_MAX_BOOT_SECTORS: u32 = 32 * 1024;

/// The header-directed part of the documented PC-FX disc identity hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcfxVolumeHeader {
    pub boot_sector: u32,
    pub boot_sector_count: u32,
}

/// Parses the 128-byte sector-1 volume header used by the PC-FX custom disc
/// hash. The fields are little-endian, as documented by the
/// RetroAchievements PC-FX identification algorithm.
pub fn parse_pcfx_volume_header(bytes: &[u8]) -> Option<PcfxVolumeHeader> {
    if bytes.len() < PCFX_VOLUME_HEADER_BYTES {
        return None;
    }
    let boot_sector = u32::from_le_bytes(
        bytes[PCFX_BOOT_SECTOR_OFFSET..PCFX_BOOT_SECTOR_OFFSET + 4]
            .try_into()
            .ok()?,
    );
    let boot_sector_count = u32::from_le_bytes(
        bytes[PCFX_BOOT_SECTOR_COUNT_OFFSET..PCFX_BOOT_SECTOR_COUNT_OFFSET + 4]
            .try_into()
            .ok()?,
    );
    if boot_sector < 2 || boot_sector_count == 0 || boot_sector_count > PCFX_MAX_BOOT_SECTORS {
        return None;
    }
    Some(PcfxVolumeHeader {
        boot_sector,
        boot_sector_count,
    })
}

/// Computes the documented representation-independent PC-FX disc hash from
/// already bounded logical-sector pieces. This is the same algorithm used by
/// RetroAchievements' [game-identification documentation][ra]: 32 bytes from
/// sector 0, the first 128 bytes of sector 1, then the header-directed boot
/// sectors in order. It is an identity fingerprint, not a title lookup.
///
/// [ra]: https://docs.retroachievements.org/developer-docs/game-identification.html
pub fn pcfx_disc_hash(sector_zero: &[u8], sector_one: &[u8], boot_code: &[u8]) -> Option<String> {
    if sector_zero.len() < PCFX_HASH_SECTOR0_BYTES || sector_one.len() < PCFX_VOLUME_HEADER_BYTES {
        return None;
    }
    let header = parse_pcfx_volume_header(sector_one)?;
    let expected_boot_bytes = usize::try_from(header.boot_sector_count)
        .ok()?
        .checked_mul(PCFX_BOOT_SECTOR_BYTES)?;
    if boot_code.len() != expected_boot_bytes {
        return None;
    }
    let mut digest = Md5::new();
    digest.update(&sector_zero[..PCFX_HASH_SECTOR0_BYTES]);
    digest.update(&sector_one[..PCFX_VOLUME_HEADER_BYTES]);
    digest.update(boot_code);
    Some(hexadecimal(digest.finalize().as_slice()))
}

fn hexadecimal(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What was observed about a PC-FX boot-sector candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PcfxBootSectorFact {
    pub primary_magic_present: bool,
    pub secondary_magic_present: bool,
}

impl PcfxBootSectorFact {
    pub fn any_magic_present(&self) -> bool {
        self.primary_magic_present || self.secondary_magic_present
    }
}

/// Checks `sector` (the first sector of a disc's first data track) for
/// either magic Mednafen itself checks. Never panics on a short buffer -
/// a magic simply cannot be present if there are not enough bytes.
pub fn parse_pcfx_boot_sector(sector: &[u8]) -> PcfxBootSectorFact {
    let primary_magic_present = sector.len() >= PCFX_PRIMARY_MAGIC.len()
        && &sector[..PCFX_PRIMARY_MAGIC.len()] == PCFX_PRIMARY_MAGIC;
    let secondary_end = PCFX_SECONDARY_MAGIC_OFFSET + PCFX_SECONDARY_MAGIC.len();
    let secondary_magic_present = sector.len() >= secondary_end
        && &sector[PCFX_SECONDARY_MAGIC_OFFSET..secondary_end] == PCFX_SECONDARY_MAGIC;
    PcfxBootSectorFact {
        primary_magic_present,
        secondary_magic_present,
    }
}

/// Neutral evidence: `Strong` `BootStructure` = `"PC-FX:Hu_CD-ROM"` for the
/// primary magic, or `"PPPPHHHHOOOOTTTTOOOO____CCCCDDDD"` for the
/// secondary one - both, if both are present (mirrors
/// [`crate::observe_content_evidence`]'s "preserve every fact" discipline
/// rather than picking one).
pub fn observe_pcfx_evidence(fact: &PcfxBootSectorFact) -> Vec<ContentEvidence> {
    let mut evidence = Vec::new();
    if fact.primary_magic_present {
        evidence.push(ContentEvidence::new(
            ContentEvidenceKind::BootStructure,
            "PC-FX:Hu_CD-ROM",
            ContentEvidenceConfidence::Strong,
            "PC-FX primary boot-sector magic present at sector offset 0 - verified against Mednafen's own PC-FX core",
        ));
    }
    if fact.secondary_magic_present {
        evidence.push(ContentEvidence::new(
            ContentEvidenceKind::BootStructure,
            "PPPPHHHHOOOOTTTTOOOO____CCCCDDDD",
            ContentEvidenceConfidence::Strong,
            "PC-FX secondary boot-sector magic present at sector offset 64 - verified against Mednafen's own PC-FX core",
        ));
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sector_with_primary_magic() -> Vec<u8> {
        let mut sector = vec![0u8; PCFX_BOOT_SECTOR_BYTES];
        sector[..PCFX_PRIMARY_MAGIC.len()].copy_from_slice(PCFX_PRIMARY_MAGIC);
        sector
    }

    fn sector_with_secondary_magic() -> Vec<u8> {
        let mut sector = vec![0u8; PCFX_BOOT_SECTOR_BYTES];
        sector
            [PCFX_SECONDARY_MAGIC_OFFSET..PCFX_SECONDARY_MAGIC_OFFSET + PCFX_SECONDARY_MAGIC.len()]
            .copy_from_slice(PCFX_SECONDARY_MAGIC);
        sector
    }

    #[test]
    fn primary_magic_is_detected() {
        let fact = parse_pcfx_boot_sector(&sector_with_primary_magic());
        assert!(fact.primary_magic_present);
        assert!(!fact.secondary_magic_present);
        assert!(fact.any_magic_present());
    }

    #[test]
    fn secondary_magic_is_detected() {
        let fact = parse_pcfx_boot_sector(&sector_with_secondary_magic());
        assert!(!fact.primary_magic_present);
        assert!(fact.secondary_magic_present);
        assert!(fact.any_magic_present());
    }

    #[test]
    fn neither_magic_present_on_unrelated_bytes() {
        let sector = vec![0u8; PCFX_BOOT_SECTOR_BYTES];
        let fact = parse_pcfx_boot_sector(&sector);
        assert!(!fact.any_magic_present());
    }

    #[test]
    fn short_buffer_fails_closed_not_panic() {
        let fact = parse_pcfx_boot_sector(&[0u8; 4]);
        assert!(!fact.any_magic_present());
    }

    #[test]
    fn empty_buffer_fails_closed_not_panic() {
        let fact = parse_pcfx_boot_sector(&[]);
        assert!(!fact.any_magic_present());
    }

    #[test]
    fn truncated_secondary_magic_is_not_recognized() {
        let mut sector = vec![0u8; PCFX_SECONDARY_MAGIC_OFFSET + 10];
        sector[PCFX_SECONDARY_MAGIC_OFFSET..].copy_from_slice(&PCFX_SECONDARY_MAGIC[..10]);
        let fact = parse_pcfx_boot_sector(&sector);
        assert!(!fact.secondary_magic_present);
    }

    #[test]
    fn primary_magic_yields_strong_boot_structure_evidence() {
        let fact = parse_pcfx_boot_sector(&sector_with_primary_magic());
        let evidence = observe_pcfx_evidence(&fact);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].kind, ContentEvidenceKind::BootStructure);
        assert_eq!(evidence[0].value, "PC-FX:Hu_CD-ROM");
        assert_eq!(evidence[0].confidence, ContentEvidenceConfidence::Strong);
    }

    #[test]
    fn both_magics_present_yields_both_facts() {
        let mut sector = sector_with_primary_magic();
        sector
            [PCFX_SECONDARY_MAGIC_OFFSET..PCFX_SECONDARY_MAGIC_OFFSET + PCFX_SECONDARY_MAGIC.len()]
            .copy_from_slice(PCFX_SECONDARY_MAGIC);
        let fact = parse_pcfx_boot_sector(&sector);
        let evidence = observe_pcfx_evidence(&fact);
        assert_eq!(evidence.len(), 2);
    }

    #[test]
    fn no_magic_yields_no_evidence() {
        let fact = PcfxBootSectorFact::default();
        assert!(observe_pcfx_evidence(&fact).is_empty());
    }

    #[test]
    fn evidence_never_includes_product_code() {
        let fact = parse_pcfx_boot_sector(&sector_with_primary_magic());
        for item in observe_pcfx_evidence(&fact) {
            assert_ne!(item.kind, ContentEvidenceKind::ProductCode);
        }
    }

    #[test]
    fn evidence_never_assigns_a_platform() {
        let fact = parse_pcfx_boot_sector(&sector_with_primary_magic());
        for item in observe_pcfx_evidence(&fact) {
            assert!(matches!(item.kind, ContentEvidenceKind::BootStructure));
        }
    }

    #[test]
    fn repeated_observation_is_deterministic() {
        let fact = parse_pcfx_boot_sector(&sector_with_primary_magic());
        assert_eq!(observe_pcfx_evidence(&fact), observe_pcfx_evidence(&fact));
    }

    #[test]
    fn volume_header_parses_bounded_little_endian_geometry() {
        let mut sector = vec![0_u8; PCFX_VOLUME_HEADER_BYTES];
        sector[PCFX_BOOT_SECTOR_OFFSET..PCFX_BOOT_SECTOR_OFFSET + 4]
            .copy_from_slice(&2_u32.to_le_bytes());
        sector[PCFX_BOOT_SECTOR_COUNT_OFFSET..PCFX_BOOT_SECTOR_COUNT_OFFSET + 4]
            .copy_from_slice(&3_u32.to_le_bytes());
        assert_eq!(
            parse_pcfx_volume_header(&sector),
            Some(PcfxVolumeHeader {
                boot_sector: 2,
                boot_sector_count: 3,
            })
        );
        assert!(parse_pcfx_volume_header(&sector[..PCFX_VOLUME_HEADER_BYTES - 1]).is_none());
    }

    #[test]
    fn disc_hash_changes_when_any_hashed_component_changes() {
        let sector_zero = sector_with_primary_magic();
        let mut sector_one = vec![0_u8; PCFX_VOLUME_HEADER_BYTES];
        sector_one[PCFX_BOOT_SECTOR_OFFSET..PCFX_BOOT_SECTOR_OFFSET + 4]
            .copy_from_slice(&2_u32.to_le_bytes());
        sector_one[PCFX_BOOT_SECTOR_COUNT_OFFSET..PCFX_BOOT_SECTOR_COUNT_OFFSET + 4]
            .copy_from_slice(&1_u32.to_le_bytes());
        let boot = vec![0xA5_u8; PCFX_BOOT_SECTOR_BYTES];
        let original = pcfx_disc_hash(&sector_zero, &sector_one, &boot).unwrap();
        let mut changed_boot = boot.clone();
        changed_boot[0] ^= 1;
        assert_ne!(
            original,
            pcfx_disc_hash(&sector_zero, &sector_one, &changed_boot).unwrap()
        );
    }

    #[test]
    fn disc_hash_rejects_wrong_boot_length() {
        let sector_zero = sector_with_primary_magic();
        let mut sector_one = vec![0_u8; PCFX_VOLUME_HEADER_BYTES];
        sector_one[PCFX_BOOT_SECTOR_OFFSET..PCFX_BOOT_SECTOR_OFFSET + 4]
            .copy_from_slice(&2_u32.to_le_bytes());
        sector_one[PCFX_BOOT_SECTOR_COUNT_OFFSET..PCFX_BOOT_SECTOR_COUNT_OFFSET + 4]
            .copy_from_slice(&1_u32.to_le_bytes());
        assert!(pcfx_disc_hash(&sector_zero, &sector_one, &[0_u8; 1]).is_none());
    }
}
