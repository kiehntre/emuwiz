//! Pure, read-only structural parsing of ZX Spectrum machine snapshots -
//! `.z80` (generations 1, 2 and 3), `.sna` (48K and 128K) and `.szx`
//! (ZX-State) - producing content/structural evidence only, never a unique
//! game identity.
//!
//! # The distinction this module keeps
//!
//! A parsed snapshot proves three separate things, and this module keeps
//! them separate rather than collapsing them into one "is a Spectrum game":
//!
//! * **A** - the container structure is internally valid (magic / version /
//!   size / block table all agree).
//! * **B** - the platform family is structurally supported (a valid `.z80`,
//!   `.sna` or `.szx` is a Sinclair-family snapshot by construction).
//! * **C** - a *machine subtype* (48K / 128K / +2 / +2A / +3 / Pentagon /
//!   Scorpion) is actually encoded in the bytes, as opposed to merely
//!   implied by the format version or absent entirely.
//!
//! Only **C**, when a documented hardware-mode value is genuinely present,
//! is reported at [`ContentEvidenceConfidence::Strong`]. A machine that is
//! only *implied* by the format (a Z80 v1 snapshot is a 48K machine by
//! definition, but no byte says so) is [`ContentEvidenceConfidence::Corroborated`].
//! An unknown hardware-mode byte is preserved as raw and never mapped to a
//! fabricated subtype.
//!
//! # Formats verified, not assumed
//!
//! * `.z80` - Z80 file format reference, World of Spectrum FAQ
//!   (`https://worldofspectrum.org/faq/reference/z80format.htm`), the
//!   reference every mainstream Spectrum emulator's `.z80` loader follows.
//!   The v1 header is 30 bytes; a zero word at offset 6 (`PC`) signals a v2/
//!   v3 file, whose extra-header length at offset 30 is 23 (v2), or 54 / 55
//!   (v3). The hardware-mode byte at offset 34 is interpreted **relative to
//!   that length** - value `3` means "128K" in a v2 file but "48K + MGT" in
//!   a v3 file - which is exactly why a hardware byte is never read without
//!   first pinning the generation.
//! * `.sna` - snapshot formats reference, World of Spectrum FAQ
//!   (`https://worldofspectrum.org/faq/reference/formats.htm`). A 48K `.sna`
//!   is a 27-byte register header followed by exactly 49152 bytes of RAM
//!   (49179 total). A 128K `.sna` adds a 4-byte extension (PC, port 0x7FFD,
//!   TR-DOS flag) and the five remaining 16K banks (131103 total).
//! * `.szx` - the ZX-State (`ZXST`) container, Spectaculator's published
//!   specification. A 8-byte header (`ZXST`, major, minor, `machineId`,
//!   flags) followed by `{ 4-byte id, 4-byte LE size, data }` blocks.
//!
//! # What this module does not do
//!
//! It never decompresses a memory page, never reconstructs RAM, never
//! executes anything, and never reads or infers a title. Reads are a
//! bounded prefix; a whole-file buffer is only ever taken by
//! [`inspect_spectrum_snapshot_file`], which caps it at
//! [`MAX_SNAPSHOT_BYTES`] from filesystem metadata before reading.

use std::path::Path;

use crate::content_detector::{ContentDetectionOutcome, ContentDetector};
use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};

/// The most a `.z80` / `.sna` / `.szx` this module will read whole. A 128K
/// `.sna` is 131103 bytes; a `.z80` or `.szx` with every page uncompressed
/// and a full AY/peripheral chunk set is comfortably under this. Anything
/// larger is not one of these snapshot formats.
pub const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024;

/// A canonical Sinclair-family machine, only ever set from a value the
/// snapshot format genuinely encodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectrumMachine {
    Spectrum16K,
    Spectrum48K,
    Spectrum128K,
    SpectrumPlus2,
    SpectrumPlus2A,
    SpectrumPlus3,
    Pentagon,
    Scorpion,
    /// A hardware value the format documents but which this pass does not
    /// map to one of the canonical machines above (SamRam, Timex, Didaktik,
    /// SE, ...). The descriptive name is carried so it can be shown without
    /// being turned into a canonical subtype.
    OtherDocumented(&'static str),
}

