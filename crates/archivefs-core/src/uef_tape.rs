//! Bounded Unified Emulator Format (UEF) container evidence.
//!
//! UEF is a gzip-wrapped chunk container for Acorn cassette streams.  This
//! module inventories the chunks and projects only the well-defined standard
//! data chunks into the shared tape-analysis model; it never synthesizes WAV
//! samples or follows opaque bit/control-flow chunks.

use std::io::Read;

use flate2::read::GzDecoder;

use crate::tape_analysis::{
    ChecksumState, MAX_ANALYSIS_ENTRIES, TapeAnalysis, TapeAnalysisError, TapeEntry, TapeEntryKind,
    TapeFormat,
};

const UEF_HEADER: &[u8] = b"UEF File!\0";
const MAX_UEF_OUTPUT: usize = 16 * 1024 * 1024;
const MAX_UEF_CHUNKS: usize = 65_536;
const MAX_UEF_TEXT: usize = 4096;

pub fn parse_uef(bytes: &[u8]) -> Result<TapeAnalysis, TapeAnalysisError> {
    if bytes.len() > crate::tape_analysis::MAX_ANALYSIS_BYTES {
        return Err(TapeAnalysisError::TooLarge);
    }
    let decoded = if bytes.starts_with(UEF_HEADER) {
        bytes.to_vec()
    } else {
        let decoder = GzDecoder::new(bytes);
        let mut decoded = Vec::new();
        decoder
            .take((MAX_UEF_OUTPUT + 1) as u64)
            .read_to_end(&mut decoded)
            .map_err(|error| TapeAnalysisError::Malformed(format!("UEF gzip: {error}")))?;
        decoded
    };
    if decoded.len() > MAX_UEF_OUTPUT {
        return Err(TapeAnalysisError::TooLarge);
    }
    if decoded.len() < UEF_HEADER.len() + 2 || !decoded.starts_with(UEF_HEADER) {
        return Err(TapeAnalysisError::Malformed("invalid UEF header".into()));
    }

    let version = format!("{}.{}", decoded[11], decoded[10]);
    let mut at = UEF_HEADER.len() + 2;
    let mut entries = Vec::new();
    let mut metadata = vec![format!("UEF version {version}")];
    let mut semantic_blocks = Vec::new();
    let mut warnings = Vec::new();
    let mut unsupported_blocks = 0usize;
    let mut chunk_count = 0usize;
    let mut standard_data_chunks = 0usize;

    while at < decoded.len() {
        if chunk_count >= MAX_UEF_CHUNKS {
            return Err(TapeAnalysisError::Malformed(
                "UEF chunk count exceeds safety limit".into(),
            ));
        }
        if decoded.len() - at < 6 {
            return Err(TapeAnalysisError::Malformed(
                "truncated UEF chunk header".into(),
            ));
        }
        let id = u16::from_le_bytes([decoded[at], decoded[at + 1]]);
        let length = u32::from_le_bytes([
            decoded[at + 2],
            decoded[at + 3],
            decoded[at + 4],
            decoded[at + 5],
        ]) as usize;
        at += 6;
        let end = at
            .checked_add(length)
            .ok_or_else(|| TapeAnalysisError::Malformed("UEF chunk length overflow".into()))?;
        if end > decoded.len() {
            return Err(TapeAnalysisError::Malformed(format!(
                "UEF chunk 0x{id:04x} exceeds container"
            )));
        }
        let body = &decoded[at..end];
        match id {
            0x0000 => {
                let text = bounded_text(body);
                if !text.is_empty() {
                    metadata.push(format!("Originator: {text}"));
                }
            }
            0x0001 => semantic_blocks.push(format!("Carrier tone ({length} bytes)")),
            0x0002 => {
                semantic_blocks.push(format!("Carrier tone with dummy byte ({length} bytes)"))
            }
            0x0003 | 0x0004 => semantic_blocks.push(format!("Gap ({length} bytes)")),
            0x0100 => {
                if body.len() < 2 {
                    warnings.push("UEF standard data chunk is missing its baud field".into());
                    unsupported_blocks += 1;
                } else {
                    let baud = u16::from_le_bytes([body[0], body[1]]);
                    let data = &body[2..];
                    if !data.is_empty() && entries.len() < MAX_ANALYSIS_ENTRIES {
                        entries.push(TapeEntry {
                            name: None,
                            kind: TapeEntryKind::Data,
                            load_address: None,
                            length: data.len() as u64,
                            checksum: ChecksumState::NotPresent,
                        });
                    }
                    standard_data_chunks += 1;
                    semantic_blocks.push(format!("Acorn data ({baud} baud, {} bytes)", data.len()));
                }
            }
            0x0102 | 0x0104 => {
                unsupported_blocks += 1;
                semantic_blocks.push(format!("Unsupported UEF bit/data chunk 0x{id:04x}"));
            }
            _ => {
                unsupported_blocks += 1;
                semantic_blocks.push(format!("Unsupported UEF chunk 0x{id:04x}"));
            }
        }
        chunk_count += 1;
        at = end;
    }

    if standard_data_chunks == 0 {
        warnings.push("UEF contains no supported standard Acorn data chunks".into());
    }
    Ok(TapeAnalysis {
        format: TapeFormat::BbcUef,
        platform: Some("BBC Micro / Acorn Electron"),
        block_count: entries.len(),
        entries,
        metadata,
        loader: None,
        checksum: ChecksumState::NotPresent,
        warnings,
        semantic_blocks,
        logical_segments: chunk_count.max(1),
        unsupported_blocks,
    })
}

fn bounded_text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len())
        .min(MAX_UEF_TEXT);
    String::from_utf8_lossy(&bytes[..end])
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    fn uef(chunks: &[(u16, &[u8])]) -> Vec<u8> {
        let mut plain = b"UEF File!\0".to_vec();
        plain.extend_from_slice(&[0x0a, 0x00]);
        for (id, body) in chunks {
            plain.extend_from_slice(&id.to_le_bytes());
            plain.extend_from_slice(&(body.len() as u32).to_le_bytes());
            plain.extend_from_slice(body);
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&plain).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn parses_bounded_uef_chunks_and_standard_data() {
        let origin = b"test tool\0";
        let data = [0xB0, 0x04, 1, 2, 3];
        let analysis = parse_uef(&uef(&[
            (0x0000, origin),
            (0x0003, &[0, 1]),
            (0x0100, &data),
        ]))
        .unwrap();
        assert_eq!(analysis.format, TapeFormat::BbcUef);
        assert_eq!(analysis.entries[0].length, 3);
        assert!(
            analysis
                .metadata
                .iter()
                .any(|item| item.contains("test tool"))
        );
    }

    #[test]
    fn unknown_and_truncated_chunks_fail_closed() {
        let analysis = parse_uef(&uef(&[(0x7777, &[1, 2, 3])])).unwrap();
        assert_eq!(analysis.unsupported_blocks, 1);
        let mut bad = b"UEF File!\0\x0a\0\x00\x01\x04\0\0\0".to_vec();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&bad).unwrap();
        bad = encoder.finish().unwrap();
        assert!(matches!(
            parse_uef(&bad),
            Err(TapeAnalysisError::Malformed(_))
        ));
    }
}
