//! Pure, read-only structural inspection of the ZX Spectrum media formats
//! that sit beside the snapshot ([`crate::zx_spectrum_snapshot`]), TR-DOS
//! ([`crate::disk_format`]) and TAP/TZX ([`crate::tape_identity`]) readers:
//!
//! * `.pzx` - PZX ("Perfect ZX") pulse tape, Patrik Rak's published PZX
//!   format specification v1.0 (`PZXT` header block, then `PULS` / `DATA` /
//!   `PAUS` / `BRWS` / `STOP` blocks, each `{ 4-byte tag, 4-byte LE size,
//!   data }`).
//! * `.rzx` - RZX input recording, the RZX v0.13 specification
//!   (`RZX!`, major, minor, 4-byte flags, then `{ id, 4-byte LE length
//!   including these 5 bytes, data }` blocks).
//! * `.nex` - ZX Spectrum Next program file, the NEX V1.0-V1.2 format
//!   (512-byte header: `Next`, `V1.x`, RAM-required byte, bank count, and a
//!   112-byte bank-presence table at offset 18).
//! * `.udi` - UDI ("Ultra Disk Image") raw floppy image, UDI v1.0
//!   (`UDI!`, file size excluding the 4-byte CRC, version 0, maximum cylinder,
//!   maximum side, extended header, then per-track records).
//! * `.mdr` - ZX Interface 1 Microdrive cartridge image as written by the
//!   mainstream Spectrum emulators: N sectors of 543 bytes (15-byte header,
//!   15-byte record descriptor, 512 data bytes, data checksum) followed by one
//!   write-protect byte. Each of the three parts ends in the Interface 1
//!   checksum (byte sum modulo 255), which is what proves the format.
//!
//! # What a result means
//!
//! Every result carries a [`SpectrumMediaStatus`]. Only a full structural walk
//! that reached exactly the end of the file is
//! [`SpectrumMediaStatus::StructurallyValidated`]; a format whose integrity
//! fields this pass deliberately does not check (UDI's trailing CRC), or
//! whose checksums show damaged parts (a Microdrive with bad sectors), is at
//! most [`SpectrumMediaStatus::PartiallyUnderstood`].
//! Neither is a claim that the media runs, that it is a good dump, or which
//! game it is: launchability is the launch planner's decision, and identity
//! still needs a DAT/catalogue match.
//!
//! [`SpectrumMediaRole::PreservationOnly`] marks media that is kept as a
//! library record but is not a game entry and is never offered for play (an
//! RZX recording replays input against a snapshot; it is not itself the
//! game).
//!
//! # What this module does not do
//!
//! It never decompresses an RZX snapshot or input block, never decodes pulse
//! timings, never verifies an RZX signature, never reconstructs a disk
//! filesystem, and never reads a title. Every read is bounded by the
//! per-format cap below, taken from filesystem metadata before reading, and a
//! file that changes while it is read is refused.

use std::io::Read;
use std::path::Path;

use crate::zx_spectrum_snapshot::{
    MachineEvidence, SpectrumMachine, parse_sna_snapshot, parse_szx_snapshot, parse_z80_snapshot,
};

pub const MAX_PZX_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_RZX_BYTES: u64 = 32 * 1024 * 1024;
/// 512-byte header, up to ~50 KB of loading screens, 112 banks of 16K, plus
/// room for the documented trailing extra data.
pub const MAX_NEX_BYTES: u64 = 4 * 1024 * 1024;
/// 84 cylinders x 2 sides of a generous raw MFM track plus sync bitmap is
/// well under this.
pub const MAX_UDI_BYTES: u64 = 4 * 1024 * 1024;
pub const MDR_SECTOR_BYTES: usize = 543;
pub const MDR_MAX_SECTORS: usize = 254;
pub const MAX_MDR_BYTES: u64 = (MDR_SECTOR_BYTES * MDR_MAX_SECTORS + 1) as u64;
const MAX_BLOCKS: usize = 65_536;