impl SpectrumMachine {
    pub fn label(self) -> &'static str {
        match self {
            Self::Spectrum16K => "ZX Spectrum 16K",
            Self::Spectrum48K => "ZX Spectrum 48K",
            Self::Spectrum128K => "ZX Spectrum 128K",
            Self::SpectrumPlus2 => "ZX Spectrum +2",
            Self::SpectrumPlus2A => "ZX Spectrum +2A",
            Self::SpectrumPlus3 => "ZX Spectrum +3",
            Self::Pentagon => "Pentagon",
            Self::Scorpion => "Scorpion",
            Self::OtherDocumented(name) => name,
        }
    }

    /// The canonical [`crate::platform`] family every value here belongs to.
    /// The registry has one Sinclair row; Pentagon and Scorpion clones are
    /// software-compatible members of it, not separate canonical platforms.
    pub const PLATFORM: &'static str = "ZX Spectrum";
}

/// How firmly the machine subtype is known - the **C** axis from the module
/// documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineEvidence {
    /// A documented hardware-mode value was present and decoded.
    Encoded(SpectrumMachine),
    /// The format version fixes the machine by definition, but no byte
    /// states it (a Z80 v1 snapshot is always a 48K machine).
    ImpliedByFormat(SpectrumMachine),
    /// A hardware-mode byte was present but its value is outside every
    /// documented case for this format/generation. Preserved as raw; no
    /// subtype is invented.
    EncodedButUndocumented { raw: u16 },
    /// This snapshot form carries no machine identifier at all and none is
    /// implied (a 48K `.sna` is distinguished from a 128K one only by its
    /// total size, which this still reports as `ImpliedByFormat`; this
    /// variant is for forms with genuinely nothing).
    Absent,
}

impl MachineEvidence {
    fn confidence(self) -> ContentEvidenceConfidence {
        match self {
            Self::Encoded(_) => ContentEvidenceConfidence::Strong,
            Self::ImpliedByFormat(_) => ContentEvidenceConfidence::Corroborated,
            Self::EncodedButUndocumented { .. } | Self::Absent => ContentEvidenceConfidence::Weak,
        }
    }

    fn machine(self) -> Option<SpectrumMachine> {
        match self {
            Self::Encoded(machine) | Self::ImpliedByFormat(machine) => Some(machine),
            Self::EncodedButUndocumented { .. } | Self::Absent => None,
        }
    }
}

/// Which snapshot container was recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFormat {
    Z80V1,
    Z80V2,
    Z80V3,
    Sna48K,
    Sna128K,
    Szx,
}

impl SnapshotFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Z80V1 => "Z80 snapshot (v1)",
            Self::Z80V2 => "Z80 snapshot (v2)",
            Self::Z80V3 => "Z80 snapshot (v3)",
            Self::Sna48K => "SNA snapshot (48K)",
            Self::Sna128K => "SNA snapshot (128K)",
            Self::Szx => "SZX (ZX-State) snapshot",
        }
    }
}

/// The structural facts a valid snapshot yields. Register values that carry
/// no identity meaning (the full register file) are deliberately not
/// surfaced; `pc` / `sp` are kept because they are the two fields a person
/// auditing a snapshot actually asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpectrumSnapshotFacts {
    pub format: SnapshotFormat,
    /// Program counter, when the format states it directly. A 48K `.sna`
    /// keeps `PC` on the stack rather than in the header, so this is `None`
    /// for that form.
    pub pc: Option<u16>,
    /// Stack pointer, when the header carries it.
    pub sp: Option<u16>,
    /// Whether the snapshot's memory image is stored compressed. `None` when
    /// the format has no single compression flag (`.szx` compresses
    /// per-block).
    pub compressed: Option<bool>,
    pub machine: MachineEvidence,
    /// The raw hardware-mode byte(s), exactly as stored, for a `.z80` v2/v3
    /// or `.szx`. Kept even when it decoded cleanly, so an auditor can see
    /// the source value.
    pub raw_hardware_mode: Option<u16>,
}

