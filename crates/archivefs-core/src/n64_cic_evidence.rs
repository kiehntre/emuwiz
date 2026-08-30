//! Nintendo 64 CIC-NUS bootcode identification and CRC1/CRC2 validation.
//!
//! All functions in this module require canonical `Z64` (big-endian) bytes.
//! The bootcode lookup is bounded to the 0xFC0-byte IPL3 region at
//! `[0x40, 0x1000)`. The five lookup values below are independently recorded
//! by z64decompress's `n64crc.c` and the n64checksum research/reference
//! implementation. PAL counterparts are retained in the variant names where
//! the same bootcode is shared; bootcode evidence alone cannot distinguish
//! those regional chip labels.
//!
//! Sources:
//! - https://github.com/z64dev/z64decompress/blob/main/src/n64crc.c
//! - https://github.com/Dragorn421/n64checksum/blob/main/README.md
//! - https://github.com/Decompollaborate/ipl3checksum

use crate::identity_source::hashing::Crc32;
use crate::n64_header_evidence::N64HeaderFact;

pub const N64_BOOTCODE_START: usize = 0x40;
pub const N64_BOOTCODE_END: usize = 0x1000;
pub const N64_CRC_DATA_START: usize = 0x1000;
pub const N64_CRC_DATA_LEN: usize = 0x100000;
pub const N64_MIN_CRC_BYTES: usize = N64_CRC_DATA_START + N64_CRC_DATA_LEN;

/// CIC label(s) that share one verified IPL3 bootcode fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CicVariant {
    Nus6101,
    Nus6102Or7101,
    Nus6103Or7103,
    Nus6105Or7105,
    Nus6106Or7106,
}

impl CicVariant {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nus6101 => "CIC-NUS-6101",
            Self::Nus6102Or7101 => "CIC-NUS-6102/7101",
            Self::Nus6103Or7103 => "CIC-NUS-6103/7103",
            Self::Nus6105Or7105 => "CIC-NUS-6105/7105",
            Self::Nus6106Or7106 => "CIC-NUS-6106/7106",
        }
    }

    fn checksum_seed(self) -> u32 {
        match self {
            Self::Nus6101 | Self::Nus6102Or7101 => 0xF8CA_4DDC,
            Self::Nus6103Or7103 => 0xA388_6759,
            Self::Nus6105Or7105 => 0xDF26_F436,
            Self::Nus6106Or7106 => 0x1FEA_617A,
        }
    }
}

/// Identifies a supported CIC from canonical IPL3 bootcode CRC32 evidence.
/// Returns `None` for unknown bootcode and never defaults to 6102.
pub fn cic_lookup(canonical_z64: &[u8]) -> Option<CicVariant> {
    if canonical_z64.len() < N64_BOOTCODE_END {
        return None;
    }
    let bootcode_crc = Crc32::of(&canonical_z64[N64_BOOTCODE_START..N64_BOOTCODE_END]);
    cic_from_bootcode_crc(&bootcode_crc)
}

fn cic_from_bootcode_crc(bootcode_crc: &str) -> Option<CicVariant> {
    match bootcode_crc {
        "6170a4a1" => Some(CicVariant::Nus6101),
        "90bb6cb5" => Some(CicVariant::Nus6102Or7101),
        "0b050ee0" => Some(CicVariant::Nus6103Or7103),
        "98bc2c86" => Some(CicVariant::Nus6105Or7105),
        "acc8580a" => Some(CicVariant::Nus6106Or7106),
        _ => None,
    }
}