/// The six-way support classification every inspected Spectrum file gets.
/// "Launchable through a confirmed emulator" is deliberately not one of
/// these: it is decided later by launch readiness, never by a parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectrumMediaStatus {
    /// Every structural field this format defines was checked and the walk
    /// ended exactly at the end of the file.
    StructurallyValidated,
    /// Recognised, and the parts that were checked are consistent, but some
    /// integrity data is not verified by this pass (named in the notes).
    PartiallyUnderstood,
    /// A real file of this format, but a version or variant this pass does
    /// not read. Not evidence of damage.
    UnsupportedVariant,
    /// Recognised but signed or otherwise protected in a way that needs
    /// specialist handling (an RZX with a security signature).
    Protected,
}

impl SpectrumMediaStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::StructurallyValidated => "Recognised and structurally validated",
            Self::PartiallyUnderstood => "Recognised, partially understood",
            Self::UnsupportedVariant => "Unsupported variant",
            Self::Protected => "Protected - needs specialist handling",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectrumMediaRole {
    /// Media that can stand as a library entry in its own right.
    Playable,
    /// Kept and described, but not a standalone game entry.
    PreservationOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectrumMediaFormat {
    PzxTape,
    RzxRecording,
    NextProgram,
    UdiDisk,
    MicrodriveCartridge,
}

impl SpectrumMediaFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::PzxTape => "PZX tape",
            Self::RzxRecording => "RZX input recording",
            Self::NextProgram => "ZX Spectrum Next program (NEX)",
            Self::UdiDisk => "UDI disk image",
            Self::MicrodriveCartridge => "Microdrive cartridge (MDR)",
        }
    }

    pub fn for_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "pzx" => Some(Self::PzxTape),
            "rzx" => Some(Self::RzxRecording),
            "nex" => Some(Self::NextProgram),
            "udi" => Some(Self::UdiDisk),
            "mdr" => Some(Self::MicrodriveCartridge),
            _ => None,
        }
    }

    fn byte_limit(self) -> u64 {
        match self {
            Self::PzxTape => MAX_PZX_BYTES,
            Self::RzxRecording => MAX_RZX_BYTES,
            Self::NextProgram => MAX_NEX_BYTES,
            Self::UdiDisk => MAX_UDI_BYTES,
            Self::MicrodriveCartridge => MAX_MDR_BYTES,
        }
    }
}

/// The structural result of one inspected file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpectrumMediaInspection {
    pub format: SpectrumMediaFormat,
    pub status: SpectrumMediaStatus,
    pub role: SpectrumMediaRole,
    /// The machine the bytes require, kept apart from any display name. Only
    /// [`MachineEvidence::Encoded`] when a documented field states it.
    pub machine: MachineEvidence,
    /// A short structural summary ("3 blocks: 2 DATA, 1 PAUS").
    pub summary: String,
    /// What this pass did not verify, in plain words.
    pub notes: Vec<String>,
}

impl SpectrumMediaInspection {
    fn new(
        format: SpectrumMediaFormat,
        status: SpectrumMediaStatus,
        machine: MachineEvidence,
        summary: String,
    ) -> Self {
        Self {
            format,
            status,
            role: SpectrumMediaRole::Playable,
            machine,
            summary,
            notes: Vec::new(),
        }
    }

    /// Whether this may become a library record. Playable media needs a
    /// checked structure; a preservation-only recording is also kept when it
    /// is signed (its block chain was still walked). An unsupported variant
    /// never is.
    pub fn catalogue_eligible(&self) -> bool {
        match self.status {
            SpectrumMediaStatus::StructurallyValidated
            | SpectrumMediaStatus::PartiallyUnderstood => true,
            SpectrumMediaStatus::Protected => self.role == SpectrumMediaRole::PreservationOnly,
            SpectrumMediaStatus::UnsupportedVariant => false,
        }
    }

