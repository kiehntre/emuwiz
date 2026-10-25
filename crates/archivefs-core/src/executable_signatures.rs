//! Shared, pure, read-only executable/container header observers: ELF,
//! PS3 SELF, Xbox XBE, Xbox 360 XEX2.
//!
//! Each function here inspects a bounded header and reports a format
//! signature - never platform. ELF in particular is ubiquitous (PS2, PS3's
//! unsigned form, and many non-Sony systems all use it), so
//! [`ElfDetector`] is deliberately the weakest kind of evidence this module
//! produces: a format fact, not even circumstantial platform evidence on
//! its own.
//!
//! # Format facts verified, not assumed
//!
//! - ELF magic (`0x7F 'E' 'L' 'F'`) is the well-known, ISO/IEEE-documented
//!   ELF identification field.
//! - PS3 SELF magic (`0x53 0x43 0x45 0x00`, `"SCE\0"`) is cross-checked
//!   against Wikipedia's community-maintained "List of file signatures"
//!   and multiple independent PS3 homebrew/reverse-engineering sources.
//! - XBE header layout (magic `"XBEH"` at 0, `base_address` at `0x104`,
//!   `certificate_addr` at `0x118`) and the certificate structure
//!   (`size`, `timedate`, `title_id`, then a 40-`u16` UTF-16LE
//!   `title_name`) are verified against `xemu`'s own `xemu-xbe.h`
//!   (`https://github.com/xemu-project/xemu/blob/master/xemu-xbe.h`), a
//!   real, actively-maintained Xbox emulator.
//! - XEX2 magic (`"XEX2"`), the optional-header table layout, and the
//!   execution-info key/offsets (`media_id` at `+0x0`, `title_id` at
//!   `+0xC` within the execution-info block) are the exact values already
//!   verified and reviewed in [`crate::game_identity`]'s own XEX parser -
//!   reused here as constants, not re-derived.
//!
//! XBE addresses are **virtual addresses**: the on-disk certificate offset
//! is `certificate_addr - base_address` (the standard, universally-used
//! XBE convention). This module performs that subtraction with a checked
//! operation and fails closed on underflow rather than wrapping.

use crate::content_detector::{ContentDetectionOutcome, ContentDetector, ContentDiagnostic};
use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};

// ---------------------------------------------------------------------
// ELF
// ---------------------------------------------------------------------

pub const ELF_MAGIC: &[u8; 4] = &[0x7f, b'E', b'L', b'F'];

pub fn looks_like_elf(header: &[u8]) -> bool {
    header.len() >= ELF_MAGIC.len() && &header[..ELF_MAGIC.len()] == ELF_MAGIC.as_slice()
}

/// Generic ELF magic detector. Deliberately the weakest fact this module
/// produces: ELF is a cross-platform, non-Sony-specific format, so this
/// alone is a `Weak` executable-format signature, never platform evidence.
pub struct ElfDetector;

impl ContentDetector for ElfDetector {
    fn id(&self) -> &'static str {
        "elf_magic"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        if !looks_like_elf(data) {
            return ContentDetectionOutcome::NotRecognized;
        }
        ContentDetectionOutcome::Recognized {
            evidence: vec![ContentEvidence::new(
                ContentEvidenceKind::ContentSignature,
                "ELF",
                ContentEvidenceConfidence::Weak,
                "ELF magic present - a generic, cross-platform executable format signature, not platform evidence on its own",
            )],
        }
    }
}

// ---------------------------------------------------------------------
// PS3 SELF
// ---------------------------------------------------------------------

pub const SELF_MAGIC: &[u8; 4] = &[0x53, 0x43, 0x45, 0x00];

pub fn looks_like_self(header: &[u8]) -> bool {
    header.len() >= SELF_MAGIC.len() && &header[..SELF_MAGIC.len()] == SELF_MAGIC.as_slice()
}

/// PS3 SELF ("Signed ELF") container magic. Never decrypts or interprets
/// the signed/encrypted body - only the leading magic is inspected.
pub struct SelfDetector;

impl ContentDetector for SelfDetector {
    fn id(&self) -> &'static str {
        "ps3_self_magic"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        if !looks_like_self(data) {
            return ContentDetectionOutcome::NotRecognized;
        }
        ContentDetectionOutcome::Recognized {
            evidence: vec![ContentEvidence::new(
                ContentEvidenceKind::ContentSignature,
                "SELF",
                ContentEvidenceConfidence::Strong,
                "PS3 SELF (Signed ELF) container magic present",
            )],
        }
    }
}

// ---------------------------------------------------------------------
// Original Xbox XBE
// ---------------------------------------------------------------------

