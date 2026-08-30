//! Pure, read-only PC Engine CD-ROM² / TurboGrafx-CD IPL (Initial Program
//! Loader) boot-record evidence.
//!
//! # Format verified, not assumed
//!
//! PC Engine CD discs predate ISO 9660 and carry no standard filesystem the
//! System Card boots from. Booting is driven by a fixed 128-byte IPL
//! boot-record that lives in **the second 2048-byte sector (LBA 1) of the
//! first data track**. Two independent technical sources establish its
//! layout:
//!
//! 1. **Hudson "Hu7 CD System - BIOS Manual"**, the official NEC/Hudson PC
//!    Engine CD-ROM² developer documentation (Internet Archive `PCEDev`
//!    collection). It documents the *IPL Information Block* in the sector
//!    after the data-record top: `IPLBLK` (top record number the program is
//!    contained in), `IPLBLN` (number of records to read), `IPLMA`/`IPLSTA`
//!    (main-memory load address), `IPLJMP` (execution start address),
//!    `IPLMPR` (bank to set to MPR before the read), `OPENMODE`, and the
//!    fixed strings `"PC Engine CD-ROM SYSTEM,0"` and `"Copyright HUDSON
//!    SOFT"`.
//! 2. **RetroAchievements `rcheevos`, `src/rhash/hash_disc.c`**
//!    (`rc_hash_pce_track` / `rc_hash_pce_cd`) - the reference disc
//!    identification implementation the retro-achievement ecosystem uses,
//!    validated against Redump PC Engine CD dumps, itself citing
//!    `http://shu.sheldows.com/shu/download/pcedocs/pce_cdrom.html`. It:
//!    - reads 128 bytes from `first_data_track_first_sector + 1`;
//!    - checks `memcmp("PC Engine CD-ROM SYSTEM", &buffer[32], 23)`;
//!    - takes the boot-program start sector as the 24-bit **big-endian**
//!      value `(buffer[0] << 16) | (buffer[1] << 8) | buffer[2]`;
//!    - takes the boot-program length as `buffer[3]` sectors;
//!    - treats `buffer[106..128]` as a 22-byte disc *title* string.
//!
//! The two sources corroborate: the signature string, its offset (32), the
//! sector (data-track LBA 1), and that the first four bytes hold a
//! start-sector pointer plus a sector count.
//!
//! # Deliberately not parsed
//!
//! * The BIOS manual lists `IPLBLK`/`IPLBLN` as one byte each and the
//!   implementation reads a 3-byte big-endian sector plus a 1-byte count;
//!   this module follows the **Redump-validated implementation** layout for
//!   bytes `0..4` and does not try to reconcile the manual's narrower field
//!   widths.
//! * The intermediate IPL fields (load/exec/bank/`OPENMODE`, offsets
//!   `4..31`) are documented but their exact byte boundaries are less
//!   certain and they are not needed for platform/media identity, so this
//!   module does not read or emit them.
//! * There is **no** System Card / version-requirement field in the IPL
//!   header itself (per both sources) - the System Card is a separate
//!   console-side HuCard BIOS. `CD-ROM²` vs `Super CD-ROM²` is a runtime
//!   RAM-size property of the game's own code, not an encoded disc field,
//!   so this module makes no such distinction.
//! * The 22-byte disc title is a debug label, not a serial/catalog/product
//!   code (neither source treats it as one), so this module never emits a
//!   `ProductCode` fact or a title.
//!
//! # Collision safety
//!
//! The 23-byte ASCII signature at a fixed offset in a fixed sector is
//! Hudson/NEC-authored and present on licensed CD-ROM² / Super CD-ROM²
//! discs. It does not collide with any other optical platform's boot
//! signature (PC-FX `PC-FX:Hu_CD-ROM`, Sega CD `SEGADISCSYSTEM`, Saturn
//! `SEGA SEGASATURN`, Dreamcast `SEGA SEGAKATANA`, 3DO `OperaFS`, CD-i
//! `CD-RTOS`, PS1 = ISO 9660 + `SYSTEM.CNF`). Homebrew / GameExpress PC
//! Engine CD discs that lack it produce no evidence here rather than a
//! false negative claim - this module only ever fires on *presence*.