    /// One line for discovery/preview explanations.
    pub fn describe(&self) -> String {
        let machine = match self.machine {
            MachineEvidence::Encoded(machine) => format!("requires {}", machine.label()),
            MachineEvidence::ImpliedByFormat(machine) => {
                format!("{} (implied by the format, not encoded)", machine.label())
            }
            MachineEvidence::EncodedButUndocumented { raw } => {
                format!("undocumented machine value {raw}")
            }
            MachineEvidence::Absent => "machine not encoded".to_string(),
        };
        let mut line = format!(
            "{} - {}; {}; {machine}",
            self.format.label(),
            self.status.label(),
            self.summary
        );
        for note in &self.notes {
            line.push_str("; ");
            line.push_str(note);
        }
        line
    }
}

/// Why a file was not accepted as the format its extension names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpectrumMediaRefusal {
    /// Host I/O failed or the file changed while being read.
    NotReadable(String),
    /// Bigger than this pass reads. A reader limit, not damage.
    TooLarge {
        bytes: u64,
        maximum: u64,
    },
    /// The bytes do not carry this format's signature or layout.
    NotRecognised(String),
    /// Recognised, but the file ends before its own structure does.
    Truncated(String),
    /// Recognised, but a field is inconsistent with the rest of the file.
    Corrupt(String),
    /// The layout fits this format, but nothing in the bytes proves it.
    Ambiguous(String),
    UnsupportedExtension(String),
}

impl std::fmt::Display for SpectrumMediaRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReadable(detail) => write!(f, "not readable: {detail}"),
            Self::TooLarge { bytes, maximum } => {
                write!(
                    f,
                    "{bytes} bytes is past the {maximum}-byte inspection limit"
                )
            }
            Self::NotRecognised(detail) => write!(f, "not recognised: {detail}"),
            Self::Truncated(detail) => write!(f, "truncated: {detail}"),
            Self::Corrupt(detail) => write!(f, "corrupt: {detail}"),
            Self::Ambiguous(detail) => write!(f, "ambiguous: {detail}"),
            Self::UnsupportedExtension(ext) => {
                write!(f, "`.{ext}` is not a format this inspector reads")
            }
        }
    }
}

impl std::error::Error for SpectrumMediaRefusal {}

type Inspected = Result<SpectrumMediaInspection, SpectrumMediaRefusal>;

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

// --- PZX --------------------------------------------------------------------