pub const XBE_MAGIC: &[u8; 4] = b"XBEH";
const XBE_BASE_OFFSET: usize = 0x104;
const XBE_CERT_ADDR_OFFSET: usize = 0x118;
/// The fixed prefix `parse_xbe_header`/`xbe_certificate_file_offset` read
/// from - `pub(crate)` so `game_identity`'s bounded XBE reader can size its
/// own read exactly, rather than duplicating this value or over-reading via
/// [`crate::xbox_boot_evidence::XBE_PREFIX_READ_BYTES`]'s more generous
/// disc-traversal bound.
pub(crate) const XBE_HEADER_PREFIX_BYTES: usize = 0x11C;
/// Bounded certificate read - the real structure is smaller than this in
/// every known XBE version; this is headroom, not a claim about the exact
/// certificate size.
pub const XBE_CERTIFICATE_READ_BYTES: usize = 512;
const XBE_CERT_TITLE_ID_OFFSET: usize = 0x8;
const XBE_CERT_TITLE_NAME_OFFSET: usize = 0xC;
const XBE_CERT_TITLE_NAME_UTF16_UNITS: usize = 40;

pub fn looks_like_xbe(header: &[u8]) -> bool {
    header.len() >= XBE_MAGIC.len() && &header[..XBE_MAGIC.len()] == XBE_MAGIC.as_slice()
}

/// What a parsed XBE header/certificate directly states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XbeHeaderFact {
    /// `certificate.title_id`, as an exact 8-hex-digit string - a raw
    /// numeric identifier, not yet a normalized product code.
    pub title_id: Option<String>,
    /// `certificate.title_name`, UTF-16LE, NUL-trimmed.
    pub title_name: Option<String>,
}

/// Parses an XBE header (`header` must include at least
/// [`XBE_HEADER_PREFIX_BYTES`]) plus a separately-supplied, already-read
/// certificate buffer (`certificate`, read by the caller from the
/// computed file offset - see the module documentation on virtual-address
/// translation). Returns `None` only when the magic itself does not
/// match; a present-but-too-short certificate simply yields `None` fields
/// rather than failing the whole parse.
pub fn parse_xbe_header(header: &[u8], certificate: Option<&[u8]>) -> Option<XbeHeaderFact> {
    if !looks_like_xbe(header) || header.len() < XBE_HEADER_PREFIX_BYTES {
        return None;
    }
    let Some(cert) = certificate else {
        return Some(XbeHeaderFact {
            title_id: None,
            title_name: None,
        });
    };
    let title_id = cert
        .get(XBE_CERT_TITLE_ID_OFFSET..XBE_CERT_TITLE_ID_OFFSET + 4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("4-byte slice")))
        .map(|value| format!("{value:08X}"));
    let title_name = cert
        .get(
            XBE_CERT_TITLE_NAME_OFFSET
                ..XBE_CERT_TITLE_NAME_OFFSET + XBE_CERT_TITLE_NAME_UTF16_UNITS * 2,
        )
        .map(decode_utf16le_trimmed);
    Some(XbeHeaderFact {
        title_id,
        title_name,
    })
}

