//! Conservative BBC Micro/Acorn standard cassette evidence from PCM WAV.
//!
//! The decoder consumes the shared WAV edge stream; it does not decode audio
//! independently, emulate a loader, or retain PCM samples.  Only the standard
//! 1200-baud CUTS/KCS-style stream is interpreted.

use crate::tape_audio::{PulseEdge, RecoveryConfidence, WavError, analyze_wav};

const BIT_US: u64 = 833;
const MAX_BLOCKS: usize = 256;
const MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BbcChecksum {
    Valid,
    Invalid,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BbcRecoveredBlock {
    pub filename: Option<String>,
    pub load_address: Option<u32>,
    pub execution_address: Option<u32>,
    pub block_number: Option<u16>,
    pub block_length: usize,
    pub final_block: Option<bool>,
    pub checksum: BbcChecksum,
    pub data_checksum: BbcChecksum,
    pub start_sample: u64,
    pub end_sample: u64,
    pub start_micros: u64,
    pub end_micros: u64,
    pub confidence: RecoveryConfidence,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BbcWavRecovery {
    pub audio: crate::tape_audio::TapeAudioAnalysis,
    pub blocks: Vec<BbcRecoveredBlock>,
    pub warnings: Vec<String>,
}

/// Decode standard BBC cassette blocks from the already-conditioned WAV edge
/// stream.  Carrier timing is classified as approximately 1200 Hz (0) or
/// 2400 Hz (1), then framed as 8N1, least-significant bit first.
pub fn decode_bbc_wav(bytes: &[u8]) -> Result<BbcWavRecovery, WavError> {
    let audio = analyze_wav(bytes)?;
    let intervals: Vec<u64> = audio
        .edges
        .windows(2)
        .map(|w| w[1].micros.saturating_sub(w[0].micros))
        .collect();
    let mut symbols = Vec::new();
    let mut elapsed = 0u64;
    let mut fast = 0usize;
    let mut slow = 0usize;
    for &p in &intervals {
        if p == 0 || p > 2_000 {
            elapsed = 0;
            continue;
        }
        let is_fast = p < 310; // half-period: 2400 Hz ~= 208 us
        if is_fast {
            fast += 1;
        } else if p >= 320 {
            slow += 1;
        }
        // Each carrier half-cycle contributes timing to the current bit cell.
        elapsed = elapsed.saturating_add(p);
        if elapsed >= BIT_US {
            symbols.push((fast >= slow, elapsed));
            elapsed = 0;
            fast = 0;
            slow = 0;
        }
    }
    if symbols.len() < 16 {
        return Ok(BbcWavRecovery {
            audio,
            blocks: Vec::new(),
            warnings: vec!["WAV recognised, but no BBC carrier/framed bytes were recovered".into()],
        });
    }
    let mut bytes_out = Vec::new();
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i + 9 < symbols.len() && bytes_out.len() < MAX_BYTES {
        // UART start=0, eight LSB-first data bits, stop=1.  Carrier mapping is
        // intentionally tolerant; the leader provides the low-frequency 0.
        if symbols[i].0 {
            i += 1;
            continue;
        }
        let start = i;
        let mut value = 0u8;
        let mut ok = true;
        for bit in 0..8 {
            let (one, width) = symbols[i + 1 + bit];
            if width < 550 || width > 1_200 {
                ok = false;
            }
            if one {
                value |= 1 << bit;
            }
        }
        if !symbols[i + 9].0 {
            ok = false;
        }
        if ok {
            bytes_out.push(value);
            spans.push((start, i + 10));
            i += 10;
        } else {
            i += 1;
        }
    }
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes_out.len() && blocks.len() < MAX_BLOCKS {
        let Some(rel) = bytes_out[cursor..].iter().position(|b| *b == 0x2a) else {
            break;
        };
        let at = cursor + rel;
        let Some(header_end) = bbc_header_len(&bytes_out[at..]) else {
            cursor = at + 1;
            continue;
        };
        if at + header_end > bytes_out.len() {
            break;
        }
        let h = &bytes_out[at..at + header_end];
        let checksum = crc16(&h[1..header_end - 2]);
        let stored = u16::from_be_bytes([h[header_end - 2], h[header_end - 1]]);
        let valid = checksum == stored;
        let name_end = h[1..11].iter().position(|b| *b == 0).unwrap_or(10);
        let filename = String::from_utf8(h[1..1 + name_end].to_vec())
            .ok()
            .filter(|s| !s.is_empty() && s.chars().all(|c| !c.is_control()));
        let load_address = le_u32(&h[12..16]);
        let execution_address = le_u32(&h[16..20]);
        let block_number = le_u16(&h[20..22]);
        let block_length = le_u16(&h[22..24]).unwrap_or(0) as usize;
        let final_block = h.get(24).map(|v| v & 0x80 != 0);
        let start_symbol = spans.get(at).map(|s| s.0).unwrap_or(0);
        let end_symbol = spans
            .get((at + header_end).saturating_sub(1))
            .map(|s| s.1)
            .unwrap_or(start_symbol);
        let data_start = at + header_end;
        let data_end = data_start.saturating_add(block_length).min(bytes_out.len());
        let data_checksum = if data_end + 2 <= bytes_out.len() && data_end > data_start {
            let expected = u16::from_be_bytes([bytes_out[data_end], bytes_out[data_end + 1]]);
            if crc16(&bytes_out[data_start..data_end]) == expected {
                BbcChecksum::Valid
            } else {
                BbcChecksum::Invalid
            }
        } else {
            BbcChecksum::Unknown
        };
        let edge_at = |n: usize| {
            audio
                .edges
                .get(n.min(audio.edges.len().saturating_sub(1)))
                .copied()
                .unwrap_or(PulseEdge {
                    sample: 0,
                    micros: 0,
                    rising: false,
                })
        };
        let first = edge_at(start_symbol.min(audio.edges.len().saturating_sub(1)));
        let last = edge_at(end_symbol.min(audio.edges.len().saturating_sub(1)));
        blocks.push(BbcRecoveredBlock {
            filename,
            load_address,
            execution_address,
            block_number,
            block_length,
            final_block,
            checksum: if valid {
                BbcChecksum::Valid
            } else {
                BbcChecksum::Invalid
            },
            data_checksum,
            start_sample: first.sample,
            end_sample: last.sample,
            start_micros: first.micros,
            end_micros: last.micros,
            confidence: if valid {
                RecoveryConfidence::High
            } else {
                RecoveryConfidence::Medium
            },
            warnings: if valid {
                Vec::new()
            } else {
                vec!["BBC header CRC invalid".into()]
            },
        });
        cursor = at + header_end;
    }
    let warnings = if blocks.is_empty() {
        vec!["Carrier/framing evidence did not form a valid BBC cassette header".into()]
    } else {
        Vec::new()
    };
    Ok(BbcWavRecovery {
        audio,
        blocks,
        warnings,
    })
}

/// Project recovered standard blocks into the shared descriptive tape model.
/// Payload bytes are intentionally not retained in this projection.
pub fn bbc_wav_tape_analysis(bytes: &[u8]) -> Result<crate::tape_analysis::TapeAnalysis, WavError> {
    let recovery = decode_bbc_wav(bytes)?;
    Ok(crate::tape_analysis::TapeAnalysis {
        format: crate::tape_analysis::TapeFormat::BbcMicroWav,
        platform: Some("BBC Micro"),
        block_count: recovery.blocks.len(),
        entries: recovery
            .blocks
            .iter()
            .map(|b| crate::tape_analysis::TapeEntry {
                name: b.filename.clone(),
                kind: crate::tape_analysis::TapeEntryKind::Data,
                load_address: b.load_address.and_then(|v| u16::try_from(v).ok()),
                length: b.block_length as u64,
                checksum: match b.checksum {
                    BbcChecksum::Valid => crate::tape_analysis::ChecksumState::Valid,
                    BbcChecksum::Invalid => crate::tape_analysis::ChecksumState::Invalid,
                    BbcChecksum::Unknown => crate::tape_analysis::ChecksumState::NotPresent,
                },
            })
            .collect(),
        metadata: recovery
            .blocks
            .iter()
            .flat_map(|b| {
                let mut m = Vec::new();
                if let Some(v) = b.execution_address {
                    m.push(format!("Execution address: {v:08X}"));
                }
                if let Some(v) = b.block_number {
                    m.push(format!("Block number: {v}"));
                }
                if let Some(v) = b.final_block {
                    m.push(if v {
                        "Final block".into()
                    } else {
                        "Continuation block".into()
                    });
                }
                m
            })
            .take(256)
            .collect(),
        loader: None,
        checksum: if recovery
            .blocks
            .iter()
            .any(|b| matches!(b.checksum, BbcChecksum::Invalid))
        {
            crate::tape_analysis::ChecksumState::Invalid
        } else if !recovery.blocks.is_empty() {
            crate::tape_analysis::ChecksumState::Valid
        } else {
            crate::tape_analysis::ChecksumState::NotApplicable
        },
        warnings: recovery.warnings,
        semantic_blocks: vec!["BBC standard cassette blocks".into()],
        logical_segments: recovery.blocks.len(),
        unsupported_blocks: 0,
    })
}

fn bbc_header_len(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 26 || bytes[0] != 0x2a {
        return None;
    }
    let end = bytes[1..11].iter().position(|b| *b == 0).unwrap_or(10);
    let n = 1 + end + 1 + 4 + 4 + 2 + 2 + 1 + 4 + 2;
    (n <= bytes.len()).then_some(n)
}
fn le_u16(b: &[u8]) -> Option<u16> {
    (b.len() >= 2).then(|| u16::from_le_bytes([b[0], b[1]]))
}
fn le_u32(b: &[u8]) -> Option<u32> {
    (b.len() >= 4).then(|| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// BBC cassette CRC as documented by the BBC Micro User Guide (CRC bytes are
/// stored high-byte first).
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &c in bytes {
        crc ^= u16::from(c) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x0810
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_deterministic() {
        assert_eq!(crc16(b"BBC Micro"), crc16(b"BBC Micro"));
        assert_ne!(crc16(b"BBC Micro"), crc16(b"BBC micro"));
    }

    #[test]
    fn header_shape_is_bounded() {
        let mut h = vec![0x2a];
        h.extend_from_slice(b"HELLO\0");
        h.extend_from_slice(&[0; 4 + 4 + 2 + 2 + 1 + 4 + 2]);
        assert_eq!(bbc_header_len(&h), Some(h.len()));
        assert!(bbc_header_len(&[0x2a; 27]).is_none());
    }
}