/// Parses a PZX tape. Every known block's internal sizes are checked; unknown
/// tags are allowed by the specification and are skipped by their size.
pub fn parse_pzx(bytes: &[u8]) -> Inspected {
    let format = SpectrumMediaFormat::PzxTape;
    if bytes.get(0..4) != Some(b"PZXT") {
        return Err(SpectrumMediaRefusal::NotRecognised(
            "a PZX file starts with a PZXT header block".into(),
        ));
    }
    let mut offset = 0usize;
    let mut blocks = 0usize;
    let (mut data, mut pulses, mut pauses, mut other) = (0usize, 0usize, 0usize, 0usize);
    let mut version = None;
    while offset < bytes.len() {
        if blocks >= MAX_BLOCKS {
            return Err(SpectrumMediaRefusal::Corrupt(format!(
                "more than {MAX_BLOCKS} blocks"
            )));
        }
        let tag = bytes
            .get(offset..offset + 4)
            .ok_or_else(|| SpectrumMediaRefusal::Truncated("block tag cut short".into()))?;
        let size = le_u32(bytes, offset + 4)
            .ok_or_else(|| SpectrumMediaRefusal::Truncated("block size cut short".into()))?
            as usize;
        let start = offset + 8;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| {
                SpectrumMediaRefusal::Truncated(format!(
                    "block {} declares more data than the file contains",
                    blocks + 1
                ))
            })?;
        let body = &bytes[start..end];
        match tag {
            b"PZXT" => {
                if blocks != 0 {
                    return Err(SpectrumMediaRefusal::Corrupt(
                        "a second PZXT header block".into(),
                    ));
                }
                if body.len() < 2 {
                    return Err(SpectrumMediaRefusal::Corrupt(
                        "the PZXT block has no version bytes".into(),
                    ));
                }
                version = Some((body[0], body[1]));
                if body[0] != 1 {
                    // Later major versions may redefine every block, so
                    // nothing after the header is interpreted.
                    return Ok(SpectrumMediaInspection::new(
                        format,
                        SpectrumMediaStatus::UnsupportedVariant,
                        MachineEvidence::Absent,
                        format!("PZX v{}.{}", body[0], body[1]),
                    ));
                }
            }
            b"PULS" => {
                if size % 2 != 0 {
                    return Err(SpectrumMediaRefusal::Corrupt(
                        "a PULS block is a whole number of 16-bit words".into(),
                    ));
                }
                pulses += 1;
            }
            b"DATA" => {
                let fixed = 8usize;
                if body.len() < fixed {
                    return Err(SpectrumMediaRefusal::Corrupt(
                        "a DATA block is shorter than its fixed fields".into(),
                    ));
                }
                let bit_count = (le_u32(body, 0).expect("8 bytes") & 0x7FFF_FFFF) as usize;
                let p0 = body[6] as usize;
                let p1 = body[7] as usize;
                let expected = fixed + 2 * (p0 + p1) + bit_count.div_ceil(8);
                if expected != body.len() {
                    return Err(SpectrumMediaRefusal::Corrupt(format!(
                        "DATA block {} is {} bytes but its bit count and pulse sequences need {expected}",
                        blocks + 1,
                        body.len()
                    )));
                }
                data += 1;
            }
            b"PAUS" => {
                if size != 4 {
                    return Err(SpectrumMediaRefusal::Corrupt(
                        "a PAUS block is exactly 4 bytes".into(),
                    ));
                }
                pauses += 1;
            }
            b"STOP" => {
                if size != 2 {
                    return Err(SpectrumMediaRefusal::Corrupt(
                        "a STOP block is exactly 2 bytes".into(),
                    ));
                }
                other += 1;
            }
            _ => other += 1,
        }
        offset = end;
        blocks += 1;
    }
    let (major, minor) = version.expect("first block is PZXT");
    let summary = format!(
        "PZX v{major}.{minor}, {blocks} blocks: {data} DATA, {pulses} PULS, {pauses} PAUS, {other} other"
    );
    if data + pulses == 0 {
        return Err(SpectrumMediaRefusal::Corrupt(
            "the tape has no PULS or DATA block, so nothing to load".into(),
        ));
    }
    let mut result = SpectrumMediaInspection::new(
        format,
        SpectrumMediaStatus::StructurallyValidated,
        MachineEvidence::Absent,
        summary,
    );
    result
        .notes
        .push("pulse timings are not decoded; loading is not proven".into());
    Ok(result)
}

// --- RZX --------------------------------------------------------------------

const RZX_HEADER_BYTES: usize = 10;
const RZX_BLOCK_HEADER_BYTES: usize = 5;

