//! Standard byte-oriented Oric TAP, not ZX TAP or a waveform decoder.
//!
//! Layout reviewed against OSDK header.cpp/tap2dsk.c and Oricutron tape.c;
//! see docs/research/ORIC_MEDIA_IDENTITY_V1.md. There is no stored checksum.

use crate::tape_analysis::{
    ChecksumState, MAX_ANALYSIS_BYTES, MAX_ANALYSIS_ENTRIES, TapeAnalysis, TapeAnalysisError,
    TapeEntry, TapeEntryKind, TapeFormat,
};

pub const MAX_ORIC_FILENAME_BYTES: usize = 16;
pub const MAX_ORIC_LEADER_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OricProgramKind {
    Basic,
    MachineCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OricAutoStart {
    Disabled,
    Basic,
    MachineCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OricTapeBlock {
    pub filename: String,
    pub start_address: u16,
    pub end_address: u16,
    pub program_kind: OricProgramKind,
    /// The declared flag, not proof of a safe or compatible entry point.
    pub auto_start: OricAutoStart,
    pub payload_offset: usize,
    pub payload_length: usize,
    pub leader_length: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OricTapeObservation {
    pub blocks: Vec<OricTapeBlock>,
    pub total_bytes: usize,
}

pub fn has_oric_leader(bytes: &[u8]) -> bool {
    let Some(sync_end) = bytes.iter().take(MAX_ORIC_LEADER_BYTES + 1).position(|byte| *byte != 0x16) else {
        return false;
    };
    sync_end >= 3 && bytes.get(sync_end) == Some(&0x24)
}

/// Validates the complete input, including every subsequent segment and EOF.
/// Nonstandard flags, names, reserved bytes and custom loaders are unsupported.
pub fn parse_oric_tap(bytes: &[u8]) -> Result<OricTapeObservation, TapeAnalysisError> {
    if bytes.len() > MAX_ANALYSIS_BYTES {
        return Err(TapeAnalysisError::TooLarge);
    }
    if bytes.starts_with(&[0x16, 0x16, 0x16])
        && bytes
            .iter()
            .take(MAX_ORIC_LEADER_BYTES + 1)
            .all(|byte| *byte == 0x16)
    {
        return Err(TapeAnalysisError::TooLarge);
    }
    if !has_oric_leader(bytes) {
        return Err(TapeAnalysisError::NotRecognized);
    }
    let bad = |message: &str| TapeAnalysisError::Malformed(message.into());
    let mut at = 0usize;
    let mut blocks = Vec::new();
    while at < bytes.len() {
        if blocks.len() >= MAX_ANALYSIS_ENTRIES {
            return Err(TapeAnalysisError::TooLarge);
        }
        let leader_start = at;
        while bytes.get(at) == Some(&0x16) {
            at += 1;
            if at - leader_start > MAX_ORIC_LEADER_BYTES {
                return Err(TapeAnalysisError::TooLarge);
            }
        }
        let leader_length = at - leader_start;
        if leader_length < 3 || bytes.get(at) != Some(&0x24) {
            return Err(bad(
                "missing Oric leader/sync or unsupported trailing bytes",
            ));
        }
        at += 1;
        let header = bytes
            .get(at..at + 9)
            .ok_or_else(|| bad("truncated Oric header"))?;
        if header[0] != 0 || header[1] != 0 || header[8] != 0 {
            return Err(bad("unsupported Oric reserved header fields"));
        }
        let program_kind = match header[2] {
            0 => OricProgramKind::Basic,
            0x80 => OricProgramKind::MachineCode,
            _ => return Err(bad("unsupported Oric program type")),
        };
        let auto_start = match header[3] {
            0 => OricAutoStart::Disabled,
            0x80 => OricAutoStart::Basic,
            0xc7 => OricAutoStart::MachineCode,
            _ => return Err(bad("unsupported Oric auto-start flag")),
        };
        let end_address = u16::from_be_bytes([header[4], header[5]]);
        let start_address = u16::from_be_bytes([header[6], header[7]]);
        let payload_length = usize::from(
            end_address
                .checked_sub(start_address)
                .ok_or_else(|| bad("Oric end address precedes start address"))?,
        ) + 1;
        at += 9;
        let name = &bytes[at..bytes.len().min(at + MAX_ORIC_FILENAME_BYTES + 1)];
        let length = name
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| bad("Oric filename is truncated or exceeds 16 bytes"))?;
        if !name[..length]
            .iter()
            .all(|b| b.is_ascii_graphic() || *b == b' ')
        {
            return Err(bad("unsupported non-printable Oric filename"));
        }
        let filename =
            String::from_utf8(name[..length].to_vec()).map_err(|_| bad("invalid filename"))?;
        at += length + 1;
        let end = at
            .checked_add(payload_length)
            .ok_or_else(|| bad("Oric length overflow"))?;
        if end > bytes.len() {
            return Err(bad("truncated Oric payload"));
        }
        blocks.push(OricTapeBlock {
            filename,
            start_address,
            end_address,
            program_kind,
            auto_start,
            payload_offset: at,
            payload_length,
            leader_length,
        });
        at = end;
    }
    Ok(OricTapeObservation {
        blocks,
        total_bytes: bytes.len(),
    })
}

impl OricTapeObservation {
    pub fn analysis(&self) -> TapeAnalysis {
        TapeAnalysis {
            format: TapeFormat::OricTap,
            platform: Some("Oric"),
            block_count: self.blocks.len(),
            entries: self.blocks.iter().map(|block| TapeEntry {
                name: (!block.filename.is_empty()).then(|| block.filename.clone()),
                kind: match block.program_kind {
                    OricProgramKind::Basic => TapeEntryKind::Basic,
                    OricProgramKind::MachineCode => TapeEntryKind::Code,
                },
                load_address: Some(block.start_address),
                length: block.payload_length as u64,
                checksum: ChecksumState::NotPresent,
            }).collect(),
            metadata: Vec::new(),
            loader: None,
            checksum: ChecksumState::NotPresent,
            warnings: vec!["TAP framing has no stored checksum; names and flags are metadata, not release or machine compatibility authority.".into()],
            semantic_blocks: self.blocks.iter().map(|b| format!(
                "Oric {:?}, ${:04X}..=${:04X}, auto-start {:?}",
                b.program_kind, b.start_address, b.end_address, b.auto_start,
            )).collect(),
            logical_segments: self.blocks.len(),
            unsupported_blocks: 0,
        }
    }
}
