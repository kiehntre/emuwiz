//! Bounded structural inspection for ZX Spectrum TAP/TZX and Amstrad CDT.
//!
//! This module is an inspector, not a tape interpreter. It walks bytes
//! linearly, validates lengths, and records metadata. It never follows TZX
//! jumps, loops, calls, returns, or selects; expands CSW/RLE or generalized
//! data; decodes timings; extracts files; or claims exact game identity from
//! embedded text.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::content_detector::{ContentDetectionOutcome, ContentDetector, ContentDiagnostic};
use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};

pub const MAX_TAPE_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ZX_TAP_BLOCKS: usize = 4_096;
pub const MAX_ZX_TAP_BLOCK_BYTES: usize = u16::MAX as usize;
pub const MAX_TZX_BLOCKS: usize = 4_096;
pub const MAX_TZX_BLOCK_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TZX_METADATA_RECORDS: usize = 256;
pub const MAX_TZX_METADATA_BYTES: usize = 64 * 1024;
pub const MAX_TZX_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_TZX_GROUP_DEPTH: usize = 64;
const TZX_HEADER_BYTES: usize = 10;
const TZX_SIGNATURE: &[u8; 8] = b"ZXTape!\x1a";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapePlatformContext {
    Unknown,
    ZxSpectrum,
    AmstradCpc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZxTapBlockKind {
    Header,
    Data,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZxTapHeader {
    pub file_type: u8,
    pub name: String,
    pub data_length: u16,
    pub parameter1: u16,
    pub parameter2: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZxTapBlock {
    pub length: u16,
    pub kind: ZxTapBlockKind,
    pub checksum_valid: bool,
    pub header: Option<ZxTapHeader>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZxTapObservation {
    pub blocks: Vec<ZxTapBlock>,
    pub metadata: Vec<ZxTapHeader>,
    pub total_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TzxBlockKind {
    StandardData,
    TurboData,
    PureTone,
    PulseSequence,
    PureData,
    DirectRecording,
    C64Data,
    C64Turbo,
    CswRecording,
    GeneralizedData,
    Pause,
    GroupStart,
    GroupEnd,
    Jump,
    LoopStart,
    LoopEnd,
    CallSequence,
    Return,
    Select,
    Stop48K,
    SignalLevel,
    Text,
    Message,
    ArchiveInfo,
    Hardware,
    EmulationInfo,
    CustomInfo,
    Snapshot,
    Instructions,
    KansasCity,
    Glue,
}

impl TzxBlockKind {
    fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0x10 => Self::StandardData,
            0x11 => Self::TurboData,
            0x12 => Self::PureTone,
            0x13 => Self::PulseSequence,
            0x14 => Self::PureData,
            0x15 => Self::DirectRecording,
            0x16 => Self::C64Data,
            0x17 => Self::C64Turbo,
            0x18 => Self::CswRecording,
            0x19 => Self::GeneralizedData,
            0x20 => Self::Pause,
            0x21 => Self::GroupStart,
            0x22 => Self::GroupEnd,
            0x23 => Self::Jump,
            0x24 => Self::LoopStart,
            0x25 => Self::LoopEnd,
            0x26 => Self::CallSequence,
            0x27 => Self::Return,
            0x28 => Self::Select,
            0x2a => Self::Stop48K,
            0x2b => Self::SignalLevel,
            0x30 => Self::Text,
            0x31 => Self::Message,
            0x32 => Self::ArchiveInfo,
            0x33 => Self::Hardware,
            0x34 => Self::EmulationInfo,
            0x35 => Self::CustomInfo,
            0x40 => Self::Snapshot,
            0x49 => Self::Instructions,
            0x4b => Self::KansasCity,
            0x5a => Self::Glue,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TzxBlock {
    pub id: u8,
    pub kind: TzxBlockKind,
    pub length: usize,
    pub metadata_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TzxObservation {
    pub major: u8,
    pub minor: u8,
    pub blocks: Vec<TzxBlock>,
    pub metadata: Vec<String>,
    pub group_depth_max: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapeParseError {
    NotRecognized,
    Truncated(&'static str),
    Malformed(&'static str),
    ResourceLimit(&'static str),
}

impl fmt::Display for TapeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRecognized => f.write_str("not a recognized tape format"),
            Self::Truncated(detail) => write!(f, "truncated tape: {detail}"),
            Self::Malformed(detail) => write!(f, "malformed tape: {detail}"),
            Self::ResourceLimit(detail) => write!(f, "tape inspection limit exceeded: {detail}"),
        }
    }
}
impl std::error::Error for TapeParseError {}

pub fn parse_zx_tap(data: &[u8]) -> Result<ZxTapObservation, TapeParseError> {
    if data.is_empty() {
        return Err(TapeParseError::Truncated("empty TAP"));
    }
    if data.len() > MAX_TAPE_FILE_BYTES {
        return Err(TapeParseError::ResourceLimit("file is larger than 8 MiB"));
    }
    if data.len() >= 12 && &data[..12] == b"C64-TAPE-RAW" {
        return Err(TapeParseError::NotRecognized);
    }
    let mut offset = 0usize;
    let mut blocks = Vec::new();
    let mut metadata = Vec::new();
    while offset < data.len() {
        if blocks.len() >= MAX_ZX_TAP_BLOCKS {
            return Err(TapeParseError::ResourceLimit("too many TAP blocks"));
        }
        if data.len() - offset < 2 {
            return Err(TapeParseError::Truncated("TAP block length"));
        }
        let length = usize::from(u16::from_le_bytes([data[offset], data[offset + 1]]));
        if !(2..=MAX_ZX_TAP_BLOCK_BYTES).contains(&length) {
            return Err(TapeParseError::Malformed("invalid TAP block length"));
        }
        let end = offset
            .checked_add(2)
            .and_then(|value| value.checked_add(length))
            .ok_or(TapeParseError::Malformed("TAP block length overflow"))?;
        if end > data.len() {
            return Err(TapeParseError::Truncated("TAP block payload"));
        }
        let block = &data[offset + 2..end];
        let checksum_valid = block.iter().fold(0u8, |checksum, byte| checksum ^ byte) == 0;
        if !checksum_valid {
            return Err(TapeParseError::Malformed("TAP XOR checksum mismatch"));
        }
        let (kind, header) = if block[0] == 0 && length == 19 {
            let header = ZxTapHeader {
                file_type: block[1],
                name: decode_text(&block[2..12], 10),
                data_length: u16::from_le_bytes([block[12], block[13]]),
                parameter1: u16::from_le_bytes([block[14], block[15]]),
                parameter2: u16::from_le_bytes([block[16], block[17]]),
            };
            if metadata.len() < MAX_TZX_METADATA_RECORDS {
                metadata.push(header.clone());
            }
            (ZxTapBlockKind::Header, Some(header))
        } else if block[0] == 0xff {
            (ZxTapBlockKind::Data, None)
        } else {
            (ZxTapBlockKind::Other, None)
        };
        blocks.push(ZxTapBlock {
            length: length as u16,
            kind,
            checksum_valid,
            header,
        });
        offset = end;
    }
    if blocks.is_empty() {
        return Err(TapeParseError::Truncated("empty TAP"));
    }
    Ok(ZxTapObservation {
        blocks,
        metadata,
        total_bytes: data.len(),
    })
}

pub fn parse_tzx(data: &[u8]) -> Result<TzxObservation, TapeParseError> {
    if data.len() > MAX_TAPE_FILE_BYTES {
        return Err(TapeParseError::ResourceLimit("file is larger than 8 MiB"));
    }
    if data.len() < TZX_HEADER_BYTES {
        return Err(TapeParseError::Truncated("TZX header"));
    }
    if &data[..8] != TZX_SIGNATURE {
        return Err(TapeParseError::NotRecognized);
    }
    let major = data[8];
    let minor = data[9];
    if major != 1 || minor > 0x7f {
        return Err(TapeParseError::Malformed("unsupported TZX version"));
    }
    let mut offset = TZX_HEADER_BYTES;
    let mut blocks = Vec::new();
    let mut metadata = Vec::new();
    let mut metadata_bytes = 0usize;
    let mut group_depth = 0usize;
    let mut group_depth_max = 0usize;
    while offset < data.len() {
        if blocks.len() >= MAX_TZX_BLOCKS {
            return Err(TapeParseError::ResourceLimit("too many TZX blocks"));
        }
        let start = offset;
        let id = *data
            .get(offset)
            .ok_or(TapeParseError::Truncated("TZX block id"))?;
        offset += 1;
        let kind =
            TzxBlockKind::from_id(id).ok_or(TapeParseError::Malformed("unknown TZX block id"))?;
        let metadata_text = match kind {
            TzxBlockKind::StandardData => {
                require(data, offset, 4, "standard block header")?;
                let length = usize::from(le_u16(data, offset + 2));
                offset = checked_end(offset, 4, length, data.len(), "standard data")?;
                None
            }
            TzxBlockKind::TurboData => {
                require(data, offset, 18, "turbo block header")?;
                let length = le_u24(data, offset + 15)?;
                offset = checked_end(offset, 18, length, data.len(), "turbo data")?;
                None
            }
            TzxBlockKind::PureTone => {
                offset = checked_end(offset, 4, 0, data.len(), "pure tone")?;
                None
            }
            TzxBlockKind::PulseSequence => {
                require(data, offset, 1, "pulse sequence count")?;
                let count = usize::from(data[offset]);
                offset = checked_end(
                    offset,
                    1,
                    count
                        .checked_mul(2)
                        .ok_or(TapeParseError::Malformed("pulse count overflow"))?,
                    data.len(),
                    "pulse sequence",
                )?;
                None
            }
            TzxBlockKind::PureData => {
                require(data, offset, 10, "pure data header")?;
                let length = le_u24(data, offset + 7)?;
                offset = checked_end(offset, 10, length, data.len(), "pure data")?;
                None
            }
            TzxBlockKind::DirectRecording => {
                require(data, offset, 8, "direct recording header")?;
                let length = le_u24(data, offset + 5)?;
                offset = checked_end(offset, 8, length, data.len(), "direct recording")?;
                None
            }
            TzxBlockKind::C64Data | TzxBlockKind::C64Turbo => {
                require(data, offset, 6, "C64 block header")?;
                let length = u32::from_le_bytes(data[offset + 2..offset + 6].try_into().unwrap());
                offset = checked_end(
                    offset,
                    6,
                    usize::try_from(length)
                        .map_err(|_| TapeParseError::ResourceLimit("C64 block too large"))?,
                    data.len(),
                    "C64 data",
                )?;
                None
            }
            TzxBlockKind::CswRecording
            | TzxBlockKind::GeneralizedData
            | TzxBlockKind::SignalLevel
            | TzxBlockKind::Stop48K
            | TzxBlockKind::Snapshot
            | TzxBlockKind::Instructions
            | TzxBlockKind::KansasCity => {
                require(data, offset, 4, "length-prefixed TZX block")?;
                let length = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset = checked_end(
                    offset,
                    4,
                    usize::try_from(length)
                        .map_err(|_| TapeParseError::ResourceLimit("TZX block too large"))?,
                    data.len(),
                    "length-prefixed block",
                )?;
                None
            }
            TzxBlockKind::Pause => {
                offset = checked_end(offset, 2, 0, data.len(), "pause")?;
                None
            }
            TzxBlockKind::GroupStart => {
                let text = read_u8_text(data, &mut offset, "group name")?;
                group_depth = group_depth
                    .checked_add(1)
                    .ok_or(TapeParseError::ResourceLimit("group depth overflow"))?;
                group_depth_max = group_depth_max.max(group_depth);
                if group_depth > MAX_TZX_GROUP_DEPTH {
                    return Err(TapeParseError::ResourceLimit("group nesting too deep"));
                }
                text
            }
            TzxBlockKind::GroupEnd => {
                group_depth = group_depth.saturating_sub(1);
                None
            }
            TzxBlockKind::Jump | TzxBlockKind::LoopStart => {
                offset = checked_end(offset, 2, 0, data.len(), "control-flow block")?;
                None
            }
            TzxBlockKind::LoopEnd | TzxBlockKind::Return => None,
            TzxBlockKind::CallSequence => {
                require(data, offset, 2, "call count")?;
                let count = usize::from(le_u16(data, offset));
                offset = checked_end(
                    offset,
                    2,
                    count
                        .checked_mul(2)
                        .ok_or(TapeParseError::Malformed("call count overflow"))?,
                    data.len(),
                    "call sequence",
                )?;
                None
            }
            TzxBlockKind::Select => {
                require(data, offset, 2, "select length")?;
                let length = usize::from(le_u16(data, offset));
                offset = checked_end(offset, 2, length, data.len(), "select block")?;
                None
            }
            TzxBlockKind::Text => read_u8_text(data, &mut offset, "text description")?,
            TzxBlockKind::Message => {
                require(data, offset, 2, "message header")?;
                let length = usize::from(data[offset + 1]);
                offset = checked_end(offset, 2, length, data.len(), "message")?;
                Some(bounded_text(&data[offset - length..offset])?)
            }
            TzxBlockKind::ArchiveInfo => {
                require(data, offset, 2, "archive-info length")?;
                let length = usize::from(le_u16(data, offset));
                offset = checked_end(offset, 2, length, data.len(), "archive info")?;
                None
            }
            TzxBlockKind::Hardware => {
                require(data, offset, 1, "hardware count")?;
                let count = usize::from(data[offset]);
                offset = checked_end(
                    offset,
                    1,
                    count
                        .checked_mul(3)
                        .ok_or(TapeParseError::Malformed("hardware count overflow"))?,
                    data.len(),
                    "hardware",
                )?;
                None
            }
            TzxBlockKind::EmulationInfo => {
                offset = checked_end(offset, 8, 0, data.len(), "emulation info")?;
                None
            }
            TzxBlockKind::CustomInfo => {
                require(data, offset, 20, "custom-info header")?;
                let length = u32::from_le_bytes(data[offset + 16..offset + 20].try_into().unwrap());
                offset = checked_end(
                    offset,
                    20,
                    usize::try_from(length)
                        .map_err(|_| TapeParseError::ResourceLimit("custom info too large"))?,
                    data.len(),
                    "custom info",
                )?;
                None
            }
            TzxBlockKind::Glue => {
                offset = checked_end(offset, 9, 0, data.len(), "glue block")?;
                None
            }
        };
        let length = offset
            .checked_sub(start)
            .ok_or(TapeParseError::Malformed("TZX offset underflow"))?;
        let metadata_index = if let Some(text) = metadata_text {
            metadata_bytes = metadata_bytes
                .checked_add(text.len())
                .ok_or(TapeParseError::ResourceLimit("metadata byte overflow"))?;
            if metadata_bytes > MAX_TZX_METADATA_BYTES {
                return Err(TapeParseError::ResourceLimit(
                    "retained metadata exceeds 64 KiB",
                ));
            }
            if metadata.len() < MAX_TZX_METADATA_RECORDS {
                metadata.push(text);
                Some(metadata.len() - 1)
            } else {
                None
            }
        } else {
            None
        };
        blocks.push(TzxBlock {
            id,
            kind,
            length,
            metadata_index,
        });
    }
    if blocks.is_empty() {
        return Err(TapeParseError::Malformed("TZX has no blocks"));
    }
    Ok(TzxObservation {
        major,
        minor,
        blocks,
        metadata,
        group_depth_max,
    })
}

pub fn parse_cdt(data: &[u8]) -> Result<TzxObservation, TapeParseError> {
    parse_tzx(data)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapeInspection {
    Commodore(crate::commodore_tape::CommodoreTapObservation),
    ZxSpectrum(ZxTapObservation),
}

pub fn inspect_tap(data: &[u8]) -> Result<TapeInspection, TapeParseError> {
    match (
        crate::commodore_tape::parse_commodore_tap(data, data.len() as u64),
        parse_zx_tap(data),
    ) {
        (Ok(_), Ok(_)) => Err(TapeParseError::Malformed(
            ".tap has ambiguous valid interpretations",
        )),
        (Ok(observation), Err(_)) => Ok(TapeInspection::Commodore(observation)),
        (Err(_), Ok(observation)) => Ok(TapeInspection::ZxSpectrum(observation)),
        (Err(_), Err(error)) => Err(error),
    }
}

pub fn read_tape_file(
    path: &Path,
    trusted: &crate::safe_read::TrustedRoots,
) -> Result<Vec<u8>, TapeParseError> {
    let safe = crate::safe_read::open_bounded_read(path, trusted)
        .map_err(|_| TapeParseError::Malformed("safe read refused tape"))?;
    if safe.len() > MAX_TAPE_FILE_BYTES as u64 {
        return Err(TapeParseError::ResourceLimit("file is larger than 8 MiB"));
    }
    let length = usize::try_from(safe.len())
        .map_err(|_| TapeParseError::ResourceLimit("file length does not fit"))?;
    let mut file: File = safe.into_file();
    file.seek(SeekFrom::Start(0))
        .map_err(|_| TapeParseError::Truncated("tape file"))?;
    let mut bytes = vec![0; length];
    file.read_exact(&mut bytes)
        .map_err(|_| TapeParseError::Truncated("tape file"))?;
    Ok(bytes)
}

pub fn inspect_tap_file(
    path: &Path,
    trusted: &crate::safe_read::TrustedRoots,
) -> Result<TapeInspection, TapeParseError> {
    let bytes = read_tape_file(path, trusted)?;
    inspect_tap(&bytes)
}

pub fn inspect_tzx_file(
    path: &Path,
    trusted: &crate::safe_read::TrustedRoots,
) -> Result<TzxObservation, TapeParseError> {
    let bytes = read_tape_file(path, trusted)?;
    parse_tzx(&bytes)
}

pub fn observe_zx_tap_evidence(observation: &ZxTapObservation) -> Vec<ContentEvidence> {
    vec![
        ContentEvidence::new(
            ContentEvidenceKind::MediaClass,
            "Tape",
            ContentEvidenceConfidence::Corroborated,
            format!(
                "valid ZX Spectrum TAP block stream with {} block(s)",
                observation.blocks.len()
            ),
        ),
        ContentEvidence::new(
            ContentEvidenceKind::TapeFormat,
            "ZX Spectrum TAP",
            ContentEvidenceConfidence::Corroborated,
            format!(
                "{} checksum-validated logical tape block(s); embedded names are descriptive only",
                observation.blocks.len()
            ),
        ),
    ]
}

pub fn observe_tzx_evidence(
    observation: &TzxObservation,
    extension: Option<&str>,
    context: TapePlatformContext,
) -> Vec<ContentEvidence> {
    let format = if extension.is_some_and(|value| value.eq_ignore_ascii_case("cdt")) {
        "CDT"
    } else {
        "TZX"
    };
    let mut evidence = vec![
        ContentEvidence::new(
            ContentEvidenceKind::MediaClass,
            "Tape",
            ContentEvidenceConfidence::Strong,
            format!(
                "valid {format}/TZX container with {} structurally framed block(s)",
                observation.blocks.len()
            ),
        ),
        ContentEvidence::new(
            ContentEvidenceKind::TapeFormat,
            format,
            ContentEvidenceConfidence::Strong,
            format!(
                "TZX version {}.{}, linear framing only",
                observation.major, observation.minor
            ),
        ),
    ];
    if matches!(context, TapePlatformContext::ZxSpectrum) {
        evidence.push(ContentEvidence::new(
            ContentEvidenceKind::ContentSignature,
            "ZX Spectrum-compatible tape context",
            ContentEvidenceConfidence::Corroborated,
            "caller supplied ZX Spectrum source context",
        ));
    }
    if matches!(context, TapePlatformContext::AmstradCpc) {
        evidence.push(ContentEvidence::new(
            ContentEvidenceKind::ContentSignature,
            "Amstrad CPC-compatible tape context",
            ContentEvidenceConfidence::Corroborated,
            "caller supplied Amstrad CPC source context",
        ));
    }
    evidence
}

pub struct ZxTapDetector;
impl ContentDetector for ZxTapDetector {
    fn id(&self) -> &'static str {
        "zx_spectrum_tap"
    }
    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        match parse_zx_tap(data) {
            Ok(observation) => ContentDetectionOutcome::Recognized {
                evidence: observe_zx_tap_evidence(&observation),
            },
            Err(TapeParseError::NotRecognized) => ContentDetectionOutcome::NotRecognized,
            Err(error) => ContentDetectionOutcome::Malformed {
                evidence: Vec::new(),
                diagnostic: ContentDiagnostic {
                    detector_id: self.id(),
                    category: "malformed",
                    message: error.to_string(),
                },
            },
        }
    }
}
pub struct TzxDetector;
impl ContentDetector for TzxDetector {
    fn id(&self) -> &'static str {
        "tzx"
    }
    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        match parse_tzx(data) {
            Ok(observation) => ContentDetectionOutcome::Recognized {
                evidence: observe_tzx_evidence(&observation, None, TapePlatformContext::Unknown),
            },
            Err(TapeParseError::NotRecognized) => ContentDetectionOutcome::NotRecognized,
            Err(error) => ContentDetectionOutcome::Malformed {
                evidence: Vec::new(),
                diagnostic: ContentDiagnostic {
                    detector_id: self.id(),
                    category: "malformed",
                    message: error.to_string(),
                },
            },
        }
    }
}

fn require(
    data: &[u8],
    offset: usize,
    length: usize,
    detail: &'static str,
) -> Result<(), TapeParseError> {
    if data
        .len()
        .checked_sub(offset)
        .is_none_or(|remaining| remaining < length)
    {
        Err(TapeParseError::Truncated(detail))
    } else {
        Ok(())
    }
}
fn checked_end(
    offset: usize,
    header: usize,
    payload: usize,
    file_len: usize,
    detail: &'static str,
) -> Result<usize, TapeParseError> {
    let end = offset
        .checked_add(header)
        .and_then(|value| value.checked_add(payload))
        .ok_or(TapeParseError::Malformed("TZX length overflow"))?;
    if payload > MAX_TZX_BLOCK_PAYLOAD_BYTES {
        return Err(TapeParseError::ResourceLimit(detail));
    }
    if end > file_len {
        return Err(TapeParseError::Truncated(detail));
    }
    Ok(end)
}
fn le_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}
fn le_u24(data: &[u8], offset: usize) -> Result<usize, TapeParseError> {
    require(data, offset, 3, "24-bit length")?;
    Ok(usize::from(data[offset])
        | (usize::from(data[offset + 1]) << 8)
        | (usize::from(data[offset + 2]) << 16))
}
fn read_u8_text(
    data: &[u8],
    offset: &mut usize,
    detail: &'static str,
) -> Result<Option<String>, TapeParseError> {
    require(data, *offset, 1, detail)?;
    let length = usize::from(data[*offset]);
    *offset = checked_end(*offset, 1, length, data.len(), detail)?;
    Ok(Some(bounded_text(&data[*offset - length..*offset])?))
}
fn bounded_text(data: &[u8]) -> Result<String, TapeParseError> {
    if data.len() > MAX_TZX_TEXT_BYTES {
        return Err(TapeParseError::ResourceLimit("text metadata exceeds 4 KiB"));
    }
    Ok(decode_text(data, MAX_TZX_TEXT_BYTES))
}
fn decode_text(data: &[u8], max: usize) -> String {
    data.iter()
        .take(max)
        .map(|byte| {
            if (0x20..=0x7e).contains(byte) {
                *byte as char
            } else {
                ' '
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn zx_block(mut block: Vec<u8>) -> Vec<u8> {
        let checksum = block.iter().fold(0u8, |value, byte| value ^ byte);
        block.push(checksum);
        let mut result = (block.len() as u16).to_le_bytes().to_vec();
        result.extend(block);
        result
    }
    fn tzx(blocks: &[u8]) -> Vec<u8> {
        let mut result = b"ZXTape!\x1a\x01\x20".to_vec();
        result.extend(blocks);
        result
    }
    #[test]
    fn empty_random_and_truncated_tap_fail_closed() {
        assert!(parse_zx_tap(&[]).is_err());
        assert!(parse_zx_tap(b"random").is_err());
        assert!(parse_zx_tap(&[1]).is_err());
        assert!(parse_zx_tap(&[3, 0, 0]).is_err());
    }
    #[test]
    fn valid_zx_header_and_data_are_descriptive_only() {
        let mut header = vec![0, 3];
        header.extend_from_slice(b"ALADDIN   ");
        header.extend_from_slice(&[1, 0, 0, 0, 1, 0]);
        let mut tape = zx_block(header);
        tape.extend(zx_block(vec![0xff, 1, 0]));
        let observation = parse_zx_tap(&tape).unwrap();
        assert_eq!(observation.blocks.len(), 2);
        assert_eq!(observation.metadata[0].name, "ALADDIN");
        assert_eq!(
            observe_zx_tap_evidence(&observation)[0].confidence,
            ContentEvidenceConfidence::Corroborated
        );
    }
    #[test]
    fn checksum_mismatch_and_absurd_block_count_refuse() {
        assert!(matches!(
            parse_zx_tap(&[2, 0, 0, 1]),
            Err(TapeParseError::Malformed(_))
        ));
        let mut tape = Vec::new();
        for _ in 0..=MAX_ZX_TAP_BLOCKS {
            tape.extend(zx_block(vec![0xff, 0]));
        }
        assert!(matches!(
            parse_zx_tap(&tape),
            Err(TapeParseError::ResourceLimit(_))
        ));
        assert!(matches!(
            parse_zx_tap(&vec![0; MAX_TAPE_FILE_BYTES + 1]),
            Err(TapeParseError::ResourceLimit(_))
        ));
    }
    #[test]
    fn commodore_tap_is_not_a_zx_tap() {
        let mut data = b"C64-TAPE-RAW".to_vec();
        data.extend_from_slice(&[0, 0, 0, 0, 1, 0, 7]);
        assert_eq!(parse_zx_tap(&data), Err(TapeParseError::NotRecognized));
    }
    #[test]
    fn tap_dispatch_selects_valid_commodore_container() {
        let mut data = b"C64-TAPE-RAW".to_vec();
        data.extend_from_slice(&[0, 0, 0, 0]);
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&[1, 2]);
        assert!(matches!(
            inspect_tap(&data),
            Ok(TapeInspection::Commodore(_))
        ));
    }
    #[test]
    fn valid_tzx_metadata_and_control_flow_are_only_records() {
        let mut blocks = vec![0x30, 5];
        blocks.extend_from_slice(b"hello");
        blocks.extend_from_slice(&[0x23, 0xff, 0xff, 0x24, 1, 0, 0x25, 0x26, 1, 0, 0, 0x27]);
        let observation = parse_tzx(&tzx(&blocks)).unwrap();
        assert_eq!(observation.metadata, vec!["hello"]);
        assert!(
            observation
                .blocks
                .iter()
                .any(|block| block.kind == TzxBlockKind::Jump)
        );
        assert!(
            observation
                .blocks
                .iter()
                .any(|block| block.kind == TzxBlockKind::LoopStart)
        );
    }
    #[test]
    fn csw_and_generalized_payloads_are_framed_not_expanded() {
        let blocks = vec![0x18, 3, 0, 0, 0, 1, 2, 3, 0x19, 2, 0, 0, 0, 9, 8];
        let observation = parse_tzx(&tzx(&blocks)).unwrap();
        assert_eq!(observation.blocks[0].kind, TzxBlockKind::CswRecording);
        assert_eq!(observation.blocks[1].kind, TzxBlockKind::GeneralizedData);
    }
    #[test]
    fn unknown_or_bad_tzx_framing_fails_closed() {
        assert_eq!(
            parse_tzx(b"ZXTape!\x1a\x01\x20\xff"),
            Err(TapeParseError::Malformed("unknown TZX block id"))
        );
        assert!(parse_tzx(b"ZXTape!\x1a\x01\x20\x30\x05hi").is_err());
        assert!(parse_tzx(b"wrong!\x1a\x01\x20").is_err());
        assert!(matches!(
            parse_tzx(b"ZXTape!\x1a\x02\x00"),
            Err(TapeParseError::Malformed("unsupported TZX version"))
        ));
        let too_many_blocks = tzx(&vec![0x22; MAX_TZX_BLOCKS + 1]);
        assert!(matches!(
            parse_tzx(&too_many_blocks),
            Err(TapeParseError::ResourceLimit("too many TZX blocks"))
        ));
        assert!(matches!(
            checked_end(usize::MAX, 1, 1, usize::MAX, "boundary"),
            Err(TapeParseError::Malformed("TZX length overflow"))
        ));
    }
    #[test]
    fn text_metadata_limit_is_enforced_before_retention() {
        assert!(matches!(
            bounded_text(&vec![b'x'; MAX_TZX_TEXT_BYTES + 1]),
            Err(TapeParseError::ResourceLimit("text metadata exceeds 4 KiB"))
        ));
    }
    #[test]
    fn cdt_context_is_conservative_and_not_extension_authority() {
        let observation = parse_tzx(&tzx(&[0x22])).unwrap();
        let unknown = observe_tzx_evidence(&observation, Some("cdt"), TapePlatformContext::Unknown);
        assert!(
            !unknown
                .iter()
                .any(|fact| fact.value.contains("Amstrad CPC"))
        );
        let cpc = observe_tzx_evidence(&observation, Some("cdt"), TapePlatformContext::AmstradCpc);
        assert!(cpc.iter().any(|fact| fact.value.contains("Amstrad CPC")));
        let zx = observe_tzx_evidence(&observation, Some("cdt"), TapePlatformContext::ZxSpectrum);
        assert!(zx.iter().any(|fact| fact.value.contains("ZX Spectrum")));
    }
}
