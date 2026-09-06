//! Bounded, read-only tape content analysis built on the existing structural
//! tape parsers. This adds descriptive metadata only; it never emulates or
//! extracts tape payloads and never claims a game identity from a filename.

use crate::commodore_tape::{
    COMMODORE_TAP_HEADER_BYTES, T64_READ_BYTES, parse_commodore_tap, parse_t64,
};
use crate::tape_identity::{TzxBlockDetails, ZxTapBlockKind, parse_tzx, parse_zx_tap};

pub const MAX_ANALYSIS_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ANALYSIS_ENTRIES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapeFormat {
    ZxTap,
    Tzx,
    CommodoreTap,
    T64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumState {
    Valid,
    Invalid,
    NotPresent,
    NotApplicable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapeEntryKind {
    Basic,
    Code,
    Data,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeEntry {
    pub name: Option<String>,
    pub kind: TapeEntryKind,
    pub load_address: Option<u16>,
    pub length: u64,
    pub checksum: ChecksumState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeAnalysis {
    pub format: TapeFormat,
    pub platform: Option<&'static str>,
    pub block_count: usize,
    pub entries: Vec<TapeEntry>,
    pub metadata: Vec<String>,
    pub loader: Option<&'static str>,
    pub checksum: ChecksumState,
    pub warnings: Vec<String>,
    pub semantic_blocks: Vec<String>,
    pub logical_segments: usize,
    pub unsupported_blocks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapeAnalysisError {
    NotRecognized,
    Malformed(String),
    TooLarge,
}

pub fn analyze_tape(bytes: &[u8]) -> Result<TapeAnalysis, TapeAnalysisError> {
    if bytes.len() > MAX_ANALYSIS_BYTES {
        return Err(TapeAnalysisError::TooLarge);
    }
    if bytes.starts_with(b"ZXTape!\x1a") {
        let observation =
            parse_tzx(bytes).map_err(|e| TapeAnalysisError::Malformed(e.to_string()))?;
        return Ok(TapeAnalysis {
            format: TapeFormat::Tzx,
            platform: Some("ZX Spectrum"),
            block_count: observation.blocks.len(),
            entries: Vec::new(),
            metadata: observation
                .metadata
                .into_iter()
                .take(MAX_ANALYSIS_ENTRIES)
                .collect(),
            loader: None,
            checksum: ChecksumState::NotApplicable,
            warnings: Vec::new(),
            semantic_blocks: observation
                .blocks
                .iter()
                .map(|b| format_tzx_detail(&b.details))
                .collect(),
            logical_segments: observation.group_depth_max.max(1),
            unsupported_blocks: observation
                .blocks
                .iter()
                .filter(|b| matches!(b.details, TzxBlockDetails::None))
                .count(),
        });
    }
    if bytes.starts_with(b"C64-TAPE-RAW") {
        let obs = parse_commodore_tap(
            &bytes[..bytes.len().min(COMMODORE_TAP_HEADER_BYTES)],
            bytes.len() as u64,
        )
        .map_err(|e| TapeAnalysisError::Malformed(e.to_string()))?;
        return Ok(TapeAnalysis {
            format: TapeFormat::CommodoreTap,
            platform: obs.machine.platform_id(),
            block_count: 1,
            entries: Vec::new(),
            metadata: Vec::new(),
            loader: None,
            checksum: ChecksumState::NotApplicable,
            warnings: vec![
                "Pulse data is present; no program name is encoded in the TAP header.".into(),
            ],
            semantic_blocks: Vec::new(),
            logical_segments: 1,
            unsupported_blocks: 0,
        });
    }
    if bytes.starts_with(b"C64S tape image file") {
        let obs = parse_t64(
            &bytes[..bytes.len().min(T64_READ_BYTES)],
            bytes.len() as u64,
        )
        .map_err(|e| TapeAnalysisError::Malformed(e.to_string()))?;
        let entries = obs
            .entries
            .into_iter()
            .take(MAX_ANALYSIS_ENTRIES)
            .map(|e| TapeEntry {
                name: (!e.name.is_empty()).then_some(e.name),
                kind: TapeEntryKind::Directory,
                load_address: Some(e.load_address),
                length: e.payload_size,
                checksum: ChecksumState::NotPresent,
            })
            .collect::<Vec<_>>();
        let entry_count = entries.len();
        return Ok(TapeAnalysis {
            format: TapeFormat::T64,
            platform: Some("Commodore 64"),
            block_count: entries.len(),
            entries,
            metadata: vec![obs.tape_name],
            loader: None,
            checksum: ChecksumState::NotApplicable,
            warnings: Vec::new(),
            semantic_blocks: Vec::new(),
            logical_segments: entry_count.max(1),
            unsupported_blocks: 0,
        });
    }
    let obs = parse_zx_tap(bytes).map_err(|e| TapeAnalysisError::Malformed(e.to_string()))?;
    let mut entries = Vec::new();
    let mut loader = None;
    for header in obs.metadata.into_iter().take(MAX_ANALYSIS_ENTRIES) {
        let (kind, address) = match header.file_type {
            0 => {
                loader = Some("BASIC -> CODE");
                (TapeEntryKind::Basic, None)
            }
            3 => (TapeEntryKind::Code, Some(header.parameter1)),
            _ => (TapeEntryKind::Data, None),
        };
        entries.push(TapeEntry {
            name: Some(header.name),
            kind,
            load_address: address,
            length: u64::from(header.data_length),
            checksum: ChecksumState::Valid,
        });
    }
    let checksum = if obs.blocks.iter().all(|b| b.checksum_valid) {
        ChecksumState::Valid
    } else {
        ChecksumState::Invalid
    };
    let warnings = if obs.blocks.iter().any(|b| b.kind == ZxTapBlockKind::Other) {
        vec!["Headerless or non-standard TAP block present.".into()]
    } else {
        Vec::new()
    };
    let entry_count = entries.len();
    Ok(TapeAnalysis {
        format: TapeFormat::ZxTap,
        platform: Some("ZX Spectrum"),
        block_count: obs.blocks.len(),
        entries,
        metadata: Vec::new(),
        loader,
        checksum,
        warnings,
        semantic_blocks: Vec::new(),
        logical_segments: entry_count.max(1),
        unsupported_blocks: 0,
    })
}

fn format_tzx_detail(detail: &TzxBlockDetails) -> String {
    match detail {
        TzxBlockDetails::Standard { pause_ms, data_len } => {
            format!("standard data: {data_len} bytes, pause {pause_ms} ms")
        }
        TzxBlockDetails::Turbo {
            pilot,
            sync1,
            sync2,
            zero,
            one,
            pilot_count,
            used_bits,
            pause_ms,
            data_len,
        } => format!(
            "turbo data: {data_len} bytes, pilot {pilot}, sync {sync1}/{sync2}, bits {zero}/{one}, pilot count {pilot_count}, final bits {used_bits}, pause {pause_ms} ms"
        ),
        TzxBlockDetails::PureTone { pulse, count } => {
            format!("pure tone: pulse {pulse}, count {count}")
        }
        TzxBlockDetails::PulseSequence { count, min, max } => {
            format!("pulse sequence: {count} pulses, range {min}..{max}")
        }
        TzxBlockDetails::PureData {
            zero,
            one,
            used_bits,
            pause_ms,
            data_len,
        } => format!(
            "pure data: {data_len} bytes, bits {zero}/{one}, final bits {used_bits}, pause {pause_ms} ms"
        ),
        TzxBlockDetails::Pause { duration_ms } => format!("pause: {duration_ms} ms"),
        TzxBlockDetails::None => "unsupported/opaque block".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn block(payload: &[u8]) -> Vec<u8> {
        let mut out = (payload.len() as u16).to_le_bytes().to_vec();
        out.extend_from_slice(payload);
        out
    }
    #[test]
    fn spectrum_header_becomes_bounded_entry() {
        let mut h = vec![0, 3];
        h.extend_from_slice(b"GAME      ");
        h.extend_from_slice(&[4, 0, 0x00, 0x80, 0, 0]);
        h.push(0); // checksum placeholder (19-byte header payload)
        h[18] = h[..18].iter().fold(0, |a, b| a ^ b);
        let analysis = analyze_tape(&block(&h)).unwrap();
        assert_eq!(analysis.entries[0].name.as_deref(), Some("GAME"));
        assert_eq!(analysis.entries[0].load_address, Some(0x8000));
    }
}