/// Why a `.z80` / `.sna` / `.szx` was not accepted as a valid snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotRefusal {
    NotReadable(String),
    TooLarge {
        bytes: u64,
        maximum: u64,
    },
    TooSmall {
        bytes: u64,
        minimum: u64,
    },
    /// The bytes carry no recognisable snapshot structure for the extension
    /// they were dispatched under.
    NotRecognised {
        detail: String,
    },
    /// The structure was recognised at some level but a later field is
    /// inconsistent (a page table that runs past the file, an extra-header
    /// length that is not a documented value, ...).
    Malformed {
        detail: String,
    },
    NoExtension,
    UnsupportedExtension(String),
}

impl std::fmt::Display for SnapshotRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReadable(detail) => write!(f, "not readable: {detail}"),
            Self::TooLarge { bytes, maximum } => {
                write!(f, "{bytes} bytes is past the {maximum}-byte snapshot limit")
            }
            Self::TooSmall { bytes, minimum } => {
                write!(
                    f,
                    "{bytes} bytes is below the {minimum} bytes this form needs"
                )
            }
            Self::NotRecognised { detail } => write!(f, "not a recognised snapshot: {detail}"),
            Self::Malformed { detail } => write!(f, "snapshot structure is malformed: {detail}"),
            Self::NoExtension => f.write_str("the file has no extension to dispatch on"),
            Self::UnsupportedExtension(ext) => {
                write!(f, "`.{ext}` is not a snapshot this module reads")
            }
        }
    }
}

impl std::error::Error for SnapshotRefusal {}

// --- helpers ------------------------------------------------------------

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

// --- Z80 --------------------------------------------------------------------

const Z80_V1_HEADER_BYTES: usize = 30;
/// v1: 30-byte header + 48K of RAM, uncompressed.
const Z80_V1_UNCOMPRESSED_LEN: usize = Z80_V1_HEADER_BYTES + 49152;
/// The end-of-memory marker an "old" compressed v1 block always finishes with.
const Z80_V1_COMPRESSION_TAIL: [u8; 4] = [0x00, 0xED, 0xED, 0x00];
const Z80_V2_EXTRA_HEADER_LEN: u16 = 23;
const Z80_V3_EXTRA_HEADER_LEN: u16 = 54;
const Z80_V3_EXTRA_HEADER_LEN_PLUS3: u16 = 55;
/// A per-page block header: 2-byte LE length, 1-byte page number.
const Z80_PAGE_HEADER_BYTES: usize = 3;
/// No real snapshot lists more pages than this; a hostile file cannot make
/// the walk long.
const Z80_MAX_PAGES: usize = 32;
/// The sentinel length meaning "this page is stored uncompressed".
const Z80_PAGE_UNCOMPRESSED_MARKER: u16 = 0xFFFF;
const Z80_PAGE_DATA_BYTES: usize = 16384;