/// Computes the certificate's file offset from the XBE header's own
/// `base_address`/`certificate_addr` virtual addresses. `None` on
/// underflow (a malformed/inconsistent header) rather than a guessed or
/// wrapped offset.
pub fn xbe_certificate_file_offset(header: &[u8]) -> Option<u64> {
    if !looks_like_xbe(header) || header.len() < XBE_HEADER_PREFIX_BYTES {
        return None;
    }
    let base = u32::from_le_bytes(
        header[XBE_BASE_OFFSET..XBE_BASE_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    let cert_addr = u32::from_le_bytes(
        header[XBE_CERT_ADDR_OFFSET..XBE_CERT_ADDR_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    cert_addr.checked_sub(base).map(u64::from)
}

fn decode_utf16le_trimmed(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    text.trim_end_matches('\0').to_string()
}

pub struct XbeDetector;

impl ContentDetector for XbeDetector {
    fn id(&self) -> &'static str {
        "xbox_xbe_magic"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        if !looks_like_xbe(data) {
            return ContentDetectionOutcome::NotRecognized;
        }
        ContentDetectionOutcome::Recognized {
            evidence: vec![ContentEvidence::new(
                ContentEvidenceKind::ContentSignature,
                "XBEH",
                ContentEvidenceConfidence::Strong,
                "Original Xbox XBE header magic present",
            )],
        }
    }
}

// ---------------------------------------------------------------------
// Xbox 360 XEX2
// ---------------------------------------------------------------------

pub const XEX_MAGIC: &[u8; 4] = b"XEX2";
const XEX_BASE_HEADER_BYTES: usize = 0x18;
const XEX_HEADER_COUNT_OFFSET: usize = 0x14;
const XEX_OPT_HEADER_TABLE_OFFSET: usize = 0x18;
const XEX_OPT_HEADER_ENTRY_BYTES: usize = 8;
/// `XEX_HEADER_EXECUTION_INFO` - verified and already reviewed in
/// [`crate::game_identity`].
const XEX_EXECUTION_INFO_KEY: u32 = 0x0004_0006;
const XEX_EXECUTION_INFO_BYTES: usize = 0x18;
const MAX_XEX_OPT_HEADERS: u32 = 512;

pub fn looks_like_xex(header: &[u8]) -> bool {
    header.len() >= XEX_MAGIC.len() && &header[..XEX_MAGIC.len()] == XEX_MAGIC.as_slice()
}

/// What a parsed XEX2 header directly states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xex2HeaderFact {
    pub media_id: String,
    pub title_id: String,
}

/// Parses the unencrypted, uncompressed XEX2 module header from `data`:
/// magic, the optional-header table, and (when present) the execution-info
/// optional header holding `media_id`/`title_id`. Never reads the
/// compressed/encrypted module body. `data` must be a single contiguous
/// buffer containing at least the header, table, and execution-info region
/// (a caller reading a real file would supply a generously bounded prefix).
///
/// Returns `None` when the magic does not match, the header count is zero
/// or exceeds [`MAX_XEX_OPT_HEADERS`], the table/execution-info region is
/// out of bounds, or no execution-info header is present - fails closed,
/// exactly as the already-reviewed [`crate::game_identity`] implementation
/// does.
pub fn parse_xex2_header(data: &[u8]) -> Option<Xex2HeaderFact> {
    if !looks_like_xex(data) || data.len() < XEX_BASE_HEADER_BYTES {
        return None;
    }
    let header_count = u32::from_be_bytes(
        data[XEX_HEADER_COUNT_OFFSET..XEX_HEADER_COUNT_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    if header_count == 0 || header_count > MAX_XEX_OPT_HEADERS {
        return None;
    }
    let table_end =
        XEX_OPT_HEADER_TABLE_OFFSET + header_count as usize * XEX_OPT_HEADER_ENTRY_BYTES;
    let table = data.get(XEX_OPT_HEADER_TABLE_OFFSET..table_end)?;

    let execution_info_offset =
        table
            .chunks_exact(XEX_OPT_HEADER_ENTRY_BYTES)
            .find_map(|entry| {
                let key = u32::from_be_bytes(entry[0..4].try_into().unwrap());
                (key == XEX_EXECUTION_INFO_KEY)
                    .then(|| u32::from_be_bytes(entry[4..8].try_into().unwrap()))
            })?;

    let start = execution_info_offset as usize;
    let execution_info = data.get(start..start + XEX_EXECUTION_INFO_BYTES)?;
    let media_id = u32::from_be_bytes(execution_info[0x0..0x4].try_into().unwrap());
    let title_id = u32::from_be_bytes(execution_info[0xC..0x10].try_into().unwrap());
    Some(Xex2HeaderFact {
        media_id: format!("{media_id:08X}"),
        title_id: format!("{title_id:08X}"),
    })
}

pub struct XexDetector;

impl ContentDetector for XexDetector {
    fn id(&self) -> &'static str {
        "xbox360_xex_magic"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        if !looks_like_xex(data) {
            return ContentDetectionOutcome::NotRecognized;
        }
        ContentDetectionOutcome::Recognized {
            evidence: vec![ContentEvidence::new(
                ContentEvidenceKind::ContentSignature,
                "XEX2",
                ContentEvidenceConfidence::Strong,
                "Xbox 360 XEX2 module header magic present",
            )],
        }
    }
}

// ---------------------------------------------------------------------
// DOS MZ executable
// ---------------------------------------------------------------------
//
// # MZ format verified, not assumed
//
// The header layout below is cross-checked against two independent
// references:
//
// - the OSDev wiki "MZ" article (https://wiki.osdev.org/MZ), and
// - "The DOS EXE File Format" (https://www.tavi.co.uk/phobos/exeformat.html),
//   the widely-cited canonical write-up of the `.EXE` header,
//
// corroborated by Wikipedia's "DOS MZ executable" article for the
// "header paragraphs x 16 = load-module offset" rule. All three agree on
// every field offset used here and on the image-size computation,
// including the `bytes-in-last-page == 0` "final page is full" case.
//
// # What a valid MZ header proves, and what it does not
//
// It proves the bytes are a structurally coherent DOS MZ executable
// container - nothing more. `MZ` is the shared prefix of DOS `.EXE`, and
// of every NE / LE / LX / PE (Windows-era) executable, which keep an MZ
// header as a stub. So this is deliberately the weakest kind of evidence
// this module produces: a `Weak`, generic
// [`ContentEvidenceKind::ContentSignature`] fact, never platform
// evidence, and never a fusion-rule leg that could resolve DOS on its
// own. NE/PE/LE/LX parsing is explicitly out of scope; the only nod to it
// is observing whether an `e_lfanew` pointer is present at `0x3C`.

/// `MZ`, little-endian `0x5A4D` - the DOS executable signature.
pub const MZ_MAGIC: &[u8; 2] = b"MZ";

/// The fixed part of the MZ header, offsets `0x00..=0x1B`. A shorter file
/// cannot be a valid MZ executable.
pub const MZ_HEADER_BYTES: usize = 0x1C;

/// Offset of the `e_lfanew` dword an NE/PE/LE/LX file carries to point at
/// its real header. Only *observed* here, never followed.
const MZ_LFANEW_OFFSET: usize = 0x3C;

/// One 512-byte page, the unit `pages_in_file` counts in.
const MZ_PAGE_BYTES: u32 = 512;

/// One relocation-table entry: a 16-bit offset and a 16-bit segment.
const MZ_RELOCATION_ENTRY_BYTES: u32 = 4;

/// A defensive upper bound on the MZ header size this module will accept,
/// so a hostile `header_paragraphs` cannot describe a multi-megabyte
/// "header". Real MZ headers are a few hundred bytes; 64 KiB is generous
/// headroom, not a claim about any real file.
pub const MZ_MAX_HEADER_BYTES: u32 = 64 * 1024;

pub fn looks_like_mz(header: &[u8]) -> bool {
    header.len() >= MZ_MAGIC.len() && &header[..MZ_MAGIC.len()] == MZ_MAGIC.as_slice()
}

/// The structural fields a parsed MZ header directly states, plus a few
/// values derived from them by checked arithmetic. Never a platform, never
/// a title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MzHeaderFact {
    /// `e_cblp` - bytes used in the final 512-byte page (`0` => full page).
    pub bytes_in_last_page: u16,
    /// `e_cp` - number of 512-byte pages in the file.
    pub pages_in_file: u16,
    /// `e_crlc` - number of relocation-table entries.
    pub relocation_count: u16,
    /// `e_cparhdr` - header size in 16-byte paragraphs.
    pub header_paragraphs: u16,
    /// `e_minalloc` - minimum extra paragraphs the program needs.
    pub min_extra_paragraphs: u16,
    /// `e_maxalloc` - maximum extra paragraphs the program wants.
    pub max_extra_paragraphs: u16,
    /// `e_ss` - initial stack-segment value (relocatable).
    pub initial_ss: u16,
    /// `e_sp` - initial stack-pointer value.
    pub initial_sp: u16,
    /// `e_csum` - header checksum field, as stored (not verified).
    pub checksum: u16,
    /// `e_ip` - initial instruction-pointer value.
    pub initial_ip: u16,
    /// `e_cs` - initial code-segment value (relocatable).
    pub initial_cs: u16,
    /// `e_lfarlc` - file offset of the relocation table.
    pub relocation_table_offset: u16,
    /// `e_ovno` - overlay number (`0` for the main program).
    pub overlay_number: u16,
    /// Derived: `(pages_in_file - 1) * 512 + (bytes_in_last_page or 512)`.
    pub load_module_bytes: u32,
    /// Derived: `header_paragraphs * 16` - where the load module begins.
    pub header_bytes: u32,
    /// Whether a non-zero `e_lfanew` pointer sits at `0x3C` - a strong hint
    /// the file is really NE/PE/LE/LX with an MZ stub. Observed only; this
    /// module never follows it.
    pub has_extended_header_pointer: bool,
}

fn le_u16_at(data: &[u8], offset: usize) -> Option<u16> {
    let slice = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn le_u32_at(data: &[u8], offset: usize) -> Option<u32> {
    let slice = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Parses the fixed MZ header from `data` (a header prefix or a whole
/// file), validating it enough to stand behind an [`MzHeaderFact`], or
/// returns `None`.
///
/// `file_len`, when known (a loose file on disk), enables the
/// "declared load module is not larger than the file" and "relocation
/// table lies within the file" checks; pass `None` when `data` is only a
/// bounded prefix (an archive member probe) and those cannot be judged.
///
/// Fails closed - returns `None` - on: a bad or absent `MZ` magic, fewer
/// than [`MZ_HEADER_BYTES`] bytes, a zero page count, a last-page byte
/// count above 512, a header smaller than the fixed header or larger than
/// [`MZ_MAX_HEADER_BYTES`], any checked-arithmetic overflow, a relocation
/// table that runs past the header (or, when `file_len` is known, past the
/// file), or a declared load-module size larger than the file.
pub fn parse_mz_header(data: &[u8], file_len: Option<u64>) -> Option<MzHeaderFact> {
    if !looks_like_mz(data) || data.len() < MZ_HEADER_BYTES {
        return None;
    }

    let bytes_in_last_page = le_u16_at(data, 0x02)?;
    let pages_in_file = le_u16_at(data, 0x04)?;
    let relocation_count = le_u16_at(data, 0x06)?;
    let header_paragraphs = le_u16_at(data, 0x08)?;
    let min_extra_paragraphs = le_u16_at(data, 0x0A)?;
    let max_extra_paragraphs = le_u16_at(data, 0x0C)?;
    let initial_ss = le_u16_at(data, 0x0E)?;
    let initial_sp = le_u16_at(data, 0x10)?;
    let checksum = le_u16_at(data, 0x12)?;
    let initial_ip = le_u16_at(data, 0x14)?;
    let initial_cs = le_u16_at(data, 0x16)?;
    let relocation_table_offset = le_u16_at(data, 0x18)?;
    let overlay_number = le_u16_at(data, 0x1A)?;

    if pages_in_file == 0 {
        return None;
    }
    if u32::from(bytes_in_last_page) > MZ_PAGE_BYTES {
        return None;
    }

    // Header size: at least the fixed header, at most a sane cap.
    let header_bytes = u32::from(header_paragraphs).checked_mul(16)?;
    if header_bytes < MZ_HEADER_BYTES as u32 || header_bytes > MZ_MAX_HEADER_BYTES {
        return None;
    }

    // Load-module size: (pages - 1) full pages plus the final page's bytes,
    // where a zero last-page count means the final page is full.
    let last_page = if bytes_in_last_page == 0 {
        MZ_PAGE_BYTES
    } else {
        u32::from(bytes_in_last_page)
    };
    let load_module_bytes = u32::from(pages_in_file)
        .checked_sub(1)?
        .checked_mul(MZ_PAGE_BYTES)?
        .checked_add(last_page)?;
    // The image must at least contain its own header.
    if load_module_bytes < header_bytes {
        return None;
    }
    if let Some(len) = file_len
        && u64::from(load_module_bytes) > len
    {
        return None;
    }

    // Relocation table (when present) must lie inside the header - and,
    // when the real file length is known, inside the file.
    if relocation_count > 0 {
        if u32::from(relocation_table_offset) < MZ_HEADER_BYTES as u32 {
            return None;
        }
        let reloc_end = u32::from(relocation_table_offset)
            .checked_add(u32::from(relocation_count).checked_mul(MZ_RELOCATION_ENTRY_BYTES)?)?;
        if reloc_end > header_bytes {
            return None;
        }
        if let Some(len) = file_len
            && u64::from(reloc_end) > len
        {
            return None;
        }
    }

    let has_extended_header_pointer = le_u32_at(data, MZ_LFANEW_OFFSET)
        .map(|lfanew| lfanew != 0)
        .unwrap_or(false);

    Some(MzHeaderFact {
        bytes_in_last_page,
        pages_in_file,
        relocation_count,
        header_paragraphs,
        min_extra_paragraphs,
        max_extra_paragraphs,
        initial_ss,
        initial_sp,
        checksum,
        initial_ip,
        initial_cs,
        relocation_table_offset,
        overlay_number,
        load_module_bytes,
        header_bytes,
        has_extended_header_pointer,
    })
}

/// Generic DOS MZ executable-container detector. Like [`ElfDetector`],
/// deliberately the weakest fact this module produces: `MZ` is shared by
/// DOS `.EXE` and every Windows-era NE/PE/LE/LX executable, so a valid MZ
/// header is a `Weak`, generic format signature, never platform evidence
/// and never a single-leg DOS resolver.
///
/// Sees only a bounded prefix (`data`), so it validates internal
/// consistency but not the real file length; a loose file on disk is
/// checked more strictly by the ingestion layer, which knows the length.
pub struct MzDetector;

impl ContentDetector for MzDetector {
    fn id(&self) -> &'static str {
        "dos_mz_magic"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        if !looks_like_mz(data) {
            return ContentDetectionOutcome::NotRecognized;
        }
        match parse_mz_header(data, None) {
            Some(fact) => {
                let detail = if fact.has_extended_header_pointer {
                    "DOS MZ executable header present, with an NE/PE/LE extended-header pointer - \
                     only the MZ stub is observed; a generic executable-format signature, not \
                     platform evidence"
                } else {
                    "DOS MZ executable header present - a generic executable-format signature \
                     shared with Windows-era executables, not platform evidence on its own"
                };
                ContentDetectionOutcome::Recognized {
                    evidence: vec![ContentEvidence::new(
                        ContentEvidenceKind::ContentSignature,
                        "MZ",
                        ContentEvidenceConfidence::Weak,
                        detail,
                    )],
                }
            }
            None => ContentDetectionOutcome::Malformed {
                evidence: Vec::new(),
                diagnostic: ContentDiagnostic {
                    detector_id: "dos_mz_magic",
                    category: "bad_header",
                    message: "MZ magic present but the fixed header failed structural validation"
                        .to_string(),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // ELF
    // ------------------------------------------------------------------

    #[test]
    fn elf_magic_is_detected() {
        let data = [0x7f, b'E', b'L', b'F', 1, 2, 3];
        assert!(looks_like_elf(&data));
        let outcome = ElfDetector.detect(&data);
        assert!(outcome.is_recognized());
        assert_eq!(
            outcome.evidence()[0].confidence,
            ContentEvidenceConfidence::Weak
        );
    }

    #[test]
    fn non_elf_is_not_recognized() {
        assert!(!looks_like_elf(b"not an elf"));
        assert_eq!(
            ElfDetector.detect(b"not an elf"),
            ContentDetectionOutcome::NotRecognized
        );
    }

    // ------------------------------------------------------------------
    // SELF
    // ------------------------------------------------------------------

    #[test]
    fn self_magic_is_detected() {
        let data = [0x53, 0x43, 0x45, 0x00, 1, 2];
        assert!(looks_like_self(&data));
        assert!(SelfDetector.detect(&data).is_recognized());
    }

    #[test]
    fn non_self_is_not_recognized() {
        assert!(!looks_like_self(b"ELF!"));
    }

    // ------------------------------------------------------------------
    // XBE
    // ------------------------------------------------------------------

    fn synthetic_xbe_header(base: u32, cert_addr: u32) -> Vec<u8> {
        let mut header = vec![0u8; XBE_HEADER_PREFIX_BYTES];
        header[0..4].copy_from_slice(XBE_MAGIC);
        header[XBE_BASE_OFFSET..XBE_BASE_OFFSET + 4].copy_from_slice(&base.to_le_bytes());
        header[XBE_CERT_ADDR_OFFSET..XBE_CERT_ADDR_OFFSET + 4]
            .copy_from_slice(&cert_addr.to_le_bytes());
        header
    }

    fn synthetic_xbe_certificate(title_id: u32, title_name: &str) -> Vec<u8> {
        let mut cert = vec![0u8; XBE_CERTIFICATE_READ_BYTES];
        cert[XBE_CERT_TITLE_ID_OFFSET..XBE_CERT_TITLE_ID_OFFSET + 4]
            .copy_from_slice(&title_id.to_le_bytes());
        let utf16: Vec<u8> = title_name
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        let end = (XBE_CERT_TITLE_NAME_OFFSET + utf16.len()).min(cert.len());
        cert[XBE_CERT_TITLE_NAME_OFFSET..end]
            .copy_from_slice(&utf16[..end - XBE_CERT_TITLE_NAME_OFFSET]);
        cert
    }

    #[test]
    fn xbe_magic_is_detected() {
        let header = synthetic_xbe_header(0x10000, 0x10120);
        assert!(looks_like_xbe(&header));
        assert!(XbeDetector.detect(&header).is_recognized());
    }

    #[test]
    fn xbe_certificate_offset_is_computed_from_virtual_addresses() {
        let header = synthetic_xbe_header(0x10000, 0x10120);
        assert_eq!(xbe_certificate_file_offset(&header), Some(0x120));
    }

    #[test]
    fn xbe_certificate_offset_underflow_fails_closed() {
        let header = synthetic_xbe_header(0x20000, 0x10120);
        assert_eq!(xbe_certificate_file_offset(&header), None);
    }

    #[test]
    fn xbe_title_id_and_name_are_extracted() {
        let header = synthetic_xbe_header(0x10000, 0x10120);
        let cert = synthetic_xbe_certificate(0x4D5A0058, "Test Game");
        let fact = parse_xbe_header(&header, Some(&cert)).unwrap();
        assert_eq!(fact.title_id.as_deref(), Some("4D5A0058"));
        assert_eq!(fact.title_name.as_deref(), Some("Test Game"));
    }

    #[test]
    fn xbe_without_certificate_yields_no_fields() {
        let header = synthetic_xbe_header(0x10000, 0x10120);
        let fact = parse_xbe_header(&header, None).unwrap();
        assert_eq!(fact.title_id, None);
        assert_eq!(fact.title_name, None);
    }

    #[test]
    fn non_xbe_header_is_not_parsed() {
        assert_eq!(parse_xbe_header(b"not xbe", None), None);
    }

    // ------------------------------------------------------------------
    // XEX2
    // ------------------------------------------------------------------

    fn synthetic_xex2(media_id: u32, title_id: u32) -> Vec<u8> {
        let mut data =
            vec![
                0u8;
                XEX_OPT_HEADER_TABLE_OFFSET + XEX_OPT_HEADER_ENTRY_BYTES + XEX_EXECUTION_INFO_BYTES
            ];
        data[0..4].copy_from_slice(XEX_MAGIC);
        data[XEX_HEADER_COUNT_OFFSET..XEX_HEADER_COUNT_OFFSET + 4]
            .copy_from_slice(&1u32.to_be_bytes());
        let execution_info_offset =
            (XEX_OPT_HEADER_TABLE_OFFSET + XEX_OPT_HEADER_ENTRY_BYTES) as u32;
        data[XEX_OPT_HEADER_TABLE_OFFSET..XEX_OPT_HEADER_TABLE_OFFSET + 4]
            .copy_from_slice(&XEX_EXECUTION_INFO_KEY.to_be_bytes());
        data[XEX_OPT_HEADER_TABLE_OFFSET + 4..XEX_OPT_HEADER_TABLE_OFFSET + 8]
            .copy_from_slice(&execution_info_offset.to_be_bytes());
        let info_start = execution_info_offset as usize;
        data[info_start..info_start + 4].copy_from_slice(&media_id.to_be_bytes());
        data[info_start + 0xC..info_start + 0x10].copy_from_slice(&title_id.to_be_bytes());
        data
    }

    #[test]
    fn xex2_magic_is_detected() {
        let data = synthetic_xex2(0x4141_4141, 0x4242_4242);
        assert!(looks_like_xex(&data));
        assert!(XexDetector.detect(&data).is_recognized());
    }

    #[test]
    fn xex2_media_and_title_id_are_extracted() {
        let data = synthetic_xex2(0x4141_4141, 0x4242_4242);
        let fact = parse_xex2_header(&data).unwrap();
        assert_eq!(fact.media_id, "41414141");
        assert_eq!(fact.title_id, "42424242");
    }

    #[test]
    fn xex2_zero_header_count_fails_closed() {
        let mut data = synthetic_xex2(1, 2);
        data[XEX_HEADER_COUNT_OFFSET..XEX_HEADER_COUNT_OFFSET + 4]
            .copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(parse_xex2_header(&data), None);
    }

    #[test]
    fn xex2_excessive_header_count_fails_closed() {
        let mut data = synthetic_xex2(1, 2);
        data[XEX_HEADER_COUNT_OFFSET..XEX_HEADER_COUNT_OFFSET + 4]
            .copy_from_slice(&(MAX_XEX_OPT_HEADERS + 1).to_be_bytes());
        assert_eq!(parse_xex2_header(&data), None);
    }

    #[test]
    fn xex2_no_execution_info_header_fails_closed() {
        let mut data = synthetic_xex2(1, 2);
        // Corrupt the key so it no longer matches XEX_EXECUTION_INFO_KEY.
        data[XEX_OPT_HEADER_TABLE_OFFSET..XEX_OPT_HEADER_TABLE_OFFSET + 4]
            .copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(parse_xex2_header(&data), None);
    }

    #[test]
    fn xex2_evidence_never_assigns_a_platform() {
        let data = synthetic_xex2(1, 2);
        let outcome = XexDetector.detect(&data);
        for item in outcome.evidence() {
            assert_eq!(item.kind, ContentEvidenceKind::ContentSignature);
        }
    }

    // ------------------------------------------------------------------
    // DOS MZ
    // ------------------------------------------------------------------

    /// A minimal, structurally valid MZ header: 2-paragraph (32-byte)
    /// header, one 512-byte page, no relocations.
    fn minimal_mz_header() -> Vec<u8> {
        let mut data = vec![0u8; 512];
        data[0x00..0x02].copy_from_slice(MZ_MAGIC);
        data[0x02..0x04].copy_from_slice(&0u16.to_le_bytes()); // last page full
        data[0x04..0x06].copy_from_slice(&1u16.to_le_bytes()); // 1 page
        data[0x06..0x08].copy_from_slice(&0u16.to_le_bytes()); // no relocations
        data[0x08..0x0A].copy_from_slice(&2u16.to_le_bytes()); // 2 paragraphs = 32 bytes
        data[0x0A..0x0C].copy_from_slice(&0u16.to_le_bytes()); // minalloc
        data[0x0C..0x0E].copy_from_slice(&0xFFFFu16.to_le_bytes()); // maxalloc
        data[0x0E..0x10].copy_from_slice(&0u16.to_le_bytes()); // ss
        data[0x10..0x12].copy_from_slice(&0xB800u16.to_le_bytes()); // sp
        data[0x12..0x14].copy_from_slice(&0u16.to_le_bytes()); // checksum
        data[0x14..0x16].copy_from_slice(&0u16.to_le_bytes()); // ip
        data[0x16..0x18].copy_from_slice(&0u16.to_le_bytes()); // cs
        data[0x18..0x1A].copy_from_slice(&0x1Cu16.to_le_bytes()); // reloc table offset
        data[0x1A..0x1C].copy_from_slice(&0u16.to_le_bytes()); // overlay 0
        data
    }

    /// An MZ header declaring `reloc_count` relocations at `reloc_off`,
    /// with a header large enough (or not) to hold them.
    fn mz_with_relocations(reloc_off: u16, reloc_count: u16, header_paragraphs: u16) -> Vec<u8> {
        let mut data = minimal_mz_header();
        data[0x06..0x08].copy_from_slice(&reloc_count.to_le_bytes());
        data[0x08..0x0A].copy_from_slice(&header_paragraphs.to_le_bytes());
        data[0x18..0x1A].copy_from_slice(&reloc_off.to_le_bytes());
        data
    }

    #[test]
    fn minimal_valid_mz_header_parses_deterministically() {
        let data = minimal_mz_header();
        assert!(looks_like_mz(&data));
        let first = parse_mz_header(&data, Some(data.len() as u64)).unwrap();
        let second = parse_mz_header(&data, Some(data.len() as u64)).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.pages_in_file, 1);
        assert_eq!(first.header_paragraphs, 2);
        assert_eq!(first.header_bytes, 32);
        assert_eq!(first.load_module_bytes, 512); // last page full
        assert_eq!(first.initial_sp, 0xB800);
        assert!(!first.has_extended_header_pointer);
    }

    #[test]
    fn last_page_byte_count_is_honoured_when_non_zero() {
        let mut data = minimal_mz_header();
        data[0x04..0x06].copy_from_slice(&2u16.to_le_bytes()); // 2 pages
        data[0x02..0x04].copy_from_slice(&100u16.to_le_bytes()); // 100 bytes in last
        let fact = parse_mz_header(&data, Some(4096)).unwrap();
        assert_eq!(fact.load_module_bytes, 512 + 100);
    }

    #[test]
    fn valid_relocation_table_within_the_header_is_accepted() {
        // 4 relocations * 4 bytes = 16, starting at 0x1C, ends at 0x2C;
        // header of 3 paragraphs = 48 bytes holds it.
        let data = mz_with_relocations(0x1C, 4, 3);
        let fact = parse_mz_header(&data, Some(data.len() as u64)).unwrap();
        assert_eq!(fact.relocation_count, 4);
        assert_eq!(fact.relocation_table_offset, 0x1C);
    }

    #[test]
    fn mz_detector_emits_only_weak_generic_signature_evidence() {
        let outcome = MzDetector.detect(&minimal_mz_header());
        assert!(outcome.is_recognized());
        let evidence = outcome.evidence();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].kind, ContentEvidenceKind::ContentSignature);
        assert_eq!(evidence[0].value, "MZ");
        assert_eq!(evidence[0].confidence, ContentEvidenceConfidence::Weak);
    }

    #[test]
    fn mz_evidence_scope_is_generic() {
        use crate::content_evidence_scope::{EvidenceScope, scope_of};
        assert_eq!(
            scope_of(ContentEvidenceKind::ContentSignature, "MZ"),
            EvidenceScope::Generic
        );
    }

    #[test]
    fn extended_header_pointer_is_observed_not_followed() {
        let mut data = minimal_mz_header();
        // A plausible e_lfanew at 0x3C, as an NE/PE MZ stub carries.
        data[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        let fact = parse_mz_header(&data, Some(data.len() as u64)).unwrap();
        assert!(fact.has_extended_header_pointer);
        // Still only weak/generic - the stub does not become anything more.
        let evidence = MzDetector.detect(&data);
        assert_eq!(
            evidence.evidence()[0].confidence,
            ContentEvidenceConfidence::Weak
        );
    }

    #[test]
    fn random_bytes_are_not_recognized_as_mz() {
        assert!(!looks_like_mz(b"not an exe at all"));
        assert_eq!(
            MzDetector.detect(b"not an exe at all"),
            ContentDetectionOutcome::NotRecognized
        );
    }

    #[test]
    fn bad_magic_is_not_mz() {
        let mut data = minimal_mz_header();
        data[0] = b'Z';
        data[1] = b'M'; // "ZM" is not accepted by this strict parser
        assert!(!looks_like_mz(&data));
        assert_eq!(parse_mz_header(&data, None), None);
    }

    #[test]
    fn truncated_mz_header_is_refused() {
        let data = minimal_mz_header();
        assert_eq!(parse_mz_header(&data[..MZ_HEADER_BYTES - 1], None), None);
    }

    #[test]
    fn mz_with_magic_but_truncated_header_reports_malformed() {
        let short = &minimal_mz_header()[..8];
        assert!(MzDetector.detect(short).is_malformed());
    }

    #[test]
    fn zero_page_count_is_refused() {
        let mut data = minimal_mz_header();
        data[0x04..0x06].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(parse_mz_header(&data, None), None);
    }

    #[test]
    fn last_page_byte_count_above_512_is_refused() {
        let mut data = minimal_mz_header();
        data[0x02..0x04].copy_from_slice(&513u16.to_le_bytes());
        assert_eq!(parse_mz_header(&data, None), None);
    }

    #[test]
    fn header_smaller_than_the_fixed_header_is_refused() {
        let mut data = minimal_mz_header();
        data[0x08..0x0A].copy_from_slice(&1u16.to_le_bytes()); // 16 bytes < 0x1C
        assert_eq!(parse_mz_header(&data, None), None);
    }

    #[test]
    fn absurd_header_paragraph_count_is_refused() {
        let mut data = minimal_mz_header();
        data[0x08..0x0A].copy_from_slice(&0xFFFFu16.to_le_bytes()); // ~1 MiB "header"
        assert_eq!(parse_mz_header(&data, None), None);
    }

    #[test]
    fn relocation_table_outside_the_header_is_refused() {
        // Table at 0x30 for 4 entries ends at 0x40; a 2-paragraph (32-byte)
        // header cannot contain it.
        let data = mz_with_relocations(0x30, 4, 2);
        assert_eq!(parse_mz_header(&data, Some(data.len() as u64)), None);
    }

    #[test]
    fn declared_load_module_larger_than_the_file_is_refused() {
        let mut data = minimal_mz_header();
        data[0x04..0x06].copy_from_slice(&64u16.to_le_bytes()); // 64 pages = 32 KiB
        // file is only 512 bytes
        assert_eq!(parse_mz_header(&data, Some(512)), None);
        // ...but with no known file length, internal consistency alone passes.
        assert!(parse_mz_header(&data, None).is_some());
    }

    #[test]
    fn relocation_table_past_eof_is_refused_when_file_length_is_known() {
        let data = mz_with_relocations(0x1C, 4, 3); // table ends at 0x2C, header 48
        // file shorter than the table end
        assert_eq!(parse_mz_header(&data, Some(0x20)), None);
    }

    #[test]
    fn mz_fact_carries_no_platform_or_title_field() {
        // Compile-time-ish guard: the struct exposes only structural
        // numeric fields. This test documents intent; a new platform/title
        // field would need a deliberate edit here.
        let fact = parse_mz_header(&minimal_mz_header(), None).unwrap();
        let _ = (
            fact.bytes_in_last_page,
            fact.pages_in_file,
            fact.relocation_count,
            fact.header_paragraphs,
            fact.initial_cs,
            fact.initial_ip,
            fact.load_module_bytes,
            fact.header_bytes,
            fact.has_extended_header_pointer,
        );
    }
}
