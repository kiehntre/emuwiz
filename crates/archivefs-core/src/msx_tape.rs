//! Conservative MSX standard cassette WAV evidence.
//! The decoder reuses the shared PCM edge pipeline and only accepts the
//! documented MSX BIOS header signature; carrier similarity alone is generic.

use crate::tape_audio::{RecoveryConfidence, WavError, analyze_wav};

const BIT_US: u64 = 833;
const MAX_BLOCKS: usize = 128;
const MAX_BYTES: usize = 64 * 1024;
const MSX_MAGIC: [u8; 8] = [0x1f, 0xa6, 0xde, 0xba, 0xcc, 0x13, 0x7d, 0x74];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsxIntegrity {
    Valid,
    Invalid,
    NotPresent,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MsxRecoveredFile {
    pub filename: Option<String>,
    pub file_type: Option<u8>,
    pub payload_length: usize,
    pub start_sample: u64,
    pub end_sample: u64,
    pub start_micros: u64,
    pub end_micros: u64,
    pub integrity: MsxIntegrity,
    pub confidence: RecoveryConfidence,
    pub warnings: Vec<String>,
    pub marker_sample_start: u64,
    pub marker_sample_end: u64,
    pub marker_time_start_s: f64,
    pub marker_time_end_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MsxWavRecovery {
    pub audio: crate::tape_audio::TapeAudioAnalysis,
    pub files: Vec<MsxRecoveredFile>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsxCustomWavRecovery {
    pub standard_files: usize,
    pub stages: Vec<crate::tape_audio::CustomStageEvidence>,
    pub blocks: Vec<crate::tape_audio::CustomRecoveredBlock>,
    pub loader_class: &'static str,
    pub fingerprint: String,
    pub warnings: Vec<String>,
}

/// Recover standard MSX 1200-baud FSK bytes.  MSX uses one 1200-Hz cycle for
/// zero and two 2400-Hz cycles for one, with an MSX BIOS header marker.  The
/// byte stream is treated as evidence; CAS/container parsing remains separate.
pub fn decode_msx_wav(bytes: &[u8]) -> Result<MsxWavRecovery, WavError> {
    let audio = analyze_wav(bytes)?;
    let intervals: Vec<u64> = audio
        .edges
        .windows(2)
        .map(|w| w[1].micros.saturating_sub(w[0].micros))
        .collect();
    let mut symbols = Vec::new();
    let mut cell_start = 0usize;
    let mut elapsed = 0;
    let mut fast = 0;
    let mut slow = 0;
    for (interval_index, p) in intervals.iter().copied().enumerate() {
        if p == 0 || p > 2_000 {
            elapsed = 0;
            fast = 0;
            slow = 0;
            continue;
        }
        if p < 310 {
            fast += 1;
        } else if p >= 320 {
            slow += 1;
        }
        elapsed += p;
        if elapsed >= BIT_US {
            let start = audio.edges.get(cell_start).map(|e| e.sample).unwrap_or(0);
            let end = audio
                .edges
                .get(interval_index + 1)
                .map(|e| e.sample)
                .unwrap_or(start);
            symbols.push((fast >= slow, start, end));
            cell_start = interval_index + 1;
            elapsed = 0;
            fast = 0;
            slow = 0;
        }
    }
    let mut out = Vec::new();
    let mut spans = Vec::new();
    let mut i = 0;
    while i + 9 < symbols.len() && out.len() < MAX_BYTES {
        if symbols[i].0 {
            i += 1;
            continue;
        }
        let mut v = 0u8;
        let ok = symbols[i + 9].0;
        for bit in 0..8 {
            if symbols[i + 1 + bit].0 {
                v |= 1 << bit;
            }
        }
        if ok {
            out.push(v);
            spans.push((symbols[i].1, symbols[i + 9].2));
            i += 10;
        } else {
            i += 1;
        }
    }
    let mut files = Vec::new();
    let mut cursor = 0;
    while cursor + 18 <= out.len() && files.len() < MAX_BLOCKS {
        let Some(mark) = out[cursor..]
            .windows(MSX_MAGIC.len())
            .position(|w| w == MSX_MAGIC)
        else {
            break;
        };
        let at = cursor + mark;
        let Some(ea) = out[at + 8..].iter().position(|b| *b == 0xea) else {
            cursor = at + 8;
            continue;
        };
        let header = at + 8 + ea;
        if header + 7 > out.len() {
            break;
        }
        let filename = String::from_utf8(out[header + 1..(header + 7).min(out.len())].to_vec())
            .ok()
            .map(|s| s.trim_matches('\0').trim().to_owned())
            .filter(|s| !s.is_empty());
        let (marker_sample_start, marker_sample_end) = spans.get(at).copied().unwrap_or((0, 0));
        let file_end = spans
            .get((header + 7).min(spans.len().saturating_sub(1)))
            .map(|s| s.1)
            .unwrap_or(marker_sample_end);
        files.push(MsxRecoveredFile {
            filename,
            file_type: None,
            payload_length: 0,
            start_sample: marker_sample_start,
            end_sample: file_end,
            start_micros: marker_sample_start.saturating_mul(1_000_000)
                / audio.spec.sample_rate as u64,
            end_micros: file_end.saturating_mul(1_000_000) / audio.spec.sample_rate as u64,
            integrity: MsxIntegrity::Valid,
            confidence: RecoveryConfidence::Medium,
            warnings: Vec::new(),
            marker_sample_start,
            marker_sample_end,
            marker_time_start_s: marker_sample_start as f64 / audio.spec.sample_rate as f64,
            marker_time_end_s: marker_sample_end as f64 / audio.spec.sample_rate as f64,
        });
        cursor = header + 7;
    }
    let has_files = !files.is_empty();
    Ok(MsxWavRecovery {
        audio,
        files,
        warnings: if !has_files {
            vec!["WAV recognised, but no MSX BIOS header was recovered".into()]
        } else {
            Vec::new()
        },
    })
}

/// Bootstrap-gated generic analysis for later MSX turbo/custom stages.  No
/// stage is labelled MSX unless a standard MSX BIOS marker was recovered.
pub fn decode_msx_custom_wav(bytes: &[u8]) -> Result<MsxCustomWavRecovery, WavError> {
    let standard = decode_msx_wav(bytes)?;
    if standard.files.is_empty() {
        return Ok(MsxCustomWavRecovery {
            standard_files: 0,
            stages: Vec::new(),
            blocks: Vec::new(),
            loader_class: "UnknownCustom",
            fingerprint: crate::tape_audio::custom_loader_fingerprint(&[]),
            warnings: vec!["MSX custom analysis requires a recovered standard MSX marker".into()],
        });
    }
    let generic = crate::tape_audio::decode_custom_wav(bytes)?;
    let anchor_end_sample = standard
        .files
        .iter()
        .map(|f| f.end_sample)
        .max()
        .unwrap_or(0);
    let anchor_end =
        anchor_end_sample.saturating_mul(1_000_000) / standard.audio.spec.sample_rate as u64;
    let stages: Vec<_> = generic
        .stages
        .into_iter()
        .filter(|s| s.start_micros >= anchor_end)
        .take(64)
        .collect();
    let blocks: Vec<_> = generic
        .blocks
        .into_iter()
        .filter(|b| b.start_micros >= anchor_end)
        .take(256)
        .collect();
    let loader_class = if stages.len() > 1 {
        "MultiStage"
    } else if stages
        .iter()
        .any(|s| s.symbol_mode == Some(crate::tape_audio::CustomSymbolMode::PairedPulse))
    {
        "GenericTurbo"
    } else if !stages.is_empty() {
        "CustomPulse"
    } else {
        "UnknownCustom"
    };
    let fingerprint = crate::tape_audio::custom_loader_fingerprint(&stages);
    Ok(MsxCustomWavRecovery {
        standard_files: standard.files.len(),
        stages,
        blocks,
        loader_class,
        fingerprint,
        warnings: Vec::new(),
    })
}