/// Parses a `.z80` snapshot (v1/v2/v3). `None` is never returned as a partial
/// success - either every checked field is consistent or this fails closed.
pub fn parse_z80_snapshot(bytes: &[u8]) -> Result<SpectrumSnapshotFacts, SnapshotRefusal> {
    if bytes.len() < Z80_V1_HEADER_BYTES {
        return Err(SnapshotRefusal::TooSmall {
            bytes: bytes.len() as u64,
            minimum: Z80_V1_HEADER_BYTES as u64,
        });
    }

    let pc_v1 = le_u16(bytes, 6).expect("30-byte header checked above");
    let flags1_raw = bytes[12];
    // A stored 255 is read as 0 for backward compatibility - the documented
    // quirk, not a guess.
    let flags1 = if flags1_raw == 0xFF { 0 } else { flags1_raw };
    let border = (flags1 >> 1) & 0x07;
    let iff1 = bytes[27];
    let iff2 = bytes[28];
    let interrupt_mode = bytes[29] & 0x03;

    // Fields shared by every generation must be sane before anything else.
    if iff1 > 1 || iff2 > 1 {
        return Err(SnapshotRefusal::Malformed {
            detail: format!("interrupt-flip-flop bytes are {iff1}/{iff2}, not 0/1"),
        });
    }
    if interrupt_mode == 3 {
        return Err(SnapshotRefusal::Malformed {
            detail: "interrupt mode 3 is not a valid Z80 interrupt mode".to_string(),
        });
    }
    let _ = border;

    if pc_v1 != 0 {
        // --- v1 --------------------------------------------------------
        let compressed = flags1 & 0x20 != 0;
        if compressed {
            if bytes.len() < Z80_V1_HEADER_BYTES + Z80_V1_COMPRESSION_TAIL.len()
                || bytes[bytes.len() - 4..] != Z80_V1_COMPRESSION_TAIL
            {
                return Err(SnapshotRefusal::Malformed {
                    detail: "a compressed v1 .z80 must end with the 00 ED ED 00 memory marker"
                        .to_string(),
                });
            }
        } else if bytes.len() != Z80_V1_UNCOMPRESSED_LEN {
            return Err(SnapshotRefusal::Malformed {
                detail: format!(
                    "an uncompressed v1 .z80 is exactly {Z80_V1_UNCOMPRESSED_LEN} bytes, this is {}",
                    bytes.len()
                ),
            });
        }
        return Ok(SpectrumSnapshotFacts {
            format: SnapshotFormat::Z80V1,
            pc: Some(pc_v1),
            sp: Some(le_u16(bytes, 8).expect("header length checked")),
            compressed: Some(compressed),
            // v1 is a 48K machine by the format's definition; no byte says so.
            machine: MachineEvidence::ImpliedByFormat(SpectrumMachine::Spectrum48K),
            raw_hardware_mode: None,
        });
    }

    // --- v2 / v3 ------------------------------------------------------------
    let extra_len = le_u16(bytes, 30).ok_or_else(|| SnapshotRefusal::Malformed {
        detail: "no extended-header length word at offset 30".to_string(),
    })?;
    let generation = match extra_len {
        Z80_V2_EXTRA_HEADER_LEN => SnapshotFormat::Z80V2,
        Z80_V3_EXTRA_HEADER_LEN | Z80_V3_EXTRA_HEADER_LEN_PLUS3 => SnapshotFormat::Z80V3,
        other => {
            return Err(SnapshotRefusal::Malformed {
                detail: format!(
                    "extended-header length {other} is not a documented value (23, 54 or 55)"
                ),
            });
        }
    };
    let extra_start = 32usize;
    let extra_end =
        extra_start
            .checked_add(extra_len as usize)
            .ok_or_else(|| SnapshotRefusal::Malformed {
                detail: "extended header length overflows".to_string(),
            })?;
    if bytes.len() < extra_end {
        return Err(SnapshotRefusal::Malformed {
            detail: format!("file ends before its {extra_len}-byte extended header does"),
        });
    }
    let pc = le_u16(bytes, 32).expect("extra header length >= 2 guaranteed above");
    let hardware_mode = bytes[34];
    let modified_hardware = bytes[37] & 0x80 != 0;

    let machine = decode_z80_hardware_mode(generation, hardware_mode, modified_hardware);

    // Walk the page table to end-of-file. Each step advances by a length that
    // has already been proven to fit, so the walk is finite and never leaves
    // the file.
    let mut offset = extra_end;
    let mut pages = 0usize;
    while offset < bytes.len() {
        if pages >= Z80_MAX_PAGES {
            return Err(SnapshotRefusal::Malformed {
                detail: format!("more than {Z80_MAX_PAGES} memory pages"),
            });
        }
        let header_end = offset
            .checked_add(Z80_PAGE_HEADER_BYTES)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| SnapshotRefusal::Malformed {
                detail: "a memory-page header runs past the end of the file".to_string(),
            })?;
        let page_len = le_u16(bytes, offset).expect("3 bytes available");
        let page_number = bytes[offset + 2];
        if page_number > 11 {
            return Err(SnapshotRefusal::Malformed {
                detail: format!("memory-page number {page_number} is outside 0..=11"),
            });
        }
        let data_len = if page_len == Z80_PAGE_UNCOMPRESSED_MARKER {
            Z80_PAGE_DATA_BYTES
        } else {
            page_len as usize
        };
        let page_end = header_end
            .checked_add(data_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| SnapshotRefusal::Malformed {
                detail: "a memory page declares more data than the file contains".to_string(),
            })?;
        offset = page_end;
        pages += 1;
    }
    if pages == 0 {
        return Err(SnapshotRefusal::Malformed {
            detail: "a v2/v3 .z80 has at least one memory page".to_string(),
        });
    }

    Ok(SpectrumSnapshotFacts {
        format: generation,
        pc: Some(pc),
        sp: Some(le_u16(bytes, 8).expect("v1 header present")),
        compressed: Some(true),
        machine,
        raw_hardware_mode: Some(u16::from(hardware_mode)),
    })
}