use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};

/// The fixed IPL signature. Checked at exactly [`PCE_CD_SIGNATURE_OFFSET`].
pub const PCE_CD_SIGNATURE: &[u8] = b"PC Engine CD-ROM SYSTEM";

/// The signature's byte offset within the 128-byte IPL boot-record.
pub const PCE_CD_SIGNATURE_OFFSET: usize = 32;

/// The IPL boot-record this module inspects - 128 bytes, read from LBA 1 of
/// the first data track (byte offset [`PCE_CD_IPL_SECTOR_OFFSET`] in the
/// logical data stream).
pub const PCE_CD_IPL_HEADER_BYTES: usize = 128;

/// One CD-ROM sector; the IPL boot-record is in the *second* one.
pub const PCE_CD_SECTOR_BYTES: u64 = 2048;

/// Byte offset of the IPL boot-record within the logical data stream: the
/// start of sector 1 (the second 2048-byte sector) of the first data track.
pub const PCE_CD_IPL_SECTOR_OFFSET: u64 = PCE_CD_SECTOR_BYTES;

/// The bytes the disc-title field occupies - read only so this module can
/// state plainly that it is *not* used as identity.
pub const PCE_CD_TITLE_OFFSET: usize = 106;
pub const PCE_CD_TITLE_BYTES: usize = 22;

/// What a valid PC Engine CD IPL boot-record directly states. Only the
/// fields both sources corroborate; nothing derived, nothing guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PceCdIplFact {
    /// The 24-bit big-endian boot-program start sector, relative to the
    /// data track's first sector (`buffer[0..3]`).
    pub boot_start_sector: u32,
    /// The number of 2048-byte sectors the boot program occupies
    /// (`buffer[3]`).
    pub boot_sector_count: u8,
}

impl PceCdIplFact {
    /// Total bytes the declared boot program spans. `None` on overflow.
    pub fn boot_span_bytes(&self) -> Option<u64> {
        (u64::from(self.boot_start_sector) + u64::from(self.boot_sector_count))
            .checked_mul(PCE_CD_SECTOR_BYTES)
    }
}

/// Parses the 128-byte IPL boot-record read from LBA 1 of the first data
/// track. Returns `Some` only when the exact `"PC Engine CD-ROM SYSTEM"`
/// signature is present at offset 32 and a non-zero boot-program length is
/// declared. Fails closed (`None`) on a short buffer, a wrong/absent
/// signature, or a zero-sector boot program - never a partial struct.
pub fn parse_pce_cd_ipl(ipl_record: &[u8]) -> Option<PceCdIplFact> {
    if ipl_record.len() < PCE_CD_IPL_HEADER_BYTES {
        return None;
    }
    let signature_end = PCE_CD_SIGNATURE_OFFSET + PCE_CD_SIGNATURE.len();
    if ipl_record.get(PCE_CD_SIGNATURE_OFFSET..signature_end) != Some(PCE_CD_SIGNATURE) {
        return None;
    }
    let boot_start_sector = u32::from_be_bytes([0, ipl_record[0], ipl_record[1], ipl_record[2]]);
    let boot_sector_count = ipl_record[3];
    if boot_sector_count == 0 {
        return None;
    }
    Some(PceCdIplFact {
        boot_start_sector,
        boot_sector_count,
    })
}

