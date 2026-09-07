//! Bounded PCM/WAV tape-audio evidence. This is deliberately a signal layer:
//! it validates PCM, conditions samples, and reports pulse timing. It does not
//! emulate loaders, decode compressed audio, or retain raw PCM in results.

use std::convert::TryInto;

use sha2::{Digest, Sha256};

pub const MAX_WAV_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_CHANNELS: u16 = 8;
pub const MAX_SAMPLE_RATE: u32 = 192_000;
pub const MAX_EDGES: usize = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmFormat {
    U8,
    I16Le,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: PcmFormat,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PulseEdge {
    pub sample: u64,
    pub micros: u64,
    pub rising: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PulseSummary {
    pub edge_count: usize,
    pub median_interval_micros: Option<u64>,
    pub min_interval_micros: Option<u64>,
    pub max_interval_micros: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeAudioAnalysis {
    pub spec: WavSpec,
    pub duration_micros: u64,
    pub dc_offset: i32,
    pub peak: u32,
    pub clipped_fraction_millionths: u32,
    pub edges: Vec<PulseEdge>,
    pub pulse_summary: PulseSummary,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryConfidence {
    High,
    Medium,
    Low,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredTapeBlock {
    pub bytes: Vec<u8>,
    pub start_sample: u64,
    pub end_sample: u64,
    pub start_micros: u64,
    pub end_micros: u64,
    pub timing_scale_millionths: u32,
    pub decoded_bits: usize,
    pub ambiguous_bits: usize,
    pub checksum_valid: Option<bool>,
    pub confidence: RecoveryConfidence,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpectrumWavRecovery {
    pub audio: TapeAudioAnalysis,
    pub blocks: Vec<RecoveredTapeBlock>,
    pub warnings: Vec<String>,
}

/// A timing family discovered in a non-ROM waveform.  Centres and spread are
/// expressed in microseconds so equivalent recordings at different sample
/// rates can be compared without retaining PCM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomTimingCluster {
    pub centre_micros: u64,
    pub count: usize,
    pub spread_micros: u64,
    pub confidence: RecoveryConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomSymbolMode {
    PairedPulse,
    SinglePulse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomBitOrder {
    MsbFirst,
    LsbFirst,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomStageEvidence {
    pub start_micros: u64,
    pub end_micros: u64,
    pub pilot_micros: u64,
    pub pilot_count: usize,
    pub sync_pulses: Vec<u64>,
    pub clusters: Vec<CustomTimingCluster>,
    pub symbol_mode: Option<CustomSymbolMode>,
    pub bit_order: CustomBitOrder,
    pub ambiguous_symbols: usize,
    pub confidence: RecoveryConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRecoveredBlock {
    pub bytes: Vec<u8>,
    pub start_micros: u64,
    pub end_micros: u64,
    pub decoded_bits: usize,
    pub ambiguous_bits: usize,
    pub checksum_valid: Option<bool>,
    pub mode: CustomSymbolMode,
    pub bit_order: CustomBitOrder,
    pub confidence: RecoveryConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomWavRecovery {
    pub audio: TapeAudioAnalysis,
    pub stages: Vec<CustomStageEvidence>,
    pub blocks: Vec<CustomRecoveredBlock>,
    pub loader_class: &'static str,
    pub warnings: Vec<String>,
}

/// One bounded standard Commodore ROM tape stream recovered from PCM edge
/// timing. `payload` excludes the nine sync/countdown bytes and trailing XOR
/// checksum. It is evidence, never a release identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommodoreRecoveredBlock {
    pub payload: Vec<u8>,
    pub start_sample: u64,
    pub end_sample: u64,
    pub start_micros: u64,
    pub end_micros: u64,
    pub checksum_valid: Option<bool>,
    pub parity_errors: usize,
    pub confidence: RecoveryConfidence,
    pub duplicate_of: Option<usize>,
}

/// Bounded C64 Datasette-compatible standard-ROM recovery. Custom/turbo
/// timings are deliberately not interpreted by this V1 decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommodoreWavRecovery {
    pub audio: TapeAudioAnalysis,
    pub blocks: Vec<CommodoreRecoveredBlock>,
    pub warnings: Vec<String>,
}

/// One standard Amstrad CPC cassette record recovered from PCM. CPC records
/// use an MSB-first stream, with a one bit represented by two equal periods
/// approximately twice the duration of a zero bit. The record is accepted
/// only when its standard sync byte and complemented CRC agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmstradCpcRecoveredBlock {
    pub sync_byte: u8,
    pub payload: Vec<u8>,
    pub start_sample: u64,
    pub end_sample: u64,
    pub start_micros: u64,
    pub end_micros: u64,
    pub block_number: Option<u8>,
    pub first_block: Option<bool>,
    pub last_block: Option<bool>,
    pub filename: Option<String>,
    pub file_type: Option<u8>,
    pub load_address: Option<u16>,
    pub length: Option<u16>,
    pub execution_address: Option<u16>,
    pub checksum_valid: Option<bool>,
    pub timing_scale_millionths: u32,
    pub confidence: RecoveryConfidence,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmstradCpcWavRecovery {
    pub audio: TapeAudioAnalysis,
    pub blocks: Vec<AmstradCpcRecoveredBlock>,
    pub warnings: Vec<String>,
    pub custom_stage_candidate: bool,
}

/// Generic custom stages discovered in a recording that already contains a
/// checksum-valid standard C64 stream. This provenance gate is intentional:
/// pulse timings alone cannot safely distinguish C64 fastloaders from another
/// machine's tape protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommodoreCustomWavRecovery {
    pub standard_blocks: usize,
    pub stages: Vec<CustomStageEvidence>,
    pub blocks: Vec<CustomRecoveredBlock>,
    pub loader_class: &'static str,
    pub fingerprint: String,
    pub warnings: Vec<String>,
}

const CPC_MAX_BLOCKS: usize = 64;
const CPC_MAX_SEGMENTS: usize = 8;
const CPC_SEGMENT_BYTES: usize = 256;
const CPC_LEADER_MIN_PULSES: usize = 64;
const CPC_LEADER_CONFIDENT_PULSES: usize = 512;
const CPC_SYNC_HEADER: u8 = 0x2c;
const CPC_SYNC_DATA: u8 = 0x16;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    Malformed(&'static str),
    Unsupported(&'static str),
    TooLarge,
    Truncated,
}

pub fn analyze_wav(bytes: &[u8]) -> Result<TapeAudioAnalysis, WavError> {
    let (spec, samples) = decode_wav(bytes)?;
    let (dc, peak, clipped) = quality(&samples);
    let threshold = ((peak / 8).max(256)) as i32;
    let edges = detect_edges(&samples, threshold, spec.sample_rate);
    let duration_micros =
        (samples.len() as u64).saturating_mul(1_000_000) / spec.sample_rate as u64;
    let pulse_summary = summarize(&edges);
    let mut warnings = Vec::new();
    if edges.is_empty() {
        warnings.push("PCM WAV recognised; no confident tape transitions recovered".into());
    }
    Ok(TapeAudioAnalysis {
        spec,
        duration_micros,
        dc_offset: dc,
        peak,
        clipped_fraction_millionths: clipped,
        edges,
        pulse_summary,
        warnings,
    })
}

/// Attempts conservative ZX Spectrum ROM-loader demodulation from the pulse
/// edges already produced by [`analyze_wav`]. It never invents bytes when a
/// pilot/sync/timing decision is ambiguous.
pub fn decode_spectrum_wav(bytes: &[u8]) -> Result<SpectrumWavRecovery, WavError> {
    let audio = analyze_wav(bytes)?;
    let mut blocks = Vec::new();
    let intervals: Vec<u64> = audio
        .edges
        .windows(2)
        .map(|w| w[1].micros.saturating_sub(w[0].micros))
        .collect();
    let mut i = 0usize;
    while i < intervals.len() {
        let pilot_start = i;
        while i < intervals.len() && close(intervals[i], 2168, 0.16) {
            i += 1;
        }
        if i - pilot_start < 64 {
            i = pilot_start + 1;
            continue;
        }
        let median = median(&intervals[pilot_start..i]);
        let scale = median.saturating_mul(1_000_000) / 2168;
        if i + 2 >= intervals.len()
            || !close_scaled_tolerant(intervals[i], 667, scale, 0.35)
            || !close_scaled_tolerant(intervals[i + 1], 735, scale, 0.35)
        {
            i = pilot_start + 1;
            continue;
        }
        let start_edge = audio.edges[pilot_start].sample;
        i += 2;
        let mut out = Vec::new();
        let mut bits = 0usize;
        let mut ambiguous = 0usize;
        let mut current = 0u8;
        let mut bit_in = 0u8;
        let mut end_edge = start_edge;
        while i + 1 < intervals.len() && out.len() < 4096 {
            if intervals[i] > 4_000 || intervals[i + 1] > 4_000 {
                break;
            }
            let a = intervals[i];
            let b = intervals[i + 1];
            let target0 = 855u64.saturating_mul(scale as u64) / 1_000_000;
            let target1 = 1710u64.saturating_mul(scale as u64) / 1_000_000;
            let value = if close(a, target0, 0.22) && close(b, target0, 0.22) {
                Some(0)
            } else if close(a, target1, 0.22) && close(b, target1, 0.22) {
                Some(1)
            } else {
                None
            };
            let Some(value) = value else {
                ambiguous += 1;
                break;
            };
            current = (current << 1) | value;
            bit_in += 1;
            bits += 1;
            i += 2;
            end_edge = audio.edges[i.min(audio.edges.len() - 1)].sample;
            if bit_in == 8 {
                out.push(current);
                current = 0;
                bit_in = 0;
            }
        }
        let recovered = !out.is_empty();
        if recovered {
            let checksum = (out.len() >= 2).then(|| out.iter().fold(0u8, |x, b| x ^ b) == 0);
            let confidence = if checksum == Some(true) && ambiguous == 0 {
                RecoveryConfidence::High
            } else if bits >= 8 {
                RecoveryConfidence::Medium
            } else {
                RecoveryConfidence::Low
            };
            blocks.push(RecoveredTapeBlock {
                bytes: out,
                start_sample: start_edge,
                end_sample: end_edge,
                start_micros: start_edge.saturating_mul(1_000_000) / audio.spec.sample_rate as u64,
                end_micros: end_edge.saturating_mul(1_000_000) / audio.spec.sample_rate as u64,
                timing_scale_millionths: scale as u32,
                decoded_bits: bits,
                ambiguous_bits: ambiguous,
                checksum_valid: checksum,
                confidence,
                warnings: if ambiguous > 0 {
                    vec!["data timing became ambiguous".into()]
                } else {
                    Vec::new()
                },
            });
        }
        i = if recovered { i } else { pilot_start + 1 };
    }
    let warnings = if blocks.is_empty() {
        vec!["WAV recognised, but no confident Spectrum pilot/sync/data block was recovered".into()]
    } else {
        Vec::new()
    };
    Ok(SpectrumWavRecovery {
        audio,
        blocks,
        warnings,
    })
}

/// Conservatively discovers and decodes generic/turbo pulse trains.  This is
/// intentionally independent of the Spectrum ROM decoder above: a non-ROM
/// pilot is evidence of a custom waveform, never a named commercial loader.
pub fn decode_custom_wav(bytes: &[u8]) -> Result<CustomWavRecovery, WavError> {
    let audio = analyze_wav(bytes)?;
    let intervals: Vec<u64> = audio
        .edges
        .windows(2)
        .map(|w| w[1].micros.saturating_sub(w[0].micros))
        .filter(|value| *value > 0 && *value <= 20_000)
        .take(MAX_CUSTOM_PULSES)
        .collect();
    let mut stages = Vec::new();
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while cursor < intervals.len() && stages.len() < MAX_CUSTOM_STAGES {
        let Some((pilot_start, pilot_end, pilot_cluster)) = find_custom_pilot(&intervals, cursor)
        else {
            break;
        };
        let sync_start = pilot_end;
        let sync_len = (1..=3)
            .find(|length| {
                sync_start + length <= intervals.len()
                    && intervals[sync_start..sync_start + length]
                        .iter()
                        .all(|value| *value < pilot_cluster / 2)
            })
            .unwrap_or(0);
        let data_start = sync_start + sync_len;
        let data_end = (data_start + MAX_CUSTOM_DATA_PULSES).min(intervals.len());
        let data = &intervals[data_start..data_end];
        let clusters = timing_clusters(data);
        let (mode, bit_order, recovered, ambiguous) = infer_custom_data(data, &clusters);
        let start_micros = audio.edges[pilot_start].micros;
        let end_edge = (data_end + 1).min(audio.edges.len().saturating_sub(1));
        let end_micros = audio.edges[end_edge].micros;
        let confidence = if pilot_end - pilot_start >= 96
            && sync_len > 0
            && !clusters.is_empty()
            && ambiguous == 0
        {
            RecoveryConfidence::High
        } else if pilot_end - pilot_start >= 48 && !clusters.is_empty() {
            RecoveryConfidence::Medium
        } else {
            RecoveryConfidence::Low
        };
        let stage = CustomStageEvidence {
            start_micros,
            end_micros,
            pilot_micros: pilot_cluster,
            pilot_count: pilot_end - pilot_start,
            sync_pulses: intervals[sync_start..data_start].to_vec(),
            clusters: clusters.clone(),
            symbol_mode: mode,
            bit_order,
            ambiguous_symbols: ambiguous,
            confidence,
        };
        stages.push(stage);
        if let (Some(mode), Some(bytes)) = (mode, recovered) {
            if !bytes.is_empty() {
                let checksum =
                    (bytes.len() >= 2).then(|| bytes.iter().fold(0u8, |acc, byte| acc ^ byte) == 0);
                blocks.push(CustomRecoveredBlock {
                    bytes,
                    start_micros,
                    end_micros,
                    decoded_bits: data.len()
                        / if mode == CustomSymbolMode::PairedPulse {
                            16
                        } else {
                            8
                        },
                    ambiguous_bits: ambiguous,
                    checksum_valid: checksum,
                    mode,
                    bit_order,
                    confidence,
                });
            }
        }
        cursor = data_end.max(pilot_end + 1);
    }
    let loader_class = if stages.len() > 1 {
        "MultiStage"
    } else if blocks
        .iter()
        .any(|block| block.mode == CustomSymbolMode::PairedPulse)
    {
        "GenericTurbo"
    } else if !stages.is_empty() {
        "CustomPulse"
    } else {
        "UnknownCustom"
    };
    let warnings = if stages.is_empty() {
        vec!["no stable non-ROM custom pilot was recovered".into()]
    } else if blocks.is_empty() {
        vec!["custom timing was found, but no unambiguous byte block was recovered".into()]
    } else {
        Vec::new()
    };
    Ok(CustomWavRecovery {
        audio,
        stages,
        blocks,
        loader_class,
        warnings,
    })
}

const MAX_COMMODORE_BLOCKS: usize = 64;
const COMMODORE_LEADER_PULSES: usize = 64;
const COMMODORE_PAYLOAD_BYTES: usize = 192;

/// Recover standard C64 Datasette streams from the existing bounded PCM edge
/// layer. Timings are calibrated from each short-pulse leader, so sample rate
/// and modest uniform speed drift do not become identity evidence.
pub fn decode_commodore_wav(bytes: &[u8]) -> Result<CommodoreWavRecovery, WavError> {
    let audio = analyze_wav(bytes)?;
    let intervals: Vec<u64> = audio
        .edges
        .windows(2)
        .map(|edges| edges[1].micros.saturating_sub(edges[0].micros))
        .collect();
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while cursor < intervals.len() && blocks.len() < MAX_COMMODORE_BLOCKS {
        let Some((leader_start, leader_end, short)) = find_commodore_leader(&intervals, cursor)
        else {
            break;
        };
        let medium = short.saturating_mul(3) / 2;
        let long = short.saturating_mul(19) / 10;
        let Some(mut position) = find_commodore_marker(&intervals, leader_end, medium, long) else {
            cursor = leader_end;
            continue;
        };
        let start = position;
        let mut raw = Vec::new();
        let mut parity_errors = 0usize;
        let mut complete = false;
        while raw.len() <= COMMODORE_PAYLOAD_BYTES + 10 && position + 2 <= intervals.len() {
            if is_commodore_pair(intervals[position], intervals[position + 1], long, short) {
                complete = true;
                position += 2;
                break;
            }
            let Some((byte, parity_ok, next)) =
                decode_commodore_byte(&intervals, position, short, medium, long)
            else {
                break;
            };
            raw.push(byte);
            parity_errors += usize::from(!parity_ok);
            position = next;
        }
        cursor = position.max(leader_end + 1);
        if raw.len() < 10 || !valid_commodore_countdown(&raw[..9]) {
            continue;
        }
        let contents = &raw[9..];
        if contents.len() < 2 {
            continue;
        }
        let (payload, check) = contents.split_at(contents.len() - 1);
        // Standard program/header records are exactly one 192-byte buffer.
        // Refuse a partial/oversized stream rather than guessing a boundary.
        if payload.len() != COMMODORE_PAYLOAD_BYTES {
            continue;
        }
        // A complete 192-byte stream has an unambiguous payload/check-byte
        // boundary even when the optional L,S end marker is clipped. The
        // marker contributes to framing confidence, not to the XOR itself.
        let checksum_valid = (payload.len() == COMMODORE_PAYLOAD_BYTES)
            .then(|| payload.iter().fold(0u8, |sum, byte| sum ^ byte) == check[0]);
        let duplicate_of = blocks
            .iter()
            .rposition(|previous: &CommodoreRecoveredBlock| previous.payload == payload)
            .filter(|_| blocks.len() > 0);
        let start_edge = audio.edges.get(leader_start).copied();
        let end_edge = audio
            .edges
            .get(position.min(audio.edges.len().saturating_sub(1)))
            .copied();
        let (start_sample, start_micros) = start_edge
            .map(|edge| (edge.sample, edge.micros))
            .unwrap_or((0, 0));
        let (end_sample, end_micros) = end_edge
            .map(|edge| (edge.sample, edge.micros))
            .unwrap_or((0, 0));
        blocks.push(CommodoreRecoveredBlock {
            payload: payload.to_vec(),
            start_sample,
            end_sample,
            start_micros,
            end_micros,
            checksum_valid,
            parity_errors,
            confidence: if checksum_valid == Some(true) && parity_errors == 0 && complete {
                RecoveryConfidence::High
            } else if parity_errors == 0 {
                RecoveryConfidence::Medium
            } else {
                RecoveryConfidence::Low
            },
            duplicate_of,
        });
        let _ = start;
    }
    let warnings = if blocks.is_empty() {
        vec!["no complete C64 standard-ROM tape stream was recovered".into()]
    } else {
        Vec::new()
    };
    Ok(CommodoreWavRecovery {
        audio,
        blocks,
        warnings,
    })
}

/// Recover standard Amstrad CPC cassette records from the existing PCM edge
/// layer. The decoder is deliberately narrower than a generic pulse decoder:
/// it requires the CPC leader/zero marker, a standard sync byte and a valid
/// complemented CRC for each recovered 256-byte segment. Non-standard timing
/// after a valid record is reported as a future custom-stage candidate only.
pub fn decode_amstrad_cpc_wav(bytes: &[u8]) -> Result<AmstradCpcWavRecovery, WavError> {
    let audio = analyze_wav(bytes)?;
    let intervals: Vec<u64> = audio
        .edges
        .windows(2)
        .map(|w| w[1].micros.saturating_sub(w[0].micros))
        .collect();
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    let mut custom_stage_candidate = false;
    while cursor + CPC_LEADER_MIN_PULSES + 20 < intervals.len() && blocks.len() < CPC_MAX_BLOCKS {
        let Some((leader_start, leader_end, one)) = find_cpc_leader(&intervals, cursor) else {
            break;
        };
        let zero = one / 2;
        if leader_end + 2 > intervals.len()
            || !cpc_period(intervals[leader_end], zero)
            || !cpc_period(intervals[leader_end + 1], zero)
        {
            cursor = leader_end.saturating_add(1);
            continue;
        }
        let sync_pos = leader_end + 2;
        let Some((sync, after_sync)) = decode_cpc_byte(&intervals, sync_pos, zero, one) else {
            cursor = leader_end.saturating_add(1);
            continue;
        };
        if sync != CPC_SYNC_HEADER && sync != CPC_SYNC_DATA {
            cursor = leader_end.saturating_add(1);
            continue;
        }
        let mut position = after_sync;
        let mut payload = Vec::new();
        let mut checksum_valid = None;
        let mut complete_segments = 0usize;
        let mut warnings = Vec::new();
        for _ in 0..CPC_MAX_SEGMENTS {
            let segment_start = payload.len();
            let mut segment = Vec::with_capacity(CPC_SEGMENT_BYTES);
            let mut failed = false;
            for _ in 0..CPC_SEGMENT_BYTES {
                if let Some((value, next)) = decode_cpc_byte(&intervals, position, zero, one) {
                    segment.push(value);
                    position = next;
                } else {
                    failed = true;
                    break;
                }
            }
            if failed {
                break;
            }
            let Some((crc_hi, next)) = decode_cpc_byte(&intervals, position, zero, one) else {
                break;
            };
            let Some((crc_lo, next2)) = decode_cpc_byte(&intervals, next, zero, one) else {
                break;
            };
            position = next2;
            let expected = (!cpc_crc16(&segment)).to_be_bytes();
            let valid = [crc_hi, crc_lo] == expected;
            checksum_valid = Some(checksum_valid.unwrap_or(true) && valid);
            if !valid {
                warnings.push("CPC segment CRC mismatch".into());
            }
            payload.extend_from_slice(&segment);
            complete_segments += 1;
            // A standard header is exactly one 256-byte segment. Data records
            // may contain up to eight; a following leader is a clear boundary.
            if sync == CPC_SYNC_HEADER {
                break;
            }
            if position + CPC_LEADER_MIN_PULSES < intervals.len()
                && cpc_period(intervals[position], one)
            {
                let run = cpc_run(&intervals, position, one);
                if run >= CPC_LEADER_MIN_PULSES {
                    break;
                }
            }
            if payload.len() >= CPC_MAX_SEGMENTS * CPC_SEGMENT_BYTES {
                break;
            }
            let _ = segment_start;
        }
        if complete_segments == 0 {
            cursor = leader_end + 1;
            continue;
        }
        let start = audio.edges[leader_start];
        let end_index = position.min(audio.edges.len().saturating_sub(1));
        let end = audio.edges[end_index];
        let header = if sync == CPC_SYNC_HEADER {
            cpc_header(&payload)
        } else {
            None
        };
        let confidence = if checksum_valid == Some(true)
            && leader_end - leader_start >= CPC_LEADER_CONFIDENT_PULSES
        {
            RecoveryConfidence::High
        } else if checksum_valid == Some(true) {
            RecoveryConfidence::Medium
        } else {
            RecoveryConfidence::Low
        };
        blocks.push(AmstradCpcRecoveredBlock {
            sync_byte: sync,
            payload,
            start_sample: start.sample,
            end_sample: end.sample,
            start_micros: start.micros,
            end_micros: end.micros,
            block_number: header.as_ref().map(|h| h.0),
            first_block: header.as_ref().map(|h| h.1),
            last_block: header.as_ref().map(|h| h.2),
            filename: header.as_ref().map(|h| h.3.clone()),
            file_type: header.as_ref().map(|h| h.4),
            load_address: header.as_ref().map(|h| h.5),
            length: header.as_ref().map(|h| h.6),
            execution_address: header.as_ref().map(|h| h.7),
            checksum_valid,
            // CPC's default writer speed is conventionally represented by a
            // 1000-us one period; the measured period is retained as the
            // local calibration rather than treated as machine identity.
            timing_scale_millionths: (one.saturating_mul(1_000_000) / 1000) as u32,
            confidence,
            warnings,
        });
        if position < intervals.len() {
            let run = cpc_run(&intervals, position, one);
            if run >= CPC_LEADER_MIN_PULSES {
                custom_stage_candidate = false;
            } else if intervals[position] > one.saturating_mul(3)
                && position + 1 < intervals.len()
                && intervals[position + 1] > one.saturating_mul(3)
            {
                custom_stage_candidate = true;
            }
        }
        cursor = position.max(leader_end + 1);
    }
    let warnings = if blocks.is_empty() {
        vec!["no standard Amstrad CPC leader/sync/CRC block was recovered".into()]
    } else if custom_stage_candidate {
        vec![
            "non-standard timing follows a valid CPC record; custom/turbo decoding is deferred"
                .into(),
        ]
    } else {
        Vec::new()
    };
    Ok(AmstradCpcWavRecovery {
        audio,
        blocks,
        warnings,
        custom_stage_candidate,
    })
}

/// Project standard CPC records into the common tape model. Header metadata is
/// emitted only for the documented 64-byte system header fields.
pub fn amstrad_cpc_wav_tape_analysis(
    bytes: &[u8],
) -> Result<crate::tape_analysis::TapeAnalysis, WavError> {
    let recovery = decode_amstrad_cpc_wav(bytes)?;
    let entries = recovery
        .blocks
        .iter()
        .filter(|b| b.sync_byte == CPC_SYNC_HEADER)
        .map(|b| crate::tape_analysis::TapeEntry {
            name: b.filename.clone(),
            kind: if b.file_type == Some(0) {
                crate::tape_analysis::TapeEntryKind::Basic
            } else {
                crate::tape_analysis::TapeEntryKind::Code
            },
            load_address: b.load_address,
            length: b.length.map(u64::from).unwrap_or(b.payload.len() as u64),
            checksum: if b.checksum_valid == Some(true) {
                crate::tape_analysis::ChecksumState::Valid
            } else {
                crate::tape_analysis::ChecksumState::Invalid
            },
        })
        .collect::<Vec<_>>();
    let checksum = if recovery
        .blocks
        .iter()
        .any(|b| b.checksum_valid == Some(false))
    {
        crate::tape_analysis::ChecksumState::Invalid
    } else if recovery
        .blocks
        .iter()
        .any(|b| b.checksum_valid == Some(true))
    {
        crate::tape_analysis::ChecksumState::Valid
    } else {
        crate::tape_analysis::ChecksumState::NotPresent
    };
    Ok(crate::tape_analysis::TapeAnalysis {
        format: crate::tape_analysis::TapeFormat::AmstradCpcWav,
        platform: Some("Amstrad CPC"),
        block_count: recovery.blocks.len(),
        entries,
        metadata: vec!["Amstrad CPC standard cassette waveform recovery".into()],
        loader: None,
        checksum,
        warnings: recovery.warnings,
        semantic_blocks: recovery
            .blocks
            .iter()
            .map(|b| {
                format!(
                    "CPC sync 0x{:02x}, block {:?}, CRC {:?}",
                    b.sync_byte, b.block_number, b.checksum_valid
                )
            })
            .collect(),
        logical_segments: recovery.blocks.len().max(1),
        unsupported_blocks: 0,
    })
}

fn find_cpc_leader(intervals: &[u64], from: usize) -> Option<(usize, usize, u64)> {
    let mut start = from;
    while start + CPC_LEADER_MIN_PULSES <= intervals.len() {
        let candidate = intervals[start];
        if !(500..=5000).contains(&candidate) {
            start += 1;
            continue;
        }
        let end = start + cpc_run(intervals, start, candidate);
        if end - start >= CPC_LEADER_MIN_PULSES {
            return Some((start, end, median(&intervals[start..end])));
        }
        start += 1;
    }
    None
}

fn cpc_run(values: &[u64], start: usize, target: u64) -> usize {
    values[start..]
        .iter()
        .take_while(|v| cpc_period(**v, target))
        .count()
}

fn cpc_period(value: u64, target: u64) -> bool {
    target > 0 && close(value, target, 0.25)
}

fn decode_cpc_byte(intervals: &[u64], position: usize, zero: u64, one: u64) -> Option<(u8, usize)> {
    let mut out = 0u8;
    let mut p = position;
    for _ in 0..8 {
        if p + 1 >= intervals.len() {
            return None;
        }
        let a = intervals[p];
        let b = intervals[p + 1];
        let value = if cpc_period(a, zero) && cpc_period(b, zero) {
            0
        } else if cpc_period(a, one) && cpc_period(b, one) {
            1
        } else {
            return None;
        };
        out = (out << 1) | value;
        p += 2;
    }
    Some((out, p))
}

fn cpc_crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn cpc_header(payload: &[u8]) -> Option<(u8, bool, bool, String, u8, u16, u16, u16)> {
    if payload.len() < 28 {
        return None;
    }
    let end = payload[0..16].iter().position(|b| *b == 0).unwrap_or(16);
    let filename = String::from_utf8_lossy(&payload[..end])
        .trim_end()
        .to_string();
    Some((
        payload[16],
        payload[23] != 0,
        payload[17] != 0,
        filename,
        payload[18],
        u16::from_le_bytes([payload[21], payload[22]]),
        u16::from_le_bytes([payload[19], payload[20]]),
        u16::from_le_bytes([payload[26], payload[27]]),
    ))
}

/// Conservatively expose V5's generic custom timing recovery as C64 custom
/// evidence. The independent standard-ROM recovery is a required provenance
/// clue; without it this function returns UnknownCustom rather than guessing a
/// platform or commercial fastloader name.
pub fn decode_commodore_custom_wav(bytes: &[u8]) -> Result<CommodoreCustomWavRecovery, WavError> {
    let standard = decode_commodore_wav(bytes)?;
    let standard_blocks = standard
        .blocks
        .iter()
        .filter(|block| block.checksum_valid == Some(true))
        .count();
    if standard_blocks == 0 {
        return Ok(CommodoreCustomWavRecovery {
            standard_blocks: 0,
            stages: Vec::new(),
            blocks: Vec::new(),
            loader_class: "UnknownCustom",
            fingerprint: custom_loader_fingerprint(&[]),
            warnings: vec![
                "custom timing was not labelled C64 without a valid standard C64 bootstrap".into(),
            ],
        });
    }
    let generic = decode_custom_wav(bytes)?;
    // `decode_custom_wav` intentionally ignores the short C64 ROM leader;
    // remaining stages are independently calibrated and retain their V5
    // timing, symbol-mode, ambiguity, and provenance facts.
    let stages = generic.stages;
    let blocks = generic.blocks;
    let loader_class = if stages.len() > 1 {
        "MultiStage"
    } else if blocks
        .iter()
        .any(|block| block.mode == CustomSymbolMode::PairedPulse)
    {
        "GenericTurbo"
    } else if !stages.is_empty() {
        "CustomPulse"
    } else {
        "UnknownCustom"
    };
    let mut warnings = generic.warnings;
    if stages.is_empty() {
        warnings.push("no non-ROM custom C64 stage was recovered".into());
    }
    Ok(CommodoreCustomWavRecovery {
        standard_blocks,
        fingerprint: custom_loader_fingerprint(&stages),
        stages,
        blocks,
        loader_class,
        warnings,
    })
}

/// Stable timing-only custom loader fingerprint. It intentionally excludes
/// titles, payload bytes, source paths, sample indices, and sample rate.
pub fn custom_loader_fingerprint(stages: &[CustomStageEvidence]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"commodore-custom-loader-v1\0");
    for stage in stages {
        hash.update(stage.pilot_count.to_le_bytes());
        for timing in custom_timing_fingerprint(stage) {
            // Edge timestamps are quantized differently at 44.1/48 kHz.
            // Keep enough resolution to distinguish real timing families but
            // deliberately discard that capture-rate rounding noise.
            hash.update((timing / 50_000 * 50_000).to_le_bytes());
        }
        hash.update([match stage.symbol_mode {
            Some(CustomSymbolMode::PairedPulse) => 1,
            Some(CustomSymbolMode::SinglePulse) => 2,
            None => 0,
        }]);
    }
    hash.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Project only checksum-valid, non-duplicate standard streams into the
/// existing logical TapeAnalysis model. A header creates an entry only after
/// enough following data bytes have been recovered for its declared range.
pub fn commodore_wav_tape_analysis(
    bytes: &[u8],
) -> Result<crate::tape_analysis::TapeAnalysis, WavError> {
    let recovery = decode_commodore_wav(bytes)?;
    let mut entries = Vec::new();
    let mut pending: Option<(u8, String, u16, u16, Vec<u8>)> = None;
    let mut warnings = recovery.warnings.clone();
    for block in &recovery.blocks {
        if block.duplicate_of.is_some() {
            continue;
        }
        if block.checksum_valid != Some(true) {
            warnings.push("Commodore stream retained with invalid or unavailable XOR check".into());
            continue;
        }
        if let Some((file_type, name, start, end, data)) = pending.as_mut() {
            data.extend_from_slice(&block.payload);
            let expected = usize::from(end.saturating_sub(*start));
            if data.len() >= expected {
                entries.push(crate::tape_analysis::TapeEntry {
                    name: (!name.is_empty()).then_some(name.clone()),
                    kind: if *file_type == 1 {
                        crate::tape_analysis::TapeEntryKind::Basic
                    } else {
                        crate::tape_analysis::TapeEntryKind::Code
                    },
                    load_address: Some(*start),
                    length: expected as u64,
                    checksum: crate::tape_analysis::ChecksumState::Valid,
                });
                pending = None;
            }
            continue;
        }
        if let Some((file_type, name, start, end)) = commodore_header(&block.payload) {
            if end < start {
                warnings.push("Commodore header has reversed address range".into());
                continue;
            }
            pending = Some((file_type, name, start, end, Vec::new()));
        }
    }
    let checksum = if recovery
        .blocks
        .iter()
        .any(|block| block.checksum_valid == Some(false))
    {
        crate::tape_analysis::ChecksumState::Invalid
    } else if recovery
        .blocks
        .iter()
        .any(|block| block.checksum_valid == Some(true))
    {
        crate::tape_analysis::ChecksumState::Valid
    } else {
        crate::tape_analysis::ChecksumState::NotPresent
    };
    Ok(crate::tape_analysis::TapeAnalysis {
        format: crate::tape_analysis::TapeFormat::CommodoreWav,
        platform: Some("Commodore 64"),
        block_count: recovery.blocks.len(),
        entries,
        metadata: vec!["C64 standard-ROM Datasette waveform recovery".into()],
        loader: None,
        checksum,
        warnings,
        semantic_blocks: recovery
            .blocks
            .iter()
            .map(|block| {
                format!(
                    "Commodore stream: {} bytes, XOR {:?}, duplicate {:?}",
                    block.payload.len(),
                    block.checksum_valid,
                    block.duplicate_of
                )
            })
            .collect(),
        logical_segments: recovery.blocks.len(),
        unsupported_blocks: 0,
    })
}

fn find_commodore_leader(intervals: &[u64], from: usize) -> Option<(usize, usize, u64)> {
    let mut start = from;
    while start + COMMODORE_LEADER_PULSES <= intervals.len() {
        let candidate = intervals[start];
        if candidate < 80 || candidate > 600 {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < intervals.len() && close(intervals[end], candidate, 0.15) {
            end += 1;
        }
        if end - start >= COMMODORE_LEADER_PULSES {
            return Some((start, end, median(&intervals[start..end])));
        }
        start += 1;
    }
    None
}

fn find_commodore_marker(intervals: &[u64], from: usize, medium: u64, long: u64) -> Option<usize> {
    (from..intervals.len().saturating_sub(19))
        .find(|index| is_commodore_pair(intervals[*index], intervals[*index + 1], long, medium))
}

fn is_commodore_pair(first: u64, second: u64, expected_first: u64, expected_second: u64) -> bool {
    close(first, expected_first, 0.20) && close(second, expected_second, 0.20)
}

fn decode_commodore_byte(
    intervals: &[u64],
    position: usize,
    short: u64,
    medium: u64,
    long: u64,
) -> Option<(u8, bool, usize)> {
    if position + 20 > intervals.len()
        || !is_commodore_pair(intervals[position], intervals[position + 1], long, medium)
    {
        return None;
    }
    let mut value = 0u8;
    let mut parity = 1u8;
    for bit in 0..9 {
        let first = intervals[position + 2 + bit * 2];
        let second = intervals[position + 3 + bit * 2];
        let decoded = if is_commodore_pair(first, second, short, medium) {
            0
        } else if is_commodore_pair(first, second, medium, short) {
            1
        } else {
            return None;
        };
        if bit < 8 {
            value |= decoded << bit;
        }
        parity ^= decoded;
    }
    // `parity` begins at one and includes all nine recorded bits. Odd parity
    // is therefore valid only when the final XOR is zero.
    Some((value, parity == 0, position + 20))
}

fn valid_commodore_countdown(bytes: &[u8]) -> bool {
    bytes == [0x89, 0x88, 0x87, 0x86, 0x85, 0x84, 0x83, 0x82, 0x81]
        || bytes == [0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]
}

fn commodore_header(payload: &[u8]) -> Option<(u8, String, u16, u16)> {
    if payload.len() != COMMODORE_PAYLOAD_BYTES || !matches!(payload[0], 1..=5) {
        return None;
    }
    let name = payload[5..21]
        .iter()
        .copied()
        .take_while(|byte| *byte != b' ')
        .map(|byte| {
            if (0x20..=0x7e).contains(&byte) {
                byte as char
            } else {
                '\u{fffd}'
            }
        })
        .collect();
    Some((
        payload[0],
        name,
        u16::from_le_bytes([payload[1], payload[2]]),
        u16::from_le_bytes([payload[3], payload[4]]),
    ))
}

const MAX_CUSTOM_PULSES: usize = 200_000;
const MAX_CUSTOM_STAGES: usize = 16;
const MAX_CUSTOM_DATA_PULSES: usize = 32_768;

fn find_custom_pilot(intervals: &[u64], from: usize) -> Option<(usize, usize, u64)> {
    let mut i = from;
    while i < intervals.len() {
        let start = i;
        let centre = intervals[i];
        while i < intervals.len() && close(intervals[i], centre, 0.10) {
            i += 1;
        }
        // C64 ROM leaders are much shorter than the non-ROM custom pilot
        // range supported by V5. Skipping them lets a standard C64 bootstrap
        // coexist with a later independently calibrated custom stage.
        if i - start >= 32 && centre > 500 && !close(centre, 2168, 0.18) {
            return Some((start, i, centre));
        }
        i = start + 1;
    }
    None
}

fn timing_clusters(values: &[u64]) -> Vec<CustomTimingCluster> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let mut groups: Vec<Vec<u64>> = Vec::new();
    for value in sorted {
        if let Some(group) = groups.last_mut()
            && close(value, group[group.len() - 1], 0.14)
        {
            group.push(value);
        } else if groups.len() < 8 {
            groups.push(vec![value]);
        }
    }
    groups
        .into_iter()
        .filter(|group| group.len() >= 4)
        .map(|group| {
            let centre = median(&group);
            let spread = group
                .iter()
                .map(|value| value.abs_diff(centre))
                .max()
                .unwrap_or(0);
            let confidence = if group.len() >= 24 && spread <= centre / 8 {
                RecoveryConfidence::High
            } else if group.len() >= 8 {
                RecoveryConfidence::Medium
            } else {
                RecoveryConfidence::Low
            };
            CustomTimingCluster {
                centre_micros: centre,
                count: group.len(),
                spread_micros: spread,
                confidence,
            }
        })
        .collect()
}

fn infer_custom_data(
    data: &[u64],
    clusters: &[CustomTimingCluster],
) -> (
    Option<CustomSymbolMode>,
    CustomBitOrder,
    Option<Vec<u8>>,
    usize,
) {
    if clusters.len() < 2 {
        return (None, CustomBitOrder::Ambiguous, None, data.len());
    }
    let mut centres: Vec<u64> = clusters
        .iter()
        .map(|cluster| cluster.centre_micros)
        .collect();
    centres.sort_unstable();
    let short = centres[0];
    let long = centres[1];
    let pair_fit = data
        .chunks_exact(2)
        .filter(|pair| {
            close(pair[0], pair[1], 0.18)
                && (close(pair[0], short, 0.22) || close(pair[0], long, 0.22))
        })
        .count();
    let single_fit = data
        .iter()
        .filter(|value| close(**value, short, 0.22) || close(**value, long, 0.22))
        .count();
    let mode = if pair_fit >= 4 && pair_fit * 2 >= single_fit {
        CustomSymbolMode::PairedPulse
    } else if single_fit >= 8 {
        CustomSymbolMode::SinglePulse
    } else {
        return (None, CustomBitOrder::Ambiguous, None, data.len());
    };
    let width = if mode == CustomSymbolMode::PairedPulse {
        2
    } else {
        1
    };
    let symbols: Vec<Option<u8>> = data
        .chunks_exact(width)
        .map(|chunk| {
            let value = if mode == CustomSymbolMode::PairedPulse {
                if !close(chunk[0], chunk[1], 0.18) {
                    return None;
                }
                chunk[0]
            } else {
                chunk[0]
            };
            if close(value, short, 0.22) {
                Some(0)
            } else if close(value, long, 0.22) {
                Some(1)
            } else {
                None
            }
        })
        .collect();
    let ambiguous = symbols.iter().filter(|symbol| symbol.is_none()).count();
    let usable: Vec<u8> = symbols.into_iter().flatten().collect();
    if usable.len() < 8 {
        return (Some(mode), CustomBitOrder::Ambiguous, None, ambiguous);
    }
    let msb = bits_to_bytes(&usable, false);
    let lsb = bits_to_bytes(&usable, true);
    // A small framing hint is the only order selection made here: a leading
    // flag byte of 0, 1, or FF is common in tape blocks.  Otherwise retain
    // both interpretations as ambiguous and refuse to invent bytes.
    let framing = |bytes: &[u8]| {
        bytes
            .first()
            .is_some_and(|byte| matches!(byte, 0 | 1 | 0xff))
    };
    let bit_order = if framing(&msb) && !framing(&lsb) {
        CustomBitOrder::MsbFirst
    } else if framing(&lsb) && !framing(&msb) {
        CustomBitOrder::LsbFirst
    } else {
        CustomBitOrder::Ambiguous
    };
    let bytes = match bit_order {
        CustomBitOrder::MsbFirst => msb,
        CustomBitOrder::LsbFirst => lsb,
        CustomBitOrder::Ambiguous => return (Some(mode), bit_order, None, ambiguous),
    };
    (Some(mode), bit_order, Some(bytes), ambiguous)
}

fn bits_to_bytes(bits: &[u8], lsb: bool) -> Vec<u8> {
    bits.chunks_exact(8)
        .map(|chunk| {
            chunk.iter().enumerate().fold(0u8, |acc, (index, bit)| {
                if lsb {
                    acc | (*bit << index)
                } else {
                    (acc << 1) | *bit
                }
            })
        })
        .take(4096)
        .collect()
}

/// Reuses the canonical TAP interpreter for blocks recovered from audio.
/// Invalid/partial blocks remain available in `SpectrumWavRecovery`, but are
/// not promoted into a TAP analysis until their checksums are valid.
pub fn recovered_tape_analysis(
    recovery: &SpectrumWavRecovery,
) -> Option<crate::tape_analysis::TapeAnalysis> {
    tape_analysis_from_blocks(
        recovery
            .blocks
            .iter()
            .map(|block| (&block.bytes, block.checksum_valid)),
    )
}

/// Attempts the same TAP handoff for custom bytes, but only when every
/// recovered block has an independently valid checksum. Generic bytes that do
/// not satisfy the TAP interpreter remain custom evidence instead.
pub fn custom_recovered_tape_analysis(
    recovery: &CustomWavRecovery,
) -> Option<crate::tape_analysis::TapeAnalysis> {
    tape_analysis_from_blocks(
        recovery
            .blocks
            .iter()
            .map(|block| (&block.bytes, block.checksum_valid)),
    )
}

/// Stable, sample-rate-independent fingerprint material for one custom stage.
/// It contains only normalized timing families, never filenames, sample
/// indices, or audio bytes.
pub fn custom_timing_fingerprint(stage: &CustomStageEvidence) -> Vec<u64> {
    let base = stage.pilot_micros.max(1);
    stage
        .clusters
        .iter()
        .map(|cluster| cluster.centre_micros.saturating_mul(1_000_000) / base)
        .collect()
}

fn tape_analysis_from_blocks<'a>(
    blocks: impl IntoIterator<Item = (&'a Vec<u8>, Option<bool>)>,
) -> Option<crate::tape_analysis::TapeAnalysis> {
    let mut tap = Vec::new();
    for (bytes, checksum_valid) in blocks {
        if bytes.len() < 2 || bytes.len() > u16::MAX as usize || checksum_valid != Some(true) {
            return None;
        }
        tap.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        tap.extend_from_slice(bytes);
    }
    (!tap.is_empty())
        .then(|| crate::tape_analysis::analyze_tape(&tap).ok())
        .flatten()
}

fn close(value: u64, target: u64, tolerance: f64) -> bool {
    let d = value.abs_diff(target) as f64;
    d <= target as f64 * tolerance
}
fn close_scaled_tolerant(value: u64, target: u64, scale: u64, tolerance: f64) -> bool {
    close(value, target.saturating_mul(scale) / 1_000_000, tolerance)
}
fn median(values: &[u64]) -> u64 {
    let mut v = values.to_vec();
    v.sort_unstable();
    v[v.len() / 2]
}

fn decode_wav(bytes: &[u8]) -> Result<(WavSpec, Vec<i32>), WavError> {
    if bytes.len() > MAX_WAV_BYTES {
        return Err(WavError::TooLarge);
    }
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(WavError::Malformed("RIFF/WAVE header"));
    }
    let mut pos = 12;
    let mut spec = None;
    let mut data = None;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        let end = start
            .checked_add(size)
            .ok_or(WavError::Malformed("chunk overflow"))?;
        if end > bytes.len() {
            return Err(WavError::Truncated);
        }
        match &bytes[pos..pos + 4] {
            b"fmt " => {
                if size < 16 {
                    return Err(WavError::Malformed("fmt chunk"));
                }
                let code = u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap());
                let channels = u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap());
                let rate = u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap());
                let bits = u16::from_le_bytes(bytes[start + 14..start + 16].try_into().unwrap());
                if code != 1 {
                    return Err(WavError::Unsupported("non-PCM WAV"));
                }
                if channels == 0 || channels > MAX_CHANNELS || rate == 0 || rate > MAX_SAMPLE_RATE {
                    return Err(WavError::Unsupported("channel/sample-rate limit"));
                }
                let format = match bits {
                    8 => PcmFormat::U8,
                    16 => PcmFormat::I16Le,
                    _ => return Err(WavError::Unsupported("PCM bit depth")),
                };
                spec = Some(WavSpec {
                    sample_rate: rate,
                    channels,
                    format,
                });
            }
            b"data" => data = Some((start, end)),
            _ => {}
        }
        pos = end + (size & 1);
    }
    let spec = spec.ok_or(WavError::Malformed("missing fmt chunk"))?;
    let (start, end) = data.ok_or(WavError::Malformed("missing data chunk"))?;
    let bps = match spec.format {
        PcmFormat::U8 => 1,
        PcmFormat::I16Le => 2,
    };
    let frame = bps * spec.channels as usize;
    if (end - start) % frame != 0 {
        return Err(WavError::Malformed("partial PCM frame"));
    }
    let frames = (end - start) / frame;
    if frames > MAX_WAV_BYTES / frame {
        return Err(WavError::TooLarge);
    }
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut sum: i64 = 0;
        for c in 0..spec.channels as usize {
            let i = start + f * frame + c * bps;
            sum += match spec.format {
                PcmFormat::U8 => i64::from(bytes[i] as i32 - 128) * 256,
                PcmFormat::I16Le => i64::from(i16::from_le_bytes([bytes[i], bytes[i + 1]])),
            };
        }
        out.push((sum / spec.channels as i64) as i32);
    }
    Ok((spec, out))
}

fn quality(samples: &[i32]) -> (i32, u32, u32) {
    if samples.is_empty() {
        return (0, 0, 0);
    }
    let mean = samples.iter().map(|v| i64::from(*v)).sum::<i64>() / samples.len() as i64;
    let mut peak = 0u32;
    let mut clipped = 0usize;
    for s in samples {
        let a = s.saturating_sub(mean as i32).unsigned_abs();
        peak = peak.max(a);
        if a >= 32_000 {
            clipped += 1;
        }
    }
    (
        mean as i32,
        peak,
        (clipped.saturating_mul(1_000_000) / samples.len()) as u32,
    )
}
fn detect_edges(samples: &[i32], threshold: i32, rate: u32) -> Vec<PulseEdge> {
    let mut out = Vec::new();
    let mut high = false;
    for (i, s) in samples.iter().enumerate() {
        if !high && *s >= threshold {
            high = true;
            if out.len() < MAX_EDGES {
                out.push(PulseEdge {
                    sample: i as u64,
                    micros: (i as u64) * 1_000_000 / rate as u64,
                    rising: true,
                });
            }
        } else if high && *s <= -threshold {
            high = false;
            if out.len() < MAX_EDGES {
                out.push(PulseEdge {
                    sample: i as u64,
                    micros: (i as u64) * 1_000_000 / rate as u64,
                    rising: false,
                });
            }
        }
    }
    out
}
fn summarize(edges: &[PulseEdge]) -> PulseSummary {
    let mut v: Vec<u64> = edges
        .windows(2)
        .map(|w| w[1].micros.saturating_sub(w[0].micros))
        .filter(|x| *x > 0)
        .collect();
    v.sort_unstable();
    PulseSummary {
        edge_count: edges.len(),
        median_interval_micros: v.get(v.len() / 2).copied(),
        min_interval_micros: v.first().copied(),
        max_interval_micros: v.last().copied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wav(rate: u32, ch: u16, bits: u16, payload: &[u8]) -> Vec<u8> {
        let block = 16 + 8 + payload.len();
        let mut b = Vec::from(&b"RIFF"[..]);
        b.extend_from_slice(&(36 + payload.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&ch.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        let byte_rate = rate * ch as u32 * (bits / 8) as u32;
        b.extend_from_slice(&byte_rate.to_le_bytes());
        b.extend_from_slice(&(ch * (bits / 8)).to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        b.extend_from_slice(payload);
        assert_eq!(b.len(), block + 20);
        b
    }
    #[test]
    fn parses_offset_8bit() {
        let a = analyze_wav(&wav(44100, 1, 8, &[0, 255, 0, 255])).unwrap();
        assert_eq!(a.spec.format, PcmFormat::U8);
        assert!(a.pulse_summary.edge_count > 0);
    }
    #[test]
    fn parses_stereo_16bit() {
        let mut p = Vec::new();
        for (l, r) in [(0i16, 0i16), (30000, -30000), (-30000, 30000)] {
            p.extend_from_slice(&l.to_le_bytes());
            p.extend_from_slice(&r.to_le_bytes());
        }
        let a = analyze_wav(&wav(48000, 2, 16, &p)).unwrap();
        assert_eq!(a.spec.channels, 2);
    }
    #[test]
    fn rejects_non_pcm() {
        let mut b = wav(44100, 1, 8, &[0]);
        b[20] = 3;
        assert!(matches!(analyze_wav(&b), Err(WavError::Unsupported(_))));
    }

    #[test]
    fn recovers_a_rom_timed_byte() {
        let rate = 22_050u32;
        let mut samples = vec![128u8; rate as usize / 10];
        let mut level = 220i16;
        let mut add = |micros: u64| {
            let n = (micros * rate as u64 / 1_000_000).max(1) as usize;
            samples.extend(std::iter::repeat_n(if level > 0 { 255 } else { 0 }, n));
            level = -level;
        };
        for _ in 0..141 {
            add(2168);
        }
        add(667);
        add(735);
        for bit in [1, 0, 1, 0, 0, 1, 0, 1] {
            add(if bit == 1 { 1710 } else { 855 });
            add(if bit == 1 { 1710 } else { 855 });
        }
        add(855);
        let result = decode_spectrum_wav(&wav(rate, 1, 8, &samples)).unwrap();
        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].bytes, vec![0xA5]);
    }

    #[test]
    fn discovers_and_recovers_a_non_rom_paired_waveform() {
        let rate = 44_100u32;
        let mut samples = vec![128u8; rate as usize / 20];
        let mut high = true;
        let mut add = |micros: u64| {
            let count = (micros * rate as u64 / 1_000_000).max(1) as usize;
            samples.extend(std::iter::repeat_n(if high { 255 } else { 0 }, count));
            high = !high;
        };
        for _ in 0..80 {
            add(1200);
        }
        add(300);
        // 0x01 followed by 0x02 gives the framing hint needed to select MSB.
        for bit in [0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, 0] {
            let pulse = if bit == 0 { 400 } else { 800 };
            add(pulse);
            add(pulse);
        }
        samples.extend(std::iter::repeat_n(0, 100));
        let result = decode_custom_wav(&wav(rate, 1, 8, &samples)).unwrap();
        assert_eq!(result.loader_class, "GenericTurbo");
        assert_eq!(result.stages.len(), 1);
        assert_eq!(result.blocks[0].mode, CustomSymbolMode::PairedPulse);
        assert_eq!(result.blocks[0].bit_order, CustomBitOrder::MsbFirst);
        assert_eq!(result.blocks[0].bytes, vec![0x01, 0x02]);
    }

    #[test]
    fn standard_rom_timing_is_not_reclassified_as_custom() {
        let rate = 22_050u32;
        let mut samples = vec![128u8; rate as usize / 20];
        let mut high = true;
        let mut add = |micros: u64| {
            let count = (micros * rate as u64 / 1_000_000).max(1) as usize;
            samples.extend(std::iter::repeat_n(if high { 255 } else { 0 }, count));
            high = !high;
        };
        for _ in 0..80 {
            add(2168);
        }
        add(667);
        add(735);
        let result = decode_custom_wav(&wav(rate, 1, 8, &samples)).unwrap();
        assert!(result.stages.is_empty());
        assert_eq!(result.loader_class, "UnknownCustom");
    }

    #[test]
    fn custom_fingerprint_uses_normalized_timing_not_sample_indices() {
        let stage = CustomStageEvidence {
            start_micros: 10,
            end_micros: 100,
            pilot_micros: 1200,
            pilot_count: 64,
            sync_pulses: vec![300],
            clusters: vec![
                CustomTimingCluster {
                    centre_micros: 400,
                    count: 20,
                    spread_micros: 2,
                    confidence: RecoveryConfidence::High,
                },
                CustomTimingCluster {
                    centre_micros: 800,
                    count: 20,
                    spread_micros: 3,
                    confidence: RecoveryConfidence::High,
                },
            ],
            symbol_mode: Some(CustomSymbolMode::PairedPulse),
            bit_order: CustomBitOrder::Ambiguous,
            ambiguous_symbols: 0,
            confidence: RecoveryConfidence::High,
        };
        assert_eq!(custom_timing_fingerprint(&stage), vec![333_333, 666_666]);
    }

    fn commodore_stream(payload: &[u8], second_copy: bool) -> Vec<u64> {
        let (short, medium, long) = (176u64, 256u64, 336u64);
        let mut intervals = vec![short; 72];
        let countdown: [u8; 9] = if second_copy {
            [0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]
        } else {
            [0x89, 0x88, 0x87, 0x86, 0x85, 0x84, 0x83, 0x82, 0x81]
        };
        let write_byte = |byte: u8, output: &mut Vec<u64>| {
            output.extend([long, medium]);
            let mut parity = 1u8;
            for bit in 0..8 {
                let value = (byte >> bit) & 1;
                parity ^= value;
                if value == 0 {
                    output.extend([short, medium]);
                } else {
                    output.extend([medium, short]);
                }
            }
            if parity == 0 {
                output.extend([short, medium]);
            } else {
                output.extend([medium, short]);
            }
        };
        for byte in countdown.into_iter().chain(payload.iter().copied()) {
            write_byte(byte, &mut intervals);
        }
        write_byte(
            payload.iter().fold(0u8, |sum, byte| sum ^ byte),
            &mut intervals,
        );
        intervals.extend([long, short]);
        intervals
    }

    fn commodore_wav(rate: u32, streams: &[Vec<u8>]) -> Vec<u8> {
        let mut samples = vec![128u8; rate as usize / 50];
        let mut high = true;
        for (index, stream) in streams.iter().enumerate() {
            for micros in commodore_stream(stream, index % 2 == 1) {
                let count = ((micros * rate as u64 + 500_000) / 1_000_000).max(1) as usize;
                samples.extend(std::iter::repeat_n(if high { 255 } else { 0 }, count));
                high = !high;
            }
        }
        wav(rate, 1, 8, &samples)
    }

    /// A synthetic standard C64 bootstrap followed by a deliberately generic
    /// custom stage. The custom bytes use V5's documented framing hint; they
    /// are not a commercial fastloader signature or a C64 header.
    fn commodore_custom_wav(rate: u32, pilot: u64, zero: u64, one: u64, paired: bool) -> Vec<u8> {
        let header = commodore_header(b"BOOT", 0x0801, 0x0801);
        let mut samples = vec![128u8; rate as usize / 50];
        let mut high = true;
        let mut add = |micros: u64| {
            let count = ((micros * rate as u64 + 500_000) / 1_000_000).max(1) as usize;
            samples.extend(std::iter::repeat_n(if high { 255 } else { 0 }, count));
            high = !high;
        };
        for micros in commodore_stream(&header, false) {
            add(micros);
        }
        // A gap makes the non-ROM stage boundary explicit without retaining
        // a raw-sample position in the resulting fingerprint.
        // This interval is deliberately outside the bounded custom-pulse
        // window, making a stage boundary without a second PCM representation.
        add(50_000);
        for _ in 0..80 {
            add(pilot);
        }
        add(pilot / 4);
        // 0x01, 0x02, 0x04, 0x08 is V5's synthetic framing hint for unambiguous MSB
        // recovery. It must never be interpreted as a standard C64 header.
        for byte in [0x01, 0x02, 0x04, 0x08] {
            for bit in (0..8).rev().map(|bit| (byte >> bit) & 1) {
                let pulse = if bit == 0 { zero } else { one };
                add(pulse);
                if paired {
                    add(pulse);
                }
            }
        }
        // Retain the final encoded pulse in the edge-interval stream.
        add(zero);
        wav(rate, 1, 8, &samples)
    }

    fn commodore_header(name: &[u8], start: u16, end: u16) -> Vec<u8> {
        let mut header = vec![
            0x03,
            start as u8,
            (start >> 8) as u8,
            end as u8,
            (end >> 8) as u8,
        ];
        header.extend_from_slice(name);
        header.resize(21, b' ');
        header.resize(COMMODORE_PAYLOAD_BYTES, b' ');
        header
    }

    fn cpc_stream(sync: u8, payload: &[u8], one: u64, corrupt_crc: bool) -> Vec<u64> {
        let zero = one / 2;
        let mut intervals = vec![one; 512];
        intervals.extend([zero, zero]);
        let mut write_byte = |byte: u8| {
            for bit in (0..8).rev() {
                let pulse = if (byte >> bit) & 1 == 0 { zero } else { one };
                intervals.extend([pulse, pulse]);
            }
        };
        write_byte(sync);
        let mut segment = payload.to_vec();
        segment.resize(CPC_SEGMENT_BYTES, 0);
        for byte in &segment {
            write_byte(*byte);
        }
        let mut crc = (!cpc_crc16(&segment)).to_be_bytes();
        if corrupt_crc {
            crc[1] ^= 0x01;
        }
        write_byte(crc[0]);
        write_byte(crc[1]);
        intervals
    }

    fn cpc_wav(rate: u32, one: u64, corrupt_crc: bool, custom_tail: bool) -> Vec<u8> {
        let mut header = vec![0u8; CPC_SEGMENT_BYTES];
        header[..8].copy_from_slice(b"TEST    ");
        header[16] = 1;
        header[17] = 1;
        header[18] = 1;
        header[19..21].copy_from_slice(&(32u16).to_le_bytes());
        header[21..23].copy_from_slice(&(0x4000u16).to_le_bytes());
        header[23] = 1;
        header[24..26].copy_from_slice(&(32u16).to_le_bytes());
        header[26..28].copy_from_slice(&(0x4000u16).to_le_bytes());
        let mut intervals = cpc_stream(CPC_SYNC_HEADER, &header, one, corrupt_crc);
        intervals.extend([50_000]);
        if custom_tail {
            intervals.extend([one * 4, one * 4, one * 4, one * 4]);
        }
        let mut samples = vec![128u8; rate as usize / 50];
        let mut high = true;
        for micros in intervals {
            let count = ((micros * rate as u64 + 500_000) / 1_000_000).max(1) as usize;
            samples.extend(std::iter::repeat_n(if high { 255 } else { 0 }, count));
            high = !high;
        }
        wav(rate, 1, 8, &samples)
    }

    #[test]
    fn cpc_standard_waveform_recovers_header_across_sample_rates() {
        for rate in [22_050, 44_100, 48_000, 96_000] {
            let result = decode_amstrad_cpc_wav(&cpc_wav(rate, 1000, false, false)).unwrap();
            assert_eq!(result.blocks.len(), 1, "rate {rate}");
            let block = &result.blocks[0];
            assert_eq!(block.sync_byte, CPC_SYNC_HEADER);
            assert_eq!(block.filename.as_deref(), Some("TEST"));
            assert_eq!(block.block_number, Some(1));
            assert_eq!(block.first_block, Some(true));
            assert_eq!(block.last_block, Some(true));
            assert_eq!(block.load_address, Some(0x4000));
            assert_eq!(block.length, Some(32));
            assert_eq!(block.execution_address, Some(0x4000));
            assert_eq!(block.checksum_valid, Some(true));
        }
    }

    #[test]
    fn cpc_checksum_timing_and_custom_boundary_fail_soft() {
        let bad = decode_amstrad_cpc_wav(&cpc_wav(44_100, 1000, true, false)).unwrap();
        assert_eq!(bad.blocks.len(), 1);
        assert_eq!(bad.blocks[0].checksum_valid, Some(false));
        let drift = decode_amstrad_cpc_wav(&cpc_wav(48_000, 1100, false, true)).unwrap();
        assert_eq!(drift.blocks[0].checksum_valid, Some(true));
        assert!(drift.custom_stage_candidate);
        assert!(drift.warnings.iter().any(|w| w.contains("deferred")));
    }

    #[test]
    fn cpc_projection_and_false_positive_guards() {
        let analysis = amstrad_cpc_wav_tape_analysis(&cpc_wav(44_100, 1000, false, false)).unwrap();
        assert_eq!(
            analysis.format,
            crate::tape_analysis::TapeFormat::AmstradCpcWav
        );
        assert_eq!(analysis.platform, Some("Amstrad CPC"));
        assert_eq!(analysis.entries[0].name.as_deref(), Some("TEST"));
        assert!(
            decode_amstrad_cpc_wav(&wav(44_100, 1, 8, &[0, 255, 0, 255]))
                .unwrap()
                .blocks
                .is_empty()
        );
        let mut spectrum_intervals = vec![2168u64; 100];
        spectrum_intervals.extend([667, 735]);
        for bit in (0..8).rev().map(|i| (0xa5 >> i) & 1) {
            let pulse = if bit == 0 { 855 } else { 1710 };
            spectrum_intervals.extend([pulse, pulse]);
        }
        let mut samples = vec![128u8; 44_100 / 50];
        let mut high = true;
        for micros in spectrum_intervals {
            let count = ((micros * 44_100 + 500_000) / 1_000_000).max(1) as usize;
            samples.extend(std::iter::repeat_n(if high { 255 } else { 0 }, count));
            high = !high;
        }
        assert!(
            decode_amstrad_cpc_wav(&wav(44_100, 1, 8, &samples))
                .unwrap()
                .blocks
                .is_empty()
        );
    }

    #[test]
    fn recovers_standard_commodore_header_data_and_duplicate_copies() {
        let header = commodore_header(b"SYNTH", 0x0801, 0x0804);
        let mut data = vec![1, 2, 3];
        data.resize(COMMODORE_PAYLOAD_BYTES, 0);
        let wav = commodore_wav(44_100, &[header.clone(), header, data.clone(), data]);
        let recovery = decode_commodore_wav(&wav).unwrap();
        assert_eq!(recovery.blocks.len(), 4);
        assert_eq!(recovery.blocks[0].checksum_valid, Some(true));
        assert_eq!(recovery.blocks[1].duplicate_of, Some(0));
        assert_eq!(recovery.blocks[3].duplicate_of, Some(2));
        let analysis = commodore_wav_tape_analysis(&wav).unwrap();
        assert_eq!(analysis.entries.len(), 1);
        assert_eq!(analysis.entries[0].name.as_deref(), Some("SYNTH"));
        assert_eq!(analysis.entries[0].load_address, Some(0x0801));
        assert_eq!(analysis.entries[0].length, 3);
    }

    #[test]
    fn commodore_standard_decoder_is_sample_rate_independent() {
        let header = commodore_header(b"RATE", 0x1000, 0x1001);
        let mut data = vec![42];
        data.resize(COMMODORE_PAYLOAD_BYTES, 0);
        for rate in [22_050, 44_100, 48_000, 96_000] {
            let recovery =
                decode_commodore_wav(&commodore_wav(rate, &[header.clone(), data.clone()]))
                    .unwrap();
            assert_eq!(recovery.blocks.len(), 2, "rate {rate}");
            assert!(
                recovery
                    .blocks
                    .iter()
                    .all(|block| block.checksum_valid == Some(true)),
                "rate {rate}"
            );
        }
    }

    #[test]
    fn spectrum_and_custom_waveforms_are_not_commodore() {
        let rate = 22_050;
        let mut samples = vec![128u8; rate as usize / 50];
        let mut high = true;
        for micros in std::iter::repeat_n(2168, 80).chain([667, 735]) {
            let count = (micros * rate as u64 / 1_000_000).max(1) as usize;
            samples.extend(std::iter::repeat_n(if high { 255 } else { 0 }, count));
            high = !high;
        }
        let recovery = decode_commodore_wav(&wav(rate, 1, 8, &samples)).unwrap();
        assert!(recovery.blocks.is_empty());
        let custom = decode_commodore_custom_wav(&wav(rate, 1, 8, &samples)).unwrap();
        assert_eq!(custom.loader_class, "UnknownCustom");
        assert!(custom.stages.is_empty());
    }

    #[test]
    fn c64_custom_stage_uses_standard_bootstrap_and_recovers_generic_bytes() {
        let recovered =
            decode_commodore_custom_wav(&commodore_custom_wav(44_100, 1200, 400, 800, true))
                .unwrap();
        assert_eq!(recovered.standard_blocks, 1);
        assert_eq!(recovered.loader_class, "GenericTurbo");
        assert_eq!(recovered.stages.len(), 1);
        assert_eq!(recovered.blocks.len(), 1);
        assert_eq!(recovered.blocks[0].bytes, vec![0x01, 0x02, 0x04, 0x08]);
        assert_eq!(recovered.blocks[0].mode, CustomSymbolMode::PairedPulse);
        assert_eq!(recovered.blocks[0].bit_order, CustomBitOrder::MsbFirst);
    }

    #[test]
    fn c64_custom_stage_is_sample_rate_stable_and_timing_sensitive() {
        let at_44100 =
            decode_commodore_custom_wav(&commodore_custom_wav(44_100, 1200, 400, 800, true))
                .unwrap();
        let at_48000 =
            decode_commodore_custom_wav(&commodore_custom_wav(48_000, 1200, 400, 800, true))
                .unwrap();
        let changed =
            decode_commodore_custom_wav(&commodore_custom_wav(48_000, 1200, 400, 1000, true))
                .unwrap();
        assert_eq!(at_44100.fingerprint, at_48000.fingerprint);
        assert_ne!(at_44100.fingerprint, changed.fingerprint);
    }

    #[test]
    fn c64_single_pulse_custom_bytes_remain_generic_and_do_not_make_metadata() {
        let fixture = commodore_custom_wav(44_100, 1200, 400, 800, false);
        let recovered = decode_commodore_custom_wav(&fixture).unwrap();
        assert_eq!(recovered.standard_blocks, 1);
        assert_eq!(recovered.loader_class, "CustomPulse");
        assert_eq!(recovered.stages.len(), 1);
        assert_eq!(recovered.blocks[0].bytes, vec![0x01, 0x02, 0x04, 0x08]);
        assert_eq!(recovered.blocks[0].mode, CustomSymbolMode::SinglePulse);
        // The valid bootstrap header deliberately has no matching standard
        // data record. Generic custom bytes must not be mistaken for one.
        assert!(
            commodore_wav_tape_analysis(&fixture)
                .unwrap()
                .entries
                .is_empty()
        );
    }
}
