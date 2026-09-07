//! Conservative Atari 8-bit standard cassette waveform evidence.
//!
//! The standard Atari recorder format is 600-baud asynchronous FSK: 3995 Hz
//! represents zero and 5327 Hz represents one, with 8N1, least-significant
//! bit first framing.  A record is two `0x55` marker bytes, a control byte,
//! 128 data bytes, and an end-around-carry checksum.  This module recognises
//! only that bounded record format; it does not decode turbo loaders or named
//! commercial formats.

use crate::tape_audio::{PulseEdge, RecoveryConfidence, WavError, analyze_wav};
use sha2::{Digest, Sha256};

const MIN_BAUD: f64 = 540.0;
const MAX_BAUD: f64 = 660.0;
const RECORD_BYTES: usize = 132;
const MAX_RECORDS: usize = 256;
const MAX_DECODED_BYTES: usize = MAX_RECORDS * RECORD_BYTES;
const MAX_HALF_CYCLE_US: u64 = 240;
const GAP_US: u64 = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtariRecordType {
    Full,
    Partial,
    EndOfFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtariChecksum {
    Valid,
    Invalid,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtariRecoveredRecord {
    pub record_type: Option<AtariRecordType>,
    pub control_byte: Option<u8>,
    pub payload: Vec<u8>,
    pub payload_length: usize,
    pub complete: bool,
    pub checksum: AtariChecksum,
    pub sequence: usize,
    pub start_sample: u64,
    pub end_sample: u64,
    pub start_micros: u64,
    pub end_micros: u64,
    pub confidence: RecoveryConfidence,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtariWavRecovery {
    pub audio: crate::tape_audio::TapeAudioAnalysis,
    pub records: Vec<AtariRecoveredRecord>,
    pub custom_stage_candidate: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtariCustomStage {
    pub evidence: crate::tape_audio::CustomStageEvidence,
    pub start_sample: u64,
    pub end_sample: u64,
    pub timing_scale_millionths: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtariCustomBlock {
    pub bytes: Vec<u8>,
    pub start_sample: u64,
    pub end_sample: u64,
    pub start_micros: u64,
    pub end_micros: u64,
    pub decoded_bits: usize,
    pub ambiguous_bits: usize,
    pub mode: crate::tape_audio::CustomSymbolMode,
    pub bit_order: crate::tape_audio::CustomBitOrder,
    pub confidence: RecoveryConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtariCustomWavRecovery {
    pub standard: AtariWavRecovery,
    pub stages: Vec<AtariCustomStage>,
    pub blocks: Vec<AtariCustomBlock>,
    pub loader_class: &'static str,
    pub fingerprint: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct Symbol {
    one: bool,
    start_sample: u64,
    end_sample: u64,
}

#[derive(Debug, Clone)]
struct DecodedByte {
    value: u8,
    start_sample: u64,
    end_sample: u64,
}

/// Decode standard Atari 8-bit cassette records from a PCM WAV.
pub fn decode_atari_wav(bytes: &[u8]) -> Result<AtariWavRecovery, WavError> {
    let audio = analyze_wav(bytes)?;
    let mut best = None;
    let mut baud = MIN_BAUD;
    while baud <= MAX_BAUD + 0.1 {
        for start_edge in 0..audio.edges.len().min(64) {
            let symbols = recover_symbols_from(&audio.edges, baud, start_edge);
            let decoded = decode_uart_bytes(&symbols);
            let parsed = parse_records(&decoded);
            let score = parsed
                .records
                .iter()
                .map(|r| {
                    if r.complete {
                        if r.checksum == AtariChecksum::Valid {
                            3
                        } else {
                            2
                        }
                    } else {
                        1
                    }
                })
                .sum::<usize>();
            if !parsed.records.is_empty()
                && best.as_ref().map(|b: &ParsedRecords| b.score).unwrap_or(0) < score
            {
                best = Some(ParsedRecords { score, ..parsed });
            }
        }
        baud += 5.0;
    }

    let Some(parsed) = best else {
        return Ok(AtariWavRecovery {
            audio,
            records: Vec::new(),
            custom_stage_candidate: false,
            warnings: vec![
                "WAV recognised, but no Atari standard record framing was recovered".into(),
            ],
        });
    };
    let mut records = parsed.records;
    for record in &mut records {
        record.start_micros =
            record.start_sample.saturating_mul(1_000_000) / audio.spec.sample_rate as u64;
        record.end_micros =
            record.end_sample.saturating_mul(1_000_000) / audio.spec.sample_rate as u64;
    }
    let custom_stage_candidate = later_nonstandard_stage(
        &audio.edges,
        records
            .last()
            .map(|r| r.end_sample)
            .unwrap_or(parsed.last_end_sample),
    );
    let mut warnings = parsed.warnings;
    if custom_stage_candidate {
        warnings.push(
            "A later non-standard waveform follows the Atari standard anchor; custom recovery is deferred".into(),
        );
    }
    if records.iter().any(|r| r.checksum == AtariChecksum::Invalid) {
        warnings.push("One or more Atari standard records have an invalid checksum".into());
    }
    Ok(AtariWavRecovery {
        audio,
        records,
        custom_stage_candidate,
        warnings,
    })
}

/// Recover generic non-standard stages only after a complete, checksum-valid
/// Atari standard record. The shared custom decoder is deliberately filtered
/// to the post-anchor region; no weak Atari resemblance can unlock it.
pub fn decode_atari_custom_wav(bytes: &[u8]) -> Result<AtariCustomWavRecovery, WavError> {
    let standard = decode_atari_wav(bytes)?;
    let anchor_end = standard
        .records
        .iter()
        .filter(|record| record.complete && record.checksum == AtariChecksum::Valid)
        .map(|record| record.end_micros)
        .max()
        .unwrap_or(0);
    if anchor_end == 0 {
        return Ok(AtariCustomWavRecovery {
            standard,
            stages: Vec::new(),
            blocks: Vec::new(),
            loader_class: "UnknownCustom",
            fingerprint: atari_custom_fingerprint(&[]),
            warnings: vec![
                "Atari custom recovery requires a complete checksum-valid standard record anchor"
                    .into(),
            ],
        });
    }
    let generic = crate::tape_audio::decode_custom_wav(bytes)?;
    let crate::tape_audio::CustomWavRecovery {
        stages: generic_stages,
        blocks: generic_blocks,
        warnings: generic_warnings,
        ..
    } = generic;
    let rate = standard.audio.spec.sample_rate as u64;
    let stages = generic_stages
        .into_iter()
        .filter(|stage| stage.start_micros >= anchor_end)
        .take(16)
        .map(|evidence| AtariCustomStage {
            start_sample: evidence.start_micros.saturating_mul(rate) / 1_000_000,
            end_sample: evidence.end_micros.saturating_mul(rate) / 1_000_000,
            timing_scale_millionths: evidence
                .pilot_micros
                .saturating_mul(1_000_000)
                .checked_div(1_200u64.max(evidence.pilot_micros))
                .unwrap_or(0)
                .min(u64::from(u32::MAX)) as u32,
            evidence,
        })
        .collect::<Vec<_>>();
    let stages_start = stages
        .iter()
        .map(|stage| stage.evidence.start_micros)
        .collect::<Vec<_>>();
    let blocks = generic_blocks
        .into_iter()
        .filter(|block| block.start_micros >= anchor_end)
        .take(256)
        .map(|block| AtariCustomBlock {
            start_sample: block.start_micros.saturating_mul(rate) / 1_000_000,
            end_sample: block.end_micros.saturating_mul(rate) / 1_000_000,
            bytes: block.bytes,
            start_micros: block.start_micros,
            end_micros: block.end_micros,
            decoded_bits: block.decoded_bits,
            ambiguous_bits: block.ambiguous_bits,
            mode: block.mode,
            bit_order: block.bit_order,
            confidence: block.confidence,
        })
        .collect::<Vec<_>>();
    let loader_class = if stages.len() > 1 {
        "MultiStage"
    } else if blocks
        .iter()
        .any(|block| block.mode == crate::tape_audio::CustomSymbolMode::PairedPulse)
    {
        "GenericTurbo"
    } else if !stages.is_empty() {
        "CustomPulse"
    } else {
        "UnknownCustom"
    };
    let mut warnings = generic_warnings;
    warnings.retain(|warning| !warning.contains("no stable non-ROM custom pilot"));
    if stages.is_empty() {
        warnings.push("No bounded non-standard Atari stage followed the standard anchor".into());
    }
    if blocks.iter().any(|block| block.ambiguous_bits > 0) {
        warnings.push("Atari custom stage retained with ambiguous symbols".into());
    }
    if !stages_start.is_empty() && stages_start.windows(2).any(|w| w[0] >= w[1]) {
        warnings.push("Atari custom stage ordering was ambiguous".into());
    }
    Ok(AtariCustomWavRecovery {
        standard,
        fingerprint: atari_custom_fingerprint(&stages),
        stages,
        blocks,
        loader_class,
        warnings,
    })
}

/// Stable Atari custom-stage fingerprint. It uses normalized timing families,
/// mode/order and relative inter-stage gaps, never sample indices or PCM.
pub fn atari_custom_fingerprint(stages: &[AtariCustomStage]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"atari-custom-loader-v1\0");
    hash.update((stages.len() as u32).to_le_bytes());
    for pair in stages.windows(2) {
        let gap = pair[1]
            .evidence
            .start_micros
            .saturating_sub(pair[0].evidence.end_micros);
        hash.update((gap / 1_000).to_le_bytes());
    }
    for stage in stages {
        hash.update((stage.evidence.pilot_count as u32).to_le_bytes());
        for timing in crate::tape_audio::custom_timing_fingerprint(&stage.evidence) {
            hash.update((timing / 100_000 * 100_000).to_le_bytes());
        }
        hash.update([match stage.evidence.symbol_mode {
            Some(crate::tape_audio::CustomSymbolMode::PairedPulse) => 1,
            Some(crate::tape_audio::CustomSymbolMode::SinglePulse) => 2,
            None => 0,
        }]);
        hash.update([match stage.evidence.bit_order {
            crate::tape_audio::CustomBitOrder::MsbFirst => 1,
            crate::tape_audio::CustomBitOrder::LsbFirst => 2,
            crate::tape_audio::CustomBitOrder::Ambiguous => 0,
        }]);
    }
    hash.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug)]
struct ParsedRecords {
    score: usize,
    records: Vec<AtariRecoveredRecord>,
    warnings: Vec<String>,
    last_end_sample: u64,
}

fn recover_symbols_from(edges: &[PulseEdge], baud: f64, start_edge: usize) -> Vec<Symbol> {
    let bit_us = 1_000_000.0 / baud;
    let mut symbols = Vec::new();
    let mut cell_start = start_edge;
    let mut elapsed = 0u64;
    let mut half_cycles = 0usize;
    for (relative_index, window) in edges[start_edge..].windows(2).enumerate() {
        let interval_index = start_edge + relative_index;
        let interval = window[1].micros.saturating_sub(window[0].micros);
        if interval == 0 || interval > GAP_US {
            elapsed = 0;
            cell_start = interval_index + 1;
            continue;
        }
        if interval > MAX_HALF_CYCLE_US {
            continue;
        }
        half_cycles += 1;
        elapsed = elapsed.saturating_add(interval);
        if elapsed as f64 >= bit_us {
            let start = edges.get(cell_start).map(|e| e.sample).unwrap_or(0);
            let end = edges
                .get(interval_index + 1)
                .map(|e| e.sample)
                .unwrap_or(start);
            symbols.push(Symbol {
                // 5327 Hz produces roughly 17--18 half-cycles per 600-baud
                // cell; 3995 Hz produces roughly 13--14.  Counting complete
                // transitions is more sample-rate-stable than classifying a
                // single quantised interval, especially at 22050 Hz.
                one: half_cycles >= 15,
                start_sample: start,
                end_sample: end,
            });
            elapsed = 0;
            half_cycles = 0;
            cell_start = interval_index + 1;
        }
    }
    symbols
}

fn decode_uart_bytes(symbols: &[Symbol]) -> Vec<DecodedByte> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 9 < symbols.len() && out.len() < MAX_DECODED_BYTES {
        if symbols[i].one || !symbols[i + 9].one {
            i += 1;
            continue;
        }
        let mut value = 0u8;
        for bit in 0..8 {
            if symbols[i + 1 + bit].one {
                value |= 1 << bit;
            }
        }
        out.push(DecodedByte {
            value,
            start_sample: symbols[i].start_sample,
            end_sample: symbols[i + 9].end_sample,
        });
        i += 10;
    }
    out
}

fn parse_records(decoded: &[DecodedByte]) -> ParsedRecords {
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    let mut cursor = 0usize;
    while cursor + 2 < decoded.len() && records.len() < MAX_RECORDS {
        let Some(relative) = decoded[cursor..]
            .windows(3)
            .position(|w| w[0].value == 0x55 && w[1].value == 0x55 && valid_control(w[2].value))
        else {
            break;
        };
        let at = cursor + relative;
        let available = decoded.len() - at;
        if available < RECORD_BYTES {
            let control = decoded[at + 2].value;
            let payload_end = (at + available).min(decoded.len());
            let payload = if payload_end > at + 3 {
                decoded[at + 3..payload_end]
                    .iter()
                    .map(|b| b.value)
                    .collect()
            } else {
                Vec::new()
            };
            records.push(AtariRecoveredRecord {
                record_type: Some(record_type(control)),
                control_byte: Some(control),
                payload_length: payload.len(),
                payload,
                complete: false,
                checksum: AtariChecksum::Unknown,
                sequence: records.len(),
                start_sample: decoded[at].start_sample,
                end_sample: decoded.last().map(|b| b.end_sample).unwrap_or(0),
                start_micros: 0,
                end_micros: 0,
                confidence: RecoveryConfidence::Low,
                warnings: vec!["Atari record is truncated before its checksum".into()],
            });
            warnings.push("Atari standard record recovery ended at a truncated record".into());
            break;
        }
        let frame = &decoded[at..at + RECORD_BYTES];
        let control = frame[2].value;
        let data: Vec<u8> = frame[3..131].iter().map(|b| b.value).collect();
        let expected = end_around_checksum(frame[..131].iter().map(|b| b.value));
        let checksum = if expected == frame[131].value {
            AtariChecksum::Valid
        } else {
            AtariChecksum::Invalid
        };
        let (payload, mut record_warnings) = match record_type(control) {
            AtariRecordType::Full => (data, Vec::new()),
            AtariRecordType::Partial => {
                let length = data[127] as usize;
                if !(1..=127).contains(&length) {
                    (
                        Vec::new(),
                        vec!["Atari partial record length is invalid".into()],
                    )
                } else {
                    (data[..length].to_vec(), Vec::new())
                }
            }
            AtariRecordType::EndOfFile => {
                let mut warnings = Vec::new();
                if data.iter().any(|b| *b != 0) {
                    warnings.push("Atari EOF record data is not all zero".into());
                }
                (Vec::new(), warnings)
            }
        };
        if checksum == AtariChecksum::Invalid {
            record_warnings.push("Atari end-around-carry checksum is invalid".into());
        }
        records.push(AtariRecoveredRecord {
            record_type: Some(record_type(control)),
            control_byte: Some(control),
            payload_length: payload.len(),
            payload,
            complete: true,
            checksum,
            sequence: records.len(),
            start_sample: decoded[at].start_sample,
            end_sample: decoded[at + RECORD_BYTES - 1].end_sample,
            start_micros: 0,
            end_micros: 0,
            confidence: if checksum == AtariChecksum::Valid {
                RecoveryConfidence::High
            } else {
                RecoveryConfidence::Medium
            },
            warnings: record_warnings,
        });
        cursor = at + RECORD_BYTES;
    }
    let last_end_sample = records.last().map(|r| r.end_sample).unwrap_or(0);
    ParsedRecords {
        score: records.iter().map(|r| if r.complete { 2 } else { 1 }).sum(),
        records,
        warnings,
        last_end_sample,
    }
}

fn valid_control(control: u8) -> bool {
    matches!(control, 0xfa | 0xfc | 0xfe)
}

fn record_type(control: u8) -> AtariRecordType {
    match control {
        0xfa => AtariRecordType::Partial,
        0xfe => AtariRecordType::EndOfFile,
        _ => AtariRecordType::Full,
    }
}

fn end_around_checksum(bytes: impl IntoIterator<Item = u8>) -> u8 {
    let mut sum = 0u8;
    for byte in bytes {
        let (next, carry) = sum.overflowing_add(byte);
        sum = next;
        if carry {
            sum = sum.wrapping_add(1);
        }
    }
    sum
}

fn later_nonstandard_stage(edges: &[PulseEdge], anchor_end_sample: u64) -> bool {
    let later: Vec<u64> = edges
        .windows(2)
        .filter_map(|w| {
            (w[0].sample >= anchor_end_sample).then_some(w[1].micros.saturating_sub(w[0].micros))
        })
        .filter(|p| *p > 0 && *p <= 2_000)
        .collect();
    if later.len() < 16 {
        return false;
    }
    let standardish = later.iter().filter(|p| (70..=160).contains(*p)).count();
    standardish * 2 < later.len()
}

/// Project recovered standard records into the shared descriptive tape model.
/// Record payloads are deliberately not retained by this projection.
pub fn atari_wav_tape_analysis(
    bytes: &[u8],
) -> Result<crate::tape_analysis::TapeAnalysis, WavError> {
    let recovery = decode_atari_wav(bytes)?;
    let entries = recovery
        .records
        .iter()
        .filter(|r| r.record_type != Some(AtariRecordType::EndOfFile))
        .map(|r| crate::tape_analysis::TapeEntry {
            name: None,
            kind: crate::tape_analysis::TapeEntryKind::Data,
            load_address: None,
            length: r.payload_length as u64,
            checksum: match r.checksum {
                AtariChecksum::Valid => crate::tape_analysis::ChecksumState::Valid,
                AtariChecksum::Invalid => crate::tape_analysis::ChecksumState::Invalid,
                AtariChecksum::Unknown => crate::tape_analysis::ChecksumState::NotPresent,
            },
        })
        .collect::<Vec<_>>();
    let checksum = if recovery
        .records
        .iter()
        .any(|r| r.checksum == AtariChecksum::Invalid)
    {
        crate::tape_analysis::ChecksumState::Invalid
    } else if recovery
        .records
        .iter()
        .any(|r| r.checksum == AtariChecksum::Valid)
    {
        crate::tape_analysis::ChecksumState::Valid
    } else {
        crate::tape_analysis::ChecksumState::NotPresent
    };
    Ok(crate::tape_analysis::TapeAnalysis {
        format: crate::tape_analysis::TapeFormat::Atari8BitWav,
        platform: Some("Atari 8-bit"),
        block_count: recovery.records.len(),
        entries,
        metadata: vec!["Atari standard 600-baud cassette waveform".into()],
        loader: None,
        checksum,
        warnings: recovery.warnings,
        semantic_blocks: recovery
            .records
            .iter()
            .map(|r| {
                format!(
                    "Atari {:?} record, {} payload bytes",
                    r.record_type, r.payload_length
                )
            })
            .collect(),
        logical_segments: recovery.records.len().max(1),
        unsupported_blocks: usize::from(recovery.custom_stage_candidate),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, samples: &[u8]) -> Vec<u8> {
        let mut out = Vec::from(&b"RIFF"[..]);
        out.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        out.extend_from_slice(samples);
        out
    }

    fn record(control: u8, payload: &[u8], corrupt: bool) -> Vec<u8> {
        let mut frame = vec![0x55, 0x55, control];
        let mut data = [0u8; 128];
        match control {
            0xfa => {
                data[..payload.len()].copy_from_slice(payload);
                data[127] = payload.len() as u8;
            }
            0xfc => {
                data[..payload.len().min(128)].copy_from_slice(&payload[..payload.len().min(128)])
            }
            0xfe => {}
            _ => unreachable!(),
        }
        frame.extend_from_slice(&data);
        let mut checksum = end_around_checksum(frame.iter().copied());
        if corrupt {
            checksum ^= 0x01;
        }
        frame.push(checksum);
        frame
    }

    fn make_wave(rate: u32, records: &[Vec<u8>], baud_scale: f64) -> Vec<u8> {
        let mut samples = vec![128u8; (rate / 20) as usize];
        let mut phase = false;
        let add_tone = |samples: &mut Vec<u8>, freq: f64, duration: f64, phase: &mut bool| {
            let count = (duration * rate as f64).round() as usize;
            let period = rate as f64 / (freq * 2.0);
            for i in 0..count.max(1) {
                let high = ((i as f64 / period) as u64 & 1) == 0;
                samples.push(if high ^ *phase { 235 } else { 20 });
            }
            *phase = !*phase;
        };
        let add_bit = |samples: &mut Vec<u8>, one: bool, phase: &mut bool| {
            let freq = if one { 5327.0 } else { 3995.0 };
            add_tone(samples, freq, 1.0 / (600.0 * baud_scale), phase);
        };
        for frame in records {
            for byte in frame {
                add_bit(&mut samples, false, &mut phase);
                for bit in 0..8 {
                    add_bit(&mut samples, (byte >> bit) & 1 != 0, &mut phase);
                }
                add_bit(&mut samples, true, &mut phase);
            }
            samples.extend(std::iter::repeat_n(128, (rate / 100) as usize));
        }
        wav(rate, &samples)
    }

    fn append_intervals(rate: u32, mut samples: Vec<u8>, intervals: &[u64]) -> Vec<u8> {
        let mut high = true;
        for micros in intervals {
            let count = (micros.saturating_mul(rate as u64) / 1_000_000).max(1) as usize;
            samples.extend(std::iter::repeat_n(if high { 235 } else { 20 }, count));
            high = !high;
        }
        wav(rate, &samples)
    }

    fn custom_stage_intervals() -> Vec<u64> {
        let mut intervals = vec![1_200; 96];
        intervals.push(300);
        let bytes = [0x01, 0xa5, 0x5a, 0xff];
        for byte in bytes {
            for bit in (0..8).rev() {
                let pulse = if (byte >> bit) & 1 == 0 { 700 } else { 900 };
                intervals.extend([pulse, pulse]);
            }
        }
        intervals.extend([700, 700, 700]);
        intervals
    }

    fn custom_wave(rate: u32, standard: Option<&[u8]>, intervals: &[u64]) -> Vec<u8> {
        let base = if let Some(frame) = standard {
            let records = vec![frame.to_vec()];
            make_wave(rate, &records, 1.0)
        } else {
            make_wave(rate, &[], 1.0)
        };
        append_intervals(rate, base[44..].to_vec(), intervals)
    }

    #[test]
    fn standard_records_recover_across_sample_rates() {
        let frame = record(0xfc, b"ATARI", false);
        for rate in [22_050, 44_100, 48_000, 96_000] {
            let bytes = make_wave(rate, std::slice::from_ref(&frame), 1.0);
            let result = decode_atari_wav(&bytes).unwrap();
            assert_eq!(result.records.len(), 1, "rate {rate}");
            assert_eq!(result.records[0].record_type, Some(AtariRecordType::Full));
            assert_eq!(&result.records[0].payload[..5], b"ATARI");
            assert_eq!(result.records[0].checksum, AtariChecksum::Valid);
        }
    }

    #[test]
    fn partial_and_eof_records_preserve_structure() {
        let partial = record(0xfa, b"SHORT", false);
        let eof = record(0xfe, &[], false);
        let result = decode_atari_wav(&make_wave(44_100, &[partial, eof], 1.0)).unwrap();
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.records[0].payload, b"SHORT");
        assert_eq!(
            result.records[1].record_type,
            Some(AtariRecordType::EndOfFile)
        );
    }

    #[test]
    fn bad_checksum_is_preserved_and_truncation_fails_soft() {
        let bad = record(0xfc, b"BAD", true);
        let mut result =
            decode_atari_wav(&make_wave(48_000, std::slice::from_ref(&bad), 1.0)).unwrap();
        assert_eq!(result.records[0].checksum, AtariChecksum::Invalid);
        let mut truncated = bad;
        truncated.truncate(50);
        result =
            decode_atari_wav(&make_wave(48_000, std::slice::from_ref(&truncated), 1.0)).unwrap();
        assert!(result.records.is_empty() || !result.records[0].complete);
    }

    #[test]
    fn generic_fsk_does_not_pass_atari_gate() {
        let mut bytes = Vec::new();
        for i in 0..140 {
            bytes.push(i as u8);
        }
        let result =
            decode_atari_wav(&make_wave(44_100, std::slice::from_ref(&bytes), 1.0)).unwrap();
        assert!(result.records.is_empty());
    }

    #[test]
    fn excessive_drift_is_refused() {
        let frame = record(0xfc, b"DRIFT", false);
        let result =
            decode_atari_wav(&make_wave(44_100, std::slice::from_ref(&frame), 1.25)).unwrap();
        assert!(result.records.is_empty());
    }

    #[test]
    fn projection_uses_atari_format_without_payloads() {
        let frame = record(0xfc, b"DATA", false);
        let analysis = atari_wav_tape_analysis(&make_wave(44_100, &[frame], 1.0)).unwrap();
        assert_eq!(
            analysis.format,
            crate::tape_analysis::TapeFormat::Atari8BitWav
        );
        assert_eq!(analysis.platform, Some("Atari 8-bit"));
        assert_eq!(analysis.entries[0].length, 128);
    }

    #[test]
    fn strong_atari_anchor_unlocks_generic_turbo_and_preserves_standard_records() {
        let frame = record(0xfc, b"BOOT", false);
        let result = decode_atari_custom_wav(&custom_wave(
            44_100,
            Some(&frame),
            &custom_stage_intervals(),
        ))
        .unwrap();
        assert_eq!(result.standard.records[0].checksum, AtariChecksum::Valid);
        assert_eq!(result.loader_class, "GenericTurbo");
        assert_eq!(result.stages.len(), 1);
        assert_eq!(result.blocks[0].bytes[0], 0x01);
        assert!(result.blocks[0].start_sample > result.standard.records[0].end_sample);
    }

    #[test]
    fn custom_pulse_and_unknown_fallbacks_remain_conservative() {
        let frame = record(0xfc, b"BOOT", false);
        let mut pulse = vec![1_200; 96];
        pulse.push(300);
        pulse.extend(std::iter::repeat_n(450, 32));
        let result = decode_atari_custom_wav(&custom_wave(48_000, Some(&frame), &pulse)).unwrap();
        assert_eq!(result.loader_class, "CustomPulse");
        assert!(result.blocks.is_empty());

        let unanchored =
            decode_atari_custom_wav(&custom_wave(44_100, None, &custom_stage_intervals())).unwrap();
        assert_eq!(unanchored.loader_class, "UnknownCustom");
        assert!(unanchored.stages.is_empty());
    }

    #[test]
    fn checksum_invalid_anchor_does_not_unlock_custom_recovery() {
        let frame = record(0xfc, b"BAD", true);
        let result = decode_atari_custom_wav(&custom_wave(
            44_100,
            Some(&frame),
            &custom_stage_intervals(),
        ))
        .unwrap();
        assert!(result.stages.is_empty());
        assert_eq!(result.loader_class, "UnknownCustom");
    }

    #[test]
    fn custom_fingerprint_is_rate_stable_and_drift_sensitive() {
        let frame = record(0xfc, b"BOOT", false);
        let recoveries = [22_050, 44_100, 48_000, 96_000]
            .into_iter()
            .map(|rate| {
                decode_atari_custom_wav(&custom_wave(rate, Some(&frame), &custom_stage_intervals()))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let a = &recoveries[0];
        for recovery in &recoveries[1..] {
            assert_eq!(a.fingerprint, recovery.fingerprint);
        }

        let drifted = custom_stage_intervals()
            .into_iter()
            .map(|value| {
                if value == 700 {
                    800
                } else if value == 900 {
                    1_100
                } else {
                    value
                }
            })
            .collect::<Vec<_>>();
        let c = decode_atari_custom_wav(&custom_wave(44_100, Some(&frame), &drifted)).unwrap();
        assert_ne!(a.fingerprint, c.fingerprint);
    }

    #[test]
    fn standard_only_has_no_custom_classification() {
        let frame = record(0xfc, b"ONLY", false);
        let result =
            decode_atari_custom_wav(&make_wave(22_050, std::slice::from_ref(&frame), 1.0)).unwrap();
        assert!(result.standard.records.len() == 1);
        assert!(result.stages.is_empty());
        assert_eq!(result.loader_class, "UnknownCustom");
    }
}