/// Interprets the `.z80` hardware-mode byte **for the pinned generation**.
/// The same value means different machines in a v2 versus a v3 file, so the
/// generation is a required input, not something re-derived here.
fn decode_z80_hardware_mode(
    generation: SnapshotFormat,
    mode: u8,
    modified_hardware: bool,
) -> MachineEvidence {
    use SpectrumMachine::*;
    let known = |machine| MachineEvidence::Encoded(machine);
    match generation {
        SnapshotFormat::Z80V2 => match mode {
            0 | 1 => known(Spectrum48K),
            2 => known(OtherDocumented("SamRam")),
            3 | 4 if modified_hardware => MachineEvidence::Encoded(SpectrumPlus2),
            3 | 4 => known(Spectrum128K),
            _ => MachineEvidence::EncodedButUndocumented {
                raw: u16::from(mode),
            },
        },
        SnapshotFormat::Z80V3 => match mode {
            0 | 1 | 3 => known(Spectrum48K),
            2 => known(OtherDocumented("SamRam")),
            4 | 5 | 6 if modified_hardware => MachineEvidence::Encoded(SpectrumPlus2),
            4 | 5 | 6 => known(Spectrum128K),
            7 | 8 if modified_hardware => MachineEvidence::Encoded(SpectrumPlus2A),
            7 | 8 => known(SpectrumPlus3),
            9 => known(Pentagon),
            10 => known(Scorpion),
            11 => known(OtherDocumented("Didaktik-Kompakt")),
            12 => known(SpectrumPlus2),
            13 => known(SpectrumPlus2A),
            14 => known(OtherDocumented("Timex TC2048")),
            15 => known(OtherDocumented("Timex TC2068")),
            128 => known(OtherDocumented("Timex TS2068")),
            _ => MachineEvidence::EncodedButUndocumented {
                raw: u16::from(mode),
            },
        },
        _ => MachineEvidence::EncodedButUndocumented {
            raw: u16::from(mode),
        },
    }
}

// --- SNA -----------------------------------------------------------------

const SNA_HEADER_BYTES: usize = 27;
const SNA_48K_LEN: usize = SNA_HEADER_BYTES + 49152; // 49179
/// 27-byte header + 3 pages + PC(2) + port 0x7FFD(1) + TR-DOS(1) + 5 pages.
const SNA_128K_LEN: usize = SNA_HEADER_BYTES + 49152 + 4 + (5 * 16384); // 131103