/// Parses an RZX recording's block chain. The embedded snapshot is examined
/// for its machine only when stored uncompressed; nothing is decompressed.
pub fn parse_rzx(bytes: &[u8]) -> Inspected {
    let format = SpectrumMediaFormat::RzxRecording;
    if bytes.get(0..4) != Some(b"RZX!") {
        return Err(SpectrumMediaRefusal::NotRecognised(
            "an RZX file starts with RZX!".into(),
        ));
    }
    if bytes.len() < RZX_HEADER_BYTES {
        return Err(SpectrumMediaRefusal::Truncated(
            "the RZX header is 10 bytes".into(),
        ));
    }
    let (major, minor) = (bytes[4], bytes[5]);
    let signed = le_u32(bytes, 6).expect("10 bytes") & 1 != 0;
    let mut offset = RZX_HEADER_BYTES;
    let mut blocks = 0usize;
    let (mut snapshots, mut inputs) = (0usize, 0usize);
    let mut security = false;
    let mut machine = MachineEvidence::Absent;
    let mut compressed_snapshot = false;
    while offset < bytes.len() {
        if blocks >= MAX_BLOCKS {
            return Err(SpectrumMediaRefusal::Corrupt(format!(
                "more than {MAX_BLOCKS} blocks"
            )));
        }
        let id = bytes[offset];
        let length = le_u32(bytes, offset + 1)
            .ok_or_else(|| SpectrumMediaRefusal::Truncated("block length cut short".into()))?
            as usize;
        if length < RZX_BLOCK_HEADER_BYTES {
            return Err(SpectrumMediaRefusal::Corrupt(format!(
                "block {} length {length} is smaller than its own header",
                blocks + 1
            )));
        }
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| {
                SpectrumMediaRefusal::Truncated(format!(
                    "block {} declares more data than the file contains",
                    blocks + 1
                ))
            })?;
        let body = &bytes[offset + RZX_BLOCK_HEADER_BYTES..end];
        match id {
            0x20 | 0x21 => security = true,
            0x30 => {
                // flags (4), extension (4), uncompressed length (4), data.
                if body.len() < 12 {
                    return Err(SpectrumMediaRefusal::Corrupt(
                        "a snapshot block is shorter than its fixed fields".into(),
                    ));
                }
                let flags = le_u32(body, 0).expect("12 bytes");
                let external = flags & 1 != 0;
                let compressed = flags & 2 != 0;
                if !external && compressed {
                    compressed_snapshot = true;
                } else if !external && snapshots == 0 {
                    let data = &body[12..];
                    let ext = &body[4..8];
                    let parsed = match ext {
                        b"z80\0" | b"Z80\0" => parse_z80_snapshot(data).ok(),
                        b"sna\0" | b"SNA\0" => parse_sna_snapshot(data).ok(),
                        b"szx\0" | b"SZX\0" => parse_szx_snapshot(data).ok(),
                        _ => None,
                    };
                    if let Some(facts) = parsed {
                        machine = facts.machine;
                    }
                }
                snapshots += 1;
            }
            0x80 => inputs += 1,
            _ => {}
        }
        offset = end;
        blocks += 1;
    }
    let summary = format!(
        "RZX v{major}.{minor}, {blocks} blocks: {snapshots} snapshot, {inputs} input recording"
    );
    let status = if major != 0 {
        SpectrumMediaStatus::UnsupportedVariant
    } else if signed || security {
        SpectrumMediaStatus::Protected
    } else {
        if inputs == 0 {
            return Err(SpectrumMediaRefusal::Corrupt(
                "the recording has no input-recording block".into(),
            ));
        }
        SpectrumMediaStatus::StructurallyValidated
    };
    let mut result = SpectrumMediaInspection::new(format, status, machine, summary);
    result.role = SpectrumMediaRole::PreservationOnly;
    if signed || security {
        result
            .notes
            .push("carries a security signature that is not verified".into());
    }
    if compressed_snapshot {
        result
            .notes
            .push("embedded snapshot is compressed and was not examined".into());
    }
    result
        .notes
        .push("an input recording replays a session; it is not a standalone game".into());
    Ok(result)
}

// --- NEX --------------------------------------------------------------------

const NEX_HEADER_BYTES: usize = 512;
const NEX_BANK_TABLE: std::ops::Range<usize> = 18..130;
const NEX_BANK_BYTES: u64 = 16 * 1024;