/// Computes the expected CRC1/CRC2 pair for a supported CIC.
///
/// This is the IPL3 checksum over the first 1 MiB after the header/bootcode.
/// It deliberately refuses short ROMs instead of padding, truncating, or
/// guessing. The caller must pass canonical Z64 bytes.
pub fn compute_crc1_crc2(canonical_z64: &[u8], cic: CicVariant) -> Option<(u32, u32)> {
    if canonical_z64.len() < N64_MIN_CRC_BYTES {
        return None;
    }
    let seed = cic.checksum_seed();
    let mut t1 = seed;
    let mut t2 = seed;
    let mut t3 = seed;
    let mut t4 = seed;
    let mut t5 = seed;
    let mut t6 = seed;
    let mut offset = N64_CRC_DATA_START;
    while offset < N64_MIN_CRC_BYTES {
        let d = u32::from_be_bytes(canonical_z64[offset..offset + 4].try_into().ok()?);
        if t6.wrapping_add(d) < t6 {
            t4 = t4.wrapping_add(1);
        }
        t6 = t6.wrapping_add(d);
        t3 ^= d;
        let rotation = d & 0x1f;
        let r = d.rotate_left(rotation);
        t5 = t5.wrapping_add(r);
        if t2 > d {
            t2 ^= r;
        } else {
            t2 ^= t6 ^ d;
        }
        if cic == CicVariant::Nus6105Or7105 {
            let bootcode_word = N64_BOOTCODE_START + 0x710 + (offset & 0xff);
            let bootcode_d = u32::from_be_bytes(
                canonical_z64[bootcode_word..bootcode_word + 4]
                    .try_into()
                    .ok()?,
            );
            t1 = t1.wrapping_add(bootcode_d ^ d);
        } else {
            t1 = t1.wrapping_add(t5 ^ d);
        }
        offset += 4;
    }
    let (crc1, crc2) = match cic {
        CicVariant::Nus6103Or7103 => ((t6 ^ t4).wrapping_add(t3), (t5 ^ t2).wrapping_add(t1)),
        CicVariant::Nus6106Or7106 => (
            (t6.wrapping_mul(t4)).wrapping_add(t3),
            (t5.wrapping_mul(t2)).wrapping_add(t1),
        ),
        _ => (t6 ^ t4 ^ t3, t5 ^ t2 ^ t1),
    };
    Some((crc1, crc2))
}

/// Validates header CRC1/CRC2 against a known CIC and canonical ROM bytes.
pub fn validate_crc1_crc2(
    canonical_z64: &[u8],
    header: &N64HeaderFact,
    cic: CicVariant,
) -> Option<bool> {
    let (crc1, crc2) = compute_crc1_crc2(canonical_z64, cic)?;
    Some(header.crc1 == crc1 && header.crc2 == crc2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_bootcode_crc_fixtures_map_without_a_default() {
        let fixtures = [
            ("6170a4a1", CicVariant::Nus6101),
            ("90bb6cb5", CicVariant::Nus6102Or7101),
            ("0b050ee0", CicVariant::Nus6103Or7103),
            ("98bc2c86", CicVariant::Nus6105Or7105),
            ("acc8580a", CicVariant::Nus6106Or7106),
        ];
        for (crc, expected) in fixtures {
            assert_eq!(cic_from_bootcode_crc(crc), Some(expected));
        }
        assert_eq!(cic_from_bootcode_crc("00000000"), None);
    }

    #[test]
    fn canonical_lookup_rejects_truncated_and_unknown_bootcode() {
        assert_eq!(cic_lookup(&[0; N64_BOOTCODE_END - 1]), None);
        assert_eq!(cic_lookup(&[0; N64_BOOTCODE_END]), None);
    }

    #[test]
    fn crc_validation_requires_a_full_first_megabyte() {
        let header = N64HeaderFact {
            clock_rate: 0,
            boot_address: 0,
            release: 0,
            crc1: 0,
            crc2: 0,
            image_name: String::new(),
            manufacturer_id: 0,
            cartridge_id: 0,
            country_code: 0,
        };
        assert_eq!(
            validate_crc1_crc2(
                &[0; N64_MIN_CRC_BYTES - 1],
                &header,
                CicVariant::Nus6102Or7101
            ),
            None
        );
    }

    #[test]
    fn implemented_crc_algorithm_round_trips_header_values() {
        let mut rom = vec![0u8; N64_MIN_CRC_BYTES];
        let cic = CicVariant::Nus6103Or7103;
        let (crc1, crc2) = compute_crc1_crc2(&rom, cic).unwrap();
        rom[0x10..0x14].copy_from_slice(&crc1.to_be_bytes());
        rom[0x14..0x18].copy_from_slice(&crc2.to_be_bytes());
        let header = N64HeaderFact {
            clock_rate: 0,
            boot_address: 0,
            release: 0,
            crc1,
            crc2,
            image_name: String::new(),
            manufacturer_id: 0,
            cartridge_id: 0,
            country_code: 0,
        };
        assert_eq!(validate_crc1_crc2(&rom, &header, cic), Some(true));
        let bad_header = N64HeaderFact {
            crc2: crc2 ^ 1,
            ..header
        };
        assert_eq!(validate_crc1_crc2(&rom, &bad_header, cic), Some(false));
    }
}