/// Parses a `.sna` snapshot. The two accepted forms are distinguished only
/// by exact total size; every other length fails closed.
pub fn parse_sna_snapshot(bytes: &[u8]) -> Result<SpectrumSnapshotFacts, SnapshotRefusal> {
    match bytes.len() {
        SNA_48K_LEN => {}
        SNA_128K_LEN => {}
        len if len < SNA_48K_LEN => {
            return Err(SnapshotRefusal::TooSmall {
                bytes: len as u64,
                minimum: SNA_48K_LEN as u64,
            });
        }
        len => {
            return Err(SnapshotRefusal::Malformed {
                detail: format!(
                    "a .sna is exactly {SNA_48K_LEN} (48K) or {SNA_128K_LEN} (128K) bytes, this is {len}"
                ),
            });
        }
    }

    let interrupt_mode = bytes[25];
    let border = bytes[26];
    if interrupt_mode > 2 {
        return Err(SnapshotRefusal::Malformed {
            detail: format!("interrupt-mode byte is {interrupt_mode}, not 0/1/2"),
        });
    }
    if border > 7 {
        return Err(SnapshotRefusal::Malformed {
            detail: format!("border-colour byte is {border}, not 0..=7"),
        });
    }
    let sp = le_u16(bytes, 23).expect("header present");

    if bytes.len() == SNA_48K_LEN {
        // PC is popped from the stack, so SP must point into RAM for the
        // snapshot to be resumable at all - a real discriminator against
        // random bytes that happened to have a small mode/border byte.
        if sp < 0x4000 || sp > 0xFFFE {
            return Err(SnapshotRefusal::Malformed {
                detail: format!(
                    "a 48K .sna keeps PC on the stack, so SP ({sp:#06x}) must be in RAM (0x4000..=0xFFFE)"
                ),
            });
        }
        return Ok(SpectrumSnapshotFacts {
            format: SnapshotFormat::Sna48K,
            pc: None,
            sp: Some(sp),
            compressed: Some(false),
            machine: MachineEvidence::ImpliedByFormat(SpectrumMachine::Spectrum48K),
            raw_hardware_mode: None,
        });
    }

    // 128K: PC / port 0x7FFD / TR-DOS flag sit right after the first three
    // 16K pages.
    let extension_at = SNA_HEADER_BYTES + (3 * 16384);
    let pc = le_u16(bytes, extension_at).expect("length is exactly SNA_128K_LEN");
    let port_7ffd = bytes[extension_at + 2];
    let trdos = bytes[extension_at + 3];
    if trdos > 1 {
        return Err(SnapshotRefusal::Malformed {
            detail: format!("the TR-DOS-paged flag is {trdos}, not 0/1"),
        });
    }
    let _ = port_7ffd;
    Ok(SpectrumSnapshotFacts {
        format: SnapshotFormat::Sna128K,
        pc: Some(pc),
        sp: Some(sp),
        compressed: Some(false),
        // The 128K .sna form itself is what proves a 128K machine; there is
        // no finer subtype byte (+2/+3 is not distinguished by this format).
        machine: MachineEvidence::ImpliedByFormat(SpectrumMachine::Spectrum128K),
        raw_hardware_mode: None,
    })
}

// --- SZX ---------------------------------------------------------------

const SZX_MAGIC: &[u8; 4] = b"ZXST";
const SZX_HEADER_BYTES: usize = 8;
const SZX_BLOCK_HEADER_BYTES: usize = 8;
const SZX_MAX_BLOCKS: usize = 128;

/// Parses the `.szx` (ZX-State) container header and walks its block table
/// to end-of-file. Block *payloads* are never decoded.
pub fn parse_szx_snapshot(bytes: &[u8]) -> Result<SpectrumSnapshotFacts, SnapshotRefusal> {
    if bytes.len() < SZX_HEADER_BYTES {
        return Err(SnapshotRefusal::TooSmall {
            bytes: bytes.len() as u64,
            minimum: SZX_HEADER_BYTES as u64,
        });
    }
    if &bytes[0..4] != SZX_MAGIC {
        return Err(SnapshotRefusal::NotRecognised {
            detail: "does not begin with the `ZXST` signature".to_string(),
        });
    }
    let machine_id = bytes[6];
    let machine = decode_szx_machine_id(machine_id);

    let mut offset = SZX_HEADER_BYTES;
    let mut blocks = 0usize;
    while offset < bytes.len() {
        if blocks >= SZX_MAX_BLOCKS {
            return Err(SnapshotRefusal::Malformed {
                detail: format!("more than {SZX_MAX_BLOCKS} ZX-State blocks"),
            });
        }
        let header_end = offset.checked_add(SZX_BLOCK_HEADER_BYTES).ok_or_else(|| {
            SnapshotRefusal::Malformed {
                detail: "a block header offset overflows".to_string(),
            }
        })?;
        if header_end > bytes.len() {
            return Err(SnapshotRefusal::Malformed {
                detail: "a ZX-State block header runs past the end of the file".to_string(),
            });
        }
        let size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]);
        let block_end = header_end
            .checked_add(size as usize)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| SnapshotRefusal::Malformed {
                detail: "a ZX-State block declares more data than the file contains".to_string(),
            })?;
        offset = block_end;
        blocks += 1;
    }

    Ok(SpectrumSnapshotFacts {
        format: SnapshotFormat::Szx,
        pc: None,
        sp: None,
        compressed: None,
        machine,
        raw_hardware_mode: Some(u16::from(machine_id)),
    })
}