/// Parses a NEX header against the file's real length. Only the 512-byte
/// header is needed, so `file_len` is the metadata length, not `bytes.len()`.
pub fn parse_nex(header: &[u8], file_len: u64) -> Inspected {
    let format = SpectrumMediaFormat::NextProgram;
    if header.get(0..4) != Some(b"Next") {
        return Err(SpectrumMediaRefusal::NotRecognised(
            "a NEX file starts with Next".into(),
        ));
    }
    if header.len() < NEX_HEADER_BYTES || file_len < NEX_HEADER_BYTES as u64 {
        return Err(SpectrumMediaRefusal::Truncated(
            "the NEX header is 512 bytes".into(),
        ));
    }
    let version = &header[4..8];
    let machine = MachineEvidence::Encoded(SpectrumMachine::SpectrumNext);
    let version_text = String::from_utf8_lossy(version).into_owned();
    if !matches!(version, b"V1.0" | b"V1.1" | b"V1.2") {
        return Ok(SpectrumMediaInspection::new(
            format,
            SpectrumMediaStatus::UnsupportedVariant,
            machine,
            format!("NEX version {version_text:?}"),
        ));
    }
    let ram = header[8];
    let banks = header[9] as usize;
    if ram > 1 {
        return Err(SpectrumMediaRefusal::Corrupt(format!(
            "RAM-required value {ram} is not 0 (768K) or 1 (1792K)"
        )));
    }
    let table = &header[NEX_BANK_TABLE];
    if table.iter().any(|flag| *flag > 1) {
        return Err(SpectrumMediaRefusal::Corrupt(
            "the bank-presence table holds a value other than 0 or 1".into(),
        ));
    }
    let present = table.iter().filter(|flag| **flag == 1).count();
    if present != banks {
        return Err(SpectrumMediaRefusal::Corrupt(format!(
            "header says {banks} banks but the presence table lists {present}"
        )));
    }
    let needed = NEX_HEADER_BYTES as u64 + banks as u64 * NEX_BANK_BYTES;
    if file_len < needed {
        return Err(SpectrumMediaRefusal::Truncated(format!(
            "{banks} banks need at least {needed} bytes, the file is {file_len}"
        )));
    }
    let mut result = SpectrumMediaInspection::new(
        format,
        SpectrumMediaStatus::StructurallyValidated,
        machine,
        format!(
            "NEX {version_text}, {banks} banks, {} RAM",
            if ram == 0 { "768K" } else { "1792K" }
        ),
    );
    result
        .notes
        .push("loading-screen and bank contents are not examined".into());
    Ok(result)
}

// --- UDI --------------------------------------------------------------------

const UDI_HEADER_BYTES: usize = 16;
const UDI_CRC_BYTES: usize = 4;

/// Parses a UDI image header and walks every track record. The trailing
/// CRC32 is not verified, so a clean walk is "partially understood".
pub fn parse_udi(bytes: &[u8]) -> Inspected {
    let format = SpectrumMediaFormat::UdiDisk;
    if bytes.get(0..4) != Some(b"UDI!") {
        return Err(SpectrumMediaRefusal::NotRecognised(
            "a UDI file starts with UDI!".into(),
        ));
    }
    if bytes.len() < UDI_HEADER_BYTES + UDI_CRC_BYTES {
        return Err(SpectrumMediaRefusal::Truncated(
            "the UDI header is 16 bytes plus a 4-byte CRC".into(),
        ));
    }
    let declared = le_u32(bytes, 4).expect("header present") as usize;
    if declared.checked_add(UDI_CRC_BYTES) != Some(bytes.len()) {
        return Err(if declared + UDI_CRC_BYTES > bytes.len() {
            SpectrumMediaRefusal::Truncated(format!(
                "header declares {declared} bytes before the CRC, the file has {}",
                bytes.len().saturating_sub(UDI_CRC_BYTES)
            ))
        } else {
            SpectrumMediaRefusal::Corrupt(format!(
                "header declares {declared} bytes before the CRC, the file has {}",
                bytes.len() - UDI_CRC_BYTES
            ))
        });
    }
    let version = bytes[8];
    let cylinders = bytes[9] as usize + 1;
    let sides = bytes[10] as usize + 1;
    let summary = format!("UDI v{version}, {cylinders} cylinders x {sides} sides");
    if version != 0 {
        return Ok(SpectrumMediaInspection::new(
            format,
            SpectrumMediaStatus::UnsupportedVariant,
            MachineEvidence::Absent,
            summary,
        ));
    }
    if sides > 2 {
        return Err(SpectrumMediaRefusal::Corrupt(format!(
            "{sides} sides is not 1 or 2"
        )));
    }
    let extended = le_u32(bytes, 12).expect("header present") as usize;
    let mut offset = UDI_HEADER_BYTES
        .checked_add(extended)
        .filter(|end| *end <= declared)
        .ok_or_else(|| {
            SpectrumMediaRefusal::Corrupt("the extended header runs past the image".into())
        })?;
    for track in 0..cylinders * sides {
        let kind = *bytes
            .get(offset)
            .filter(|_| offset < declared)
            .ok_or_else(|| {
                SpectrumMediaRefusal::Truncated(format!(
                    "image ends before track {} of {}",
                    track + 1,
                    cylinders * sides
                ))
            })?;
        if kind > 0x02 {
            return Ok(SpectrumMediaInspection::new(
                format,
                SpectrumMediaStatus::UnsupportedVariant,
                MachineEvidence::Absent,
                format!(
                    "{summary}; track {} uses record type {kind:#04x}",
                    track + 1
                ),
            ));
        }
        let length = le_u16(bytes, offset + 1)
            .ok_or_else(|| SpectrumMediaRefusal::Truncated("track length cut short".into()))?
            as usize;
        offset = (offset + 3)
            .checked_add(length + length.div_ceil(8))
            .filter(|end| *end <= declared)
            .ok_or_else(|| {
                SpectrumMediaRefusal::Truncated(format!(
                    "track {} declares more data than the image contains",
                    track + 1
                ))
            })?;
    }
    if offset != declared {
        return Err(SpectrumMediaRefusal::Corrupt(format!(
            "{} bytes follow the last track",
            declared - offset
        )));
    }
    let mut result = SpectrumMediaInspection::new(
        format,
        SpectrumMediaStatus::PartiallyUnderstood,
        MachineEvidence::Absent,
        summary,
    );
    result
        .notes
        .push("trailing CRC32 not verified; disk filesystem (TR-DOS/+3DOS) not examined".into());
    Ok(result)
}

