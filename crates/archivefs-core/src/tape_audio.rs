//! Bounded PCM/WAV tape-audio evidence. This is deliberately a signal layer:
//! it validates PCM, conditions samples, and reports pulse timing. It does not
//! emulate loaders, decode compressed audio, or retain raw PCM in results.

use std::convert::TryInto;

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

/// Reuses the canonical TAP interpreter for blocks recovered from audio.
/// Invalid/partial blocks remain available in `SpectrumWavRecovery`, but are
/// not promoted into a TAP analysis until their checksums are valid.
pub fn recovered_tape_analysis(
    recovery: &SpectrumWavRecovery,
) -> Option<crate::tape_analysis::TapeAnalysis> {
    let mut tap = Vec::new();
    for block in &recovery.blocks {
        if block.bytes.len() < 2 || block.bytes.len() > u16::MAX as usize {
            return None;
        }
        if block.checksum_valid != Some(true) {
            return None;
        }
        tap.extend_from_slice(&(block.bytes.len() as u16).to_le_bytes());
        tap.extend_from_slice(&block.bytes);
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
}