fn decode_szx_machine_id(id: u8) -> MachineEvidence {
    use SpectrumMachine::*;
    let known = MachineEvidence::Encoded;
    match id {
        0 => known(Spectrum16K),
        1 => known(Spectrum48K),
        2 => known(Spectrum128K),
        3 => known(SpectrumPlus2),
        4 => known(SpectrumPlus2A),
        5 => known(SpectrumPlus3),
        6 => known(SpectrumPlus3), // +3e - the +3 with a modified ROM set
        7 => known(Pentagon),      // Pentagon 128
        8 => known(OtherDocumented("Timex TC2048")),
        9 => known(OtherDocumented("Timex TC2068")),
        10 => known(Scorpion),
        11 => known(OtherDocumented("ZX Spectrum SE")),
        12 => known(OtherDocumented("Timex TS2068")),
        13 | 14 => known(Pentagon), // Pentagon 512 / 1024
        16 => known(Spectrum128K),  // 128Ke
        _ => MachineEvidence::EncodedButUndocumented { raw: u16::from(id) },
    }
}

// --- shared evidence -------------------------------------------------------

/// Turns parsed facts into neutral [`ContentEvidence`]. Two items are always
/// produced: a `Strong` [`ContentEvidenceKind::ContentSignature`] for the
/// **container** (axis A/B - a valid snapshot of a supported family), and one
/// for the **machine** whose confidence is exactly what the format encoded
/// (axis C). No item is a platform claim; the Sinclair platform bridge lives
/// in the ingestion/detection layers, not here.
pub fn observe_spectrum_snapshot_evidence(facts: &SpectrumSnapshotFacts) -> Vec<ContentEvidence> {
    let mut evidence = Vec::with_capacity(2);

    let mut container_detail = format!("{} validated from its structure", facts.format.label());
    if let Some(pc) = facts.pc {
        container_detail.push_str(&format!("; PC {pc:#06x}"));
    }
    if let Some(sp) = facts.sp {
        container_detail.push_str(&format!("; SP {sp:#06x}"));
    }
    if let Some(compressed) = facts.compressed {
        container_detail.push_str(if compressed {
            "; memory image compressed"
        } else {
            "; memory image uncompressed"
        });
    }
    evidence.push(ContentEvidence::new(
        ContentEvidenceKind::ContentSignature,
        facts.format.label(),
        ContentEvidenceConfidence::Strong,
        container_detail,
    ));

    let machine_detail = match facts.machine {
        MachineEvidence::Encoded(machine) => format!(
            "hardware value {} decodes to {}",
            facts
                .raw_hardware_mode
                .map(|raw| format!("{raw}"))
                .unwrap_or_else(|| "in the header".to_string()),
            machine.label()
        ),
        MachineEvidence::ImpliedByFormat(machine) => format!(
            "{} is implied by the {} form; no machine byte is present",
            machine.label(),
            facts.format.label()
        ),
        MachineEvidence::EncodedButUndocumented { raw } => format!(
            "hardware value {raw} is outside every documented case for {}; preserved as unknown, \
             no subtype inferred",
            facts.format.label()
        ),
        MachineEvidence::Absent => {
            format!("{} carries no machine identifier", facts.format.label())
        }
    };
    let machine_value = facts
        .machine
        .machine()
        .map(SpectrumMachine::label)
        .unwrap_or("ZX Spectrum (machine subtype not encoded)");
    evidence.push(ContentEvidence::new(
        ContentEvidenceKind::ContentSignature,
        machine_value,
        facts.machine.confidence(),
        machine_detail,
    ));

    evidence
}