// --- MDR --------------------------------------------------------------------

/// The Interface 1 checksum: the byte sum modulo 255 (the IF1 ROM's
/// add-with-end-around-carry loop, which never yields 255).
fn mdr_checksum(bytes: &[u8]) -> u8 {
    (bytes.iter().map(|b| u32::from(*b)).sum::<u32>() % 255) as u8
}

/// Parses a Microdrive image. The 543-byte sector geometry only makes a file
/// a *candidate*; Spectrum evidence comes from the per-sector header,
/// record-descriptor and data checksums the Interface 1 writes.
///
/// * every formatted sector's three checksums agree -> structurally validated;
/// * every header checksum agrees but some record/data checksums do not ->
///   partially understood (damaged or never-written sectors, counted);
/// * the geometry fits but no sector header checksum agrees ->
///   [`SpectrumMediaRefusal::Ambiguous`]: size alone proves nothing;
/// * a header checksum agrees on some sectors but not others, or sector
///   numbers repeat -> [`SpectrumMediaRefusal::Corrupt`].
pub fn parse_mdr(bytes: &[u8]) -> Inspected {
    let format = SpectrumMediaFormat::MicrodriveCartridge;
    let body = bytes.len().saturating_sub(1);
    let sectors = body / MDR_SECTOR_BYTES;
    if bytes.is_empty() || body % MDR_SECTOR_BYTES != 0 || sectors == 0 || sectors > MDR_MAX_SECTORS
    {
        return Err(SpectrumMediaRefusal::NotRecognised(format!(
            "a Microdrive image is 1-{MDR_MAX_SECTORS} sectors of {MDR_SECTOR_BYTES} bytes plus \
             one write-protect byte; this is {} bytes",
            bytes.len()
        )));
    }
    let mut flagged = 0usize;
    let mut header_ok = 0usize;
    let mut record_ok = 0usize;
    let mut data_ok = 0usize;
    let mut in_use = 0usize;
    let mut numbers = [false; MDR_MAX_SECTORS + 1];
    let mut repeated = false;
    for sector in bytes[..body].chunks_exact(MDR_SECTOR_BYTES) {
        if sector[0] & 1 == 0 {
            continue;
        }
        flagged += 1;
        if mdr_checksum(&sector[..14]) != sector[14] {
            continue;
        }
        header_ok += 1;
        let number = sector[1] as usize;
        if !(1..=MDR_MAX_SECTORS).contains(&number) || std::mem::replace(&mut numbers[number], true)
        {
            repeated = true;
        }
        let record_valid = sector[15] & 1 == 0 && mdr_checksum(&sector[15..29]) == sector[29];
        if record_valid {
            record_ok += 1;
            if le_u16(sector, 17).unwrap_or_default() > 0 {
                in_use += 1;
            }
            if mdr_checksum(&sector[30..542]) == sector[542] {
                data_ok += 1;
            }
        }
    }
    if header_ok == 0 {
        return Err(SpectrumMediaRefusal::Ambiguous(format!(
            "{sectors}-sector Microdrive geometry, but none of {flagged} flagged sector headers \
             carries a valid Interface 1 checksum; size alone is not Spectrum evidence"
        )));
    }
    if header_ok != flagged || repeated {
        return Err(SpectrumMediaRefusal::Corrupt(format!(
            "{header_ok} of {flagged} sector-header checksums agree{}",
            if repeated {
                "; sector numbers repeat or fall outside 1-254"
            } else {
                ""
            }
        )));
    }
    let complete = record_ok == header_ok && data_ok == header_ok;
    let write_protected = bytes[body] != 0;
    let mut result = SpectrumMediaInspection::new(
        format,
        if complete {
            SpectrumMediaStatus::StructurallyValidated
        } else {
            SpectrumMediaStatus::PartiallyUnderstood
        },
        MachineEvidence::Absent,
        format!(
            "{sectors} sectors, {header_ok} formatted, {in_use} holding data, {}write-protected",
            if write_protected { "" } else { "not " }
        ),
    );
    result.notes.push(format!(
        "checksums: {header_ok}/{header_ok} sector headers, {record_ok}/{header_ok} record \
         descriptors, {data_ok}/{header_ok} data blocks; needs ZX Interface 1"
    ));
    Ok(result)
}

