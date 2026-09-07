//! Bounded Dragon 32/64 and Tandy CoCo ordinary CAS block evidence.
//!
//! The direct-bit CAS representation is shared by the family.  This parser
//! deliberately reports family-level identity and does not guess Dragon vs
//! CoCo or interpret turbo/custom loaders.

use crate::tape_analysis::{
    ChecksumState, MAX_ANALYSIS_ENTRIES, TapeAnalysis, TapeAnalysisError, TapeEntry, TapeEntryKind,
    TapeFormat,
};

const MAX_BLOCKS: usize = 65_536;
const MAX_DATA_BYTES: usize = 255;

pub fn parse_dragon_coco_cas(bytes: &[u8]) -> Result<TapeAnalysis, TapeAnalysisError> {
    if bytes.len() > crate::tape_analysis::MAX_ANALYSIS_BYTES {
        return Err(TapeAnalysisError::TooLarge);
    }
    let Some(mut at) = find_leader(bytes) else {
        return Err(TapeAnalysisError::NotRecognized);
    };
    let mut entries = Vec::new();
    let mut semantic_blocks = Vec::new();
    let mut warnings = Vec::new();
    let mut checksums = Vec::new();
    let mut current_name = None;
    let mut current_kind = TapeEntryKind::Data;
    let mut blocks = 0usize;
    let mut unsupported_blocks = 0usize;

    while at + 2 <= bytes.len() && blocks < MAX_BLOCKS {
        if bytes[at] != 0x55 || bytes[at + 1] != 0x3c {
            warnings.push("CAS block sequence ended before the next leader/sync pair".into());
            break;
        }
        if at + 5 > bytes.len() {
            warnings.push("truncated CAS block header".into());
            break;
        }
        let block_type = bytes[at + 2];
        let length = bytes[at + 3] as usize;
        if length > MAX_DATA_BYTES || at + 5 + length > bytes.len() {
            warnings.push("truncated CAS block payload".into());
            break;
        }
        let data_start = at + 4;
        let data_end = data_start + length;
        let stored = bytes[data_end];
        let expected = bytes[at + 2..data_end]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        let checksum = if stored == expected {
            ChecksumState::Valid
        } else {
            ChecksumState::Invalid
        };
        checksums.push(checksum);
        if data_end + 1 >= bytes.len() || bytes[data_end + 1] != 0x55 {
            warnings.push("CAS block is missing its trailer byte".into());
            break;
        }
        let data = &bytes[data_start..data_end];
        match block_type {
            0x00 if length == 15 => {
                current_name = parse_name(data);
                current_kind = match data.get(8).copied() {
                    Some(0x02) => TapeEntryKind::Code,
                    _ => TapeEntryKind::Data,
                };
                semantic_blocks.push("Namefile block".into());
                entries.push(TapeEntry {
                    name: current_name.clone(),
                    kind: current_kind,
                    load_address: parse_load_address(data),
                    length: length as u64,
                    checksum,
                });
            }
            0x01 => {
                semantic_blocks.push(format!("Data block ({length} bytes)"));
                if entries.len() < MAX_ANALYSIS_ENTRIES {
                    entries.push(TapeEntry {
                        name: current_name.clone(),
                        kind: current_kind,
                        load_address: None,
                        length: length as u64,
                        checksum,
                    });
                }
            }
            0xff if length == 0 => semantic_blocks.push("End of file block".into()),
            _ => {
                unsupported_blocks += 1;
                warnings.push(format!(
                    "unsupported Dragon/CoCo CAS block type 0x{block_type:02x}"
                ));
                semantic_blocks.push(format!("Unsupported block 0x{block_type:02x}"));
            }
        }
        blocks += 1;
        at = data_end + 2;
    }
    if blocks == MAX_BLOCKS {
        warnings.push("CAS block count reached the safety limit".into());
    }
    let checksum = if checksums
        .iter()
        .any(|state| *state == ChecksumState::Invalid)
    {
        ChecksumState::Invalid
    } else if checksums.iter().any(|state| *state == ChecksumState::Valid) {
        ChecksumState::Valid
    } else {
        ChecksumState::NotPresent
    };
    Ok(TapeAnalysis {
        format: TapeFormat::DragonCocoCas,
        platform: Some("Dragon / Tandy CoCo"),
        block_count: blocks,
        entries,
        metadata: vec!["Family-level Dragon/CoCo ordinary cassette evidence".into()],
        loader: None,
        checksum,
        warnings,
        semantic_blocks,
        logical_segments: blocks.max(1),
        unsupported_blocks,
    })
}

fn find_leader(bytes: &[u8]) -> Option<usize> {
    let mut run = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        run = if *byte == 0x55 { run + 1 } else { 0 };
        if run >= 128 && bytes.get(index + 1) == Some(&0x3c) {
            // The parser consumes the final leader byte as the block's
            // framing byte immediately before sync.
            return Some(index);
        }
    }
    None
}

fn parse_name(data: &[u8]) -> Option<String> {
    let end = data[..8].iter().position(|byte| *byte == b' ').unwrap_or(8);
    String::from_utf8(data[..end].to_vec())
        .ok()
        .filter(|name| !name.is_empty() && name.chars().all(|c| !c.is_control()))
}

fn parse_load_address(data: &[u8]) -> Option<u16> {
    (data.len() >= 15).then(|| u16::from_le_bytes([data[13], data[14]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(kind: u8, data: &[u8], corrupt: bool) -> Vec<u8> {
        let mut out = vec![0x55, 0x3c, kind, data.len() as u8];
        out.extend_from_slice(data);
        let mut checksum = out[2..]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        if corrupt {
            checksum = checksum.wrapping_add(1);
        }
        out.push(checksum);
        out.push(0x55);
        out
    }

    #[test]
    fn parses_name_data_eof_and_checksum() {
        let mut bytes = vec![0x55; 128];
        let mut name = [b' '; 15];
        name[..4].copy_from_slice(b"TEST");
        name[8] = 2;
        name[13..15].copy_from_slice(&0x1234u16.to_le_bytes());
        bytes.extend(block(0, &name, false));
        bytes.extend(block(1, &[1, 2, 3], false));
        bytes.extend(block(0xff, &[], false));
        let analysis = parse_dragon_coco_cas(&bytes).unwrap();
        assert_eq!(analysis.platform, Some("Dragon / Tandy CoCo"));
        assert_eq!(analysis.entries[0].name.as_deref(), Some("TEST"));
        assert_eq!(analysis.entries[0].load_address, Some(0x1234));
        assert_eq!(analysis.checksum, ChecksumState::Valid);
    }

    #[test]
    fn checksum_and_truncation_are_preserved() {
        let mut bytes = vec![0x55; 128];
        bytes.extend(block(1, &[1, 2], true));
        let analysis = parse_dragon_coco_cas(&bytes).unwrap();
        assert_eq!(analysis.checksum, ChecksumState::Invalid);
        let mut truncated = vec![0x55; 128];
        truncated.extend_from_slice(&[0x55, 0x3c, 1, 4, 1]);
        let analysis = parse_dragon_coco_cas(&truncated).unwrap();
        assert!(!analysis.warnings.is_empty());
    }
}