// --- file entry point ---------------------------------------------------

/// The whole result of inspecting one snapshot file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpectrumSnapshotInspection {
    pub facts: SpectrumSnapshotFacts,
    /// Bytes actually read (the whole file, capped at [`MAX_SNAPSHOT_BYTES`]).
    pub bytes_inspected: u64,
}

impl SpectrumSnapshotInspection {
    /// Axis **C**: the machine subtype was genuinely present in the bytes.
    pub fn machine_subtype_is_encoded(&self) -> bool {
        matches!(self.facts.machine, MachineEvidence::Encoded(_))
    }

    /// The best machine the snapshot supports, if any.
    pub fn machine(&self) -> Option<SpectrumMachine> {
        self.facts.machine.machine()
    }
}

/// Reads `path` (bounded, size-checked from metadata first) and parses it as
/// the snapshot its extension names. Fails closed on any inconsistency.
///
/// Dispatch is by extension because that is what says *which structure to
/// look for*; the answer still comes only from the bytes - a `.z80` whose
/// header does not validate is refused, not downgraded to a filename claim.
pub fn inspect_spectrum_snapshot_file(
    path: &Path,
) -> Result<SpectrumSnapshotInspection, SnapshotRefusal> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or(SnapshotRefusal::NoExtension)?;
    if !matches!(extension.as_str(), "z80" | "sna" | "szx") {
        return Err(SnapshotRefusal::UnsupportedExtension(extension));
    }

    let metadata =
        std::fs::metadata(path).map_err(|error| SnapshotRefusal::NotReadable(error.to_string()))?;
    let length = metadata.len();
    if length > MAX_SNAPSHOT_BYTES {
        return Err(SnapshotRefusal::TooLarge {
            bytes: length,
            maximum: MAX_SNAPSHOT_BYTES,
        });
    }
    let bytes =
        std::fs::read(path).map_err(|error| SnapshotRefusal::NotReadable(error.to_string()))?;

    let facts = match extension.as_str() {
        "z80" => parse_z80_snapshot(&bytes)?,
        "sna" => parse_sna_snapshot(&bytes)?,
        "szx" => parse_szx_snapshot(&bytes)?,
        _ => unreachable!("extension set checked above"),
    };
    Ok(SpectrumSnapshotInspection {
        facts,
        bytes_inspected: bytes.len() as u64,
    })
}

// --- ContentDetector adapters ----------------------------------------------

/// [`ContentDetector`] over [`parse_z80_snapshot`], for multi-detector
/// callers that already hold the bytes.
pub struct Z80SnapshotDetector;

impl ContentDetector for Z80SnapshotDetector {
    fn id(&self) -> &'static str {
        "zx_spectrum_z80_snapshot"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        match parse_z80_snapshot(data) {
            Ok(facts) => ContentDetectionOutcome::Recognized {
                evidence: observe_spectrum_snapshot_evidence(&facts),
            },
            Err(_) => ContentDetectionOutcome::NotRecognized,
        }
    }
}

/// [`ContentDetector`] over [`parse_sna_snapshot`].
pub struct SnaSnapshotDetector;

impl ContentDetector for SnaSnapshotDetector {
    fn id(&self) -> &'static str {
        "zx_spectrum_sna_snapshot"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        match parse_sna_snapshot(data) {
            Ok(facts) => ContentDetectionOutcome::Recognized {
                evidence: observe_spectrum_snapshot_evidence(&facts),
            },
            Err(_) => ContentDetectionOutcome::NotRecognized,
        }
    }
}

/// [`ContentDetector`] over [`parse_szx_snapshot`].
pub struct SzxSnapshotDetector;

impl ContentDetector for SzxSnapshotDetector {
    fn id(&self) -> &'static str {
        "zx_spectrum_szx_snapshot"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        match parse_szx_snapshot(data) {
            Ok(facts) => ContentDetectionOutcome::Recognized {
                evidence: observe_spectrum_snapshot_evidence(&facts),
            },
            Err(_) => ContentDetectionOutcome::NotRecognized,
        }
    }
}

#[cfg(test)]
mod tests;