// --- file entry point -------------------------------------------------------

/// Reads `path` with a per-format cap from metadata and inspects it. Only the
/// 512-byte header of a `.nex` is read.
pub fn inspect_spectrum_media_file(path: &Path) -> Inspected {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let format = SpectrumMediaFormat::for_extension(&extension)
        .ok_or_else(|| SpectrumMediaRefusal::UnsupportedExtension(extension.clone()))?;
    let generation = crate::identity_source::hashing::FileFingerprint::observe(path)
        .ok_or_else(|| SpectrumMediaRefusal::NotReadable("metadata unavailable".into()))?;
    let safe = crate::safe_read::open_bounded_read(path, &crate::safe_read::TrustedRoots::none())
        .map_err(|error| SpectrumMediaRefusal::NotReadable(format!("{error:?}")))?;
    let length = safe.len();
    let maximum = format.byte_limit();
    if length > maximum {
        return Err(SpectrumMediaRefusal::TooLarge {
            bytes: length,
            maximum,
        });
    }
    let read_limit = if format == SpectrumMediaFormat::NextProgram {
        NEX_HEADER_BYTES as u64
    } else {
        maximum
    };
    let file = safe.into_file();
    let before = file
        .metadata()
        .map_err(|error| SpectrumMediaRefusal::NotReadable(error.to_string()))?;
    let mut bytes = Vec::with_capacity(length.min(read_limit) as usize);
    (&file)
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|error| SpectrumMediaRefusal::NotReadable(error.to_string()))?;
    let after = file
        .metadata()
        .map_err(|error| SpectrumMediaRefusal::NotReadable(error.to_string()))?;
    if crate::identity_source::hashing::FileFingerprint::observe(path).as_ref() != Some(&generation)
        || bytes.len() as u64 != length.min(read_limit)
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(SpectrumMediaRefusal::NotReadable(
            "file changed during bounded inspection".into(),
        ));
    }
    match format {
        SpectrumMediaFormat::PzxTape => parse_pzx(&bytes),
        SpectrumMediaFormat::RzxRecording => parse_rzx(&bytes),
        SpectrumMediaFormat::NextProgram => parse_nex(&bytes, length),
        SpectrumMediaFormat::UdiDisk => parse_udi(&bytes),
        SpectrumMediaFormat::MicrodriveCartridge => parse_mdr(&bytes),
    }
}

#[cfg(test)]
mod tests;