/// Neutral structural evidence for a parsed IPL boot-record: a single
/// `Strong` `BootStructure` fact whose value is the signature string. No
/// `ProductCode`, no title, no region.
///
/// `Strong` because the signature is platform-naming, at a fixed offset in
/// a fixed sector, Hudson/NEC-authored, and shares no value with any other
/// optical platform's boot signature - the same rating this crate gives
/// `SEGADISCSYSTEM`, `SEGA SEGASATURN` and `PC-FX:Hu_CD-ROM`.
pub fn observe_pce_cd_evidence(fact: &PceCdIplFact) -> Vec<ContentEvidence> {
    vec![ContentEvidence::new(
        ContentEvidenceKind::BootStructure,
        "PC Engine CD-ROM SYSTEM",
        ContentEvidenceConfidence::Strong,
        format!(
            "PC Engine CD-ROM\u{b2} IPL signature at offset 32 of the first data track's \
             second sector; boot program is {} sector(s) from sector {} - layout verified \
             against the RetroAchievements rcheevos PC Engine CD identifier and the Hudson \
             Hu7 CD System BIOS manual",
            fact.boot_sector_count, fact.boot_start_sector
        ),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipl_record(start_sector: u32, count: u8, with_signature: bool) -> [u8; 128] {
        let mut record = [0_u8; 128];
        let start = start_sector.to_be_bytes();
        record[0] = start[1];
        record[1] = start[2];
        record[2] = start[3];
        record[3] = count;
        if with_signature {
            record[PCE_CD_SIGNATURE_OFFSET..PCE_CD_SIGNATURE_OFFSET + PCE_CD_SIGNATURE.len()]
                .copy_from_slice(PCE_CD_SIGNATURE);
        }
        // A disc-title-shaped ASCII string, to prove it is never surfaced.
        record[PCE_CD_TITLE_OFFSET..PCE_CD_TITLE_OFFSET + 8].copy_from_slice(b"GAMENAME");
        record
    }

    #[test]
    fn valid_ipl_record_parses_start_and_count() {
        let fact = parse_pce_cd_ipl(&ipl_record(2, 4, true)).unwrap();
        assert_eq!(fact.boot_start_sector, 2);
        assert_eq!(fact.boot_sector_count, 4);
        assert_eq!(fact.boot_span_bytes(), Some((2 + 4) * 2048));
    }

    #[test]
    fn big_endian_start_sector_uses_bytes_zero_one_two() {
        // 0x010203 = 66051
        let mut record = ipl_record(0, 1, true);
        record[0] = 0x01;
        record[1] = 0x02;
        record[2] = 0x03;
        let fact = parse_pce_cd_ipl(&record).unwrap();
        assert_eq!(fact.boot_start_sector, 0x01_02_03);
    }

    #[test]
    fn missing_signature_fails_closed() {
        assert_eq!(parse_pce_cd_ipl(&ipl_record(2, 4, false)), None);
    }

    #[test]
    fn signature_one_byte_off_position_fails_closed() {
        let mut record = ipl_record(2, 4, true);
        // shift the signature forward by one byte
        record.copy_within(
            PCE_CD_SIGNATURE_OFFSET..PCE_CD_SIGNATURE_OFFSET + PCE_CD_SIGNATURE.len(),
            PCE_CD_SIGNATURE_OFFSET + 1,
        );
        record[PCE_CD_SIGNATURE_OFFSET] = 0;
        assert_eq!(parse_pce_cd_ipl(&record), None);
    }

    #[test]
    fn zero_sector_boot_program_fails_closed() {
        assert_eq!(parse_pce_cd_ipl(&ipl_record(2, 0, true)), None);
    }

    #[test]
    fn short_buffer_fails_closed_not_panic() {
        assert_eq!(parse_pce_cd_ipl(&[0_u8; 64]), None);
        assert_eq!(parse_pce_cd_ipl(&[]), None);
    }

    #[test]
    fn evidence_is_strong_bootstructure_and_never_a_product_code_or_title() {
        let fact = parse_pce_cd_ipl(&ipl_record(9, 3, true)).unwrap();
        let evidence = observe_pce_cd_evidence(&fact);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].kind, ContentEvidenceKind::BootStructure);
        assert_eq!(evidence[0].value, "PC Engine CD-ROM SYSTEM");
        assert_eq!(evidence[0].confidence, ContentEvidenceConfidence::Strong);
        assert!(
            !evidence
                .iter()
                .any(|e| e.kind == ContentEvidenceKind::ProductCode)
        );
        assert!(!evidence[0].detail.contains("GAMENAME"));
    }

    #[test]
    fn parsing_is_deterministic_and_never_mutates_input() {
        let record = ipl_record(5, 7, true);
        let before = record;
        let a = parse_pce_cd_ipl(&record);
        let b = parse_pce_cd_ipl(&record);
        assert_eq!(a, b);
        assert_eq!(record, before);
    }
}
