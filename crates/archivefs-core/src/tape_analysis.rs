//! Bounded, read-only tape content analysis built on the existing structural
//! tape parsers. This adds descriptive metadata only; it never emulates or
//! extracts tape payloads and never claims a game identity from a filename.

use crate::commodore_tape::{
    COMMODORE_TAP_HEADER_BYTES, T64_READ_BYTES, parse_commodore_tap, parse_t64,
};
use crate::tape_identity::{TzxBlockDetails, ZxTapBlockKind, parse_tzx, parse_zx_tap};
use sha2::{Digest, Sha256};

pub const MAX_ANALYSIS_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ANALYSIS_ENTRIES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapeFormat {
    ZxTap,
    Tzx,
    CommodoreTap,
    CommodoreWav,
    AmstradCpcWav,
    BbcMicroWav,
    MsxWav,
    Atari8BitWav,
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
pub enum LoaderClass {
    RomStandard,
    GenericTurbo,
    CustomPulse,
    MultiStage,
    UnknownCustom,
    KnownFamily(KnownLoaderFamily),
}
/// A named family is interpretation of existing loader evidence, never game
/// identity. It is emitted only for a documented multi-clue structural match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownLoaderFamily {
    Alkatraz,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LoaderConfidence {
    Low,
    Medium,
    High,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderEvidence {
    pub class: LoaderClass,
    pub confidence: LoaderConfidence,
    pub fingerprint: String,
    pub clues: Vec<String>,
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
    pub loader: Option<LoaderEvidence>,
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
            loader: Some(classify_tzx_loader(&observation.blocks)),
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
    let mut has_basic = false;
    for header in obs.metadata.into_iter().take(MAX_ANALYSIS_ENTRIES) {
        let (kind, address) = match header.file_type {
            0 => {
                has_basic = true;
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
        loader: Some(LoaderEvidence {
            class: LoaderClass::RomStandard,
            confidence: if checksum == ChecksumState::Valid {
                LoaderConfidence::High
            } else {
                LoaderConfidence::Medium
            },
            fingerprint: fingerprint(&["rom-standard"]),
            clues: if has_basic {
                vec!["Spectrum ROM-standard BASIC header".into()]
            } else {
                vec!["Spectrum ROM-standard header".into()]
            },
        }),
        checksum,
        warnings,
        semantic_blocks: Vec::new(),
        logical_segments: entry_count.max(1),
        unsupported_blocks: 0,
    })
}

fn classify_tzx_loader(blocks: &[crate::tape_identity::TzxBlock]) -> LoaderEvidence {
    let mut has_standard = false;
    let mut turbo = 0usize;
    let mut pulse = 0usize;
    let mut tokens = Vec::new();
    for block in blocks {
        let token = match &block.details {
            TzxBlockDetails::Standard { pause_ms, data_len } => {
                has_standard = true;
                format!("s:{data_len}:{pause_ms}")
            }
            TzxBlockDetails::Turbo {
                pilot,
                zero,
                one,
                pilot_count,
                data_len,
                ..
            } => {
                turbo += 1;
                format!("t:{pilot}:{zero}:{one}:{pilot_count}:{data_len}")
            }
            TzxBlockDetails::PureData {
                zero,
                one,
                data_len,
                ..
            } => {
                turbo += 1;
                format!("p:{zero}:{one}:{data_len}")
            }
            TzxBlockDetails::PureTone {
                pulse: pulse_len,
                count,
            } => {
                pulse += 1;
                format!("tone:{pulse_len}:{count}")
            }
            TzxBlockDetails::PulseSequence { count, min, max } => {
                pulse += 1;
                format!("pulse:{count}:{min}:{max}")
            }
            TzxBlockDetails::Pause { duration_ms } => format!("pause:{duration_ms}"),
            TzxBlockDetails::None => "opaque".into(),
        };
        tokens.push(token);
    }
    let (mut class, mut confidence, mut clues) = if turbo == 0 && pulse == 0 && has_standard {
        (
            LoaderClass::RomStandard,
            LoaderConfidence::High,
            vec!["standard-speed data blocks".into()],
        )
    } else if turbo > 0 && has_standard {
        (
            LoaderClass::MultiStage,
            LoaderConfidence::Medium,
            vec!["standard bootstrap followed by custom-timed data".into()],
        )
    } else if turbo > 1 {
        (
            LoaderClass::GenericTurbo,
            LoaderConfidence::Medium,
            vec!["multiple custom-timed data stages".into()],
        )
    } else if turbo == 1 {
        (
            LoaderClass::GenericTurbo,
            LoaderConfidence::Low,
            vec!["non-ROM timing parameters".into()],
        )
    } else if pulse > 0 {
        (
            LoaderClass::CustomPulse,
            LoaderConfidence::Low,
            vec!["custom pulse structure".into()],
        )
    } else {
        (
            LoaderClass::UnknownCustom,
            LoaderConfidence::Low,
            vec!["opaque or unsupported timing block".into()],
        )
    };
    if tokens.iter().any(|t| t == "opaque") {
        clues.push("opaque block reduces confidence".into());
    }
    if let Some((family, family_confidence, family_clues)) = known_family_from_tzx(blocks) {
        class = LoaderClass::KnownFamily(family);
        confidence = family_confidence;
        clues.extend(family_clues);
    }
    LoaderEvidence {
        class,
        confidence,
        fingerprint: fingerprint(&tokens),
        clues,
    }
}

/// Applies the documented Alkatraz structure only when every independently
/// observable clue agrees. One pulse count, a loader name, or an incidental
/// custom block can never produce a named result.
fn known_family_from_tzx(
    blocks: &[crate::tape_identity::TzxBlock],
) -> Option<(KnownLoaderFamily, LoaderConfidence, Vec<String>)> {
    for window in blocks.windows(4) {
        let [bootstrap, first_turbo, gap, second_stage] = window else {
            continue;
        };
        let TzxBlockDetails::Standard { data_len, .. } = bootstrap.details else {
            continue;
        };
        let TzxBlockDetails::Turbo {
            pilot_count,
            data_len: first_len,
            data_flag,
            ..
        } = first_turbo.details
        else {
            continue;
        };
        let TzxBlockDetails::Pause { duration_ms } = gap.details else {
            continue;
        };
        let second_is_custom = match second_stage.details {
            TzxBlockDetails::Turbo { data_len, .. }
            | TzxBlockDetails::PureData { data_len, .. } => data_len > 0,
            _ => false,
        };
        // The published structural signature is: standard bootstrap;
        // headerless turbo with about a 240-pulse pilot; a roughly 12 second
        // gap; then more custom data. The intervals below deliberately allow
        // only modest capture/encoder jitter.
        if data_len == 0
            || !(192..=288).contains(&pilot_count)
            || first_len == 0
            || data_flag == Some(0)
            || !(10_000..=14_000).contains(&duration_ms)
            || !second_is_custom
        {
            continue;
        }
        return Some((
            KnownLoaderFamily::Alkatraz,
            LoaderConfidence::High,
            vec![
                "Alkatraz: standard bootstrap before custom stages".into(),
                "Alkatraz: short 192–288 pulse turbo pilot".into(),
                "Alkatraz: headerless turbo framing".into(),
                "Alkatraz: 10–14 second inter-stage gap followed by custom data".into(),
            ],
        ));
    }
    None
}

fn fingerprint(tokens: &[impl AsRef<str>]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"loader-signature-v1\0");
    for token in tokens {
        hash.update(token.as_ref().as_bytes());
        hash.update([0]);
    }
    let digest = hash.finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
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
            ..
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

    #[test]
    fn loader_fingerprint_is_versioned_and_ignores_titles() {
        assert_eq!(
            fingerprint(&["turbo:2168:855:1710"]),
            fingerprint(&["turbo:2168:855:1710"])
        );
        assert_ne!(
            fingerprint(&["turbo:2168:855:1710"]),
            fingerprint(&["turbo:2168:2168:1710"])
        );
    }

    fn tzx_header() -> Vec<u8> {
        b"ZXTape!\x1a\x01\x14".to_vec()
    }

    fn standard_block(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x10];
        out.extend_from_slice(&1000u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    fn turbo_block(pilot_count: u16, flag: u8, data_len: usize) -> Vec<u8> {
        let data_len = data_len.max(1);
        let mut out = vec![0x11];
        for timing in [2168u16, 667, 735, 855, 1710, pilot_count] {
            out.extend_from_slice(&timing.to_le_bytes());
        }
        out.push(8);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(data_len as u32).to_le_bytes()[..3]);
        out.push(flag);
        out.extend(std::iter::repeat_n(0x55, data_len - 1));
        out
    }

    fn pause_block(duration_ms: u16) -> Vec<u8> {
        let mut out = vec![0x20];
        out.extend_from_slice(&duration_ms.to_le_bytes());
        out
    }

    fn alkatraz_fixture(pilot_count: u16, gap_ms: u16, include_tail: bool) -> Vec<u8> {
        let mut out = tzx_header();
        out.extend(standard_block(&[0, 1, 2]));
        out.extend(turbo_block(pilot_count, 0xff, 3));
        out.extend(pause_block(gap_ms));
        if include_tail {
            out.extend(turbo_block(300, 0xff, 3));
        }
        out
    }

    #[test]
    fn alkatraz_requires_documented_multi_stage_signature() {
        let analysis = analyze_tape(&alkatraz_fixture(240, 12_000, true)).unwrap();
        assert_eq!(
            analysis.loader.as_ref().unwrap().class,
            LoaderClass::KnownFamily(KnownLoaderFamily::Alkatraz)
        );
        assert_eq!(
            analysis.loader.as_ref().unwrap().confidence,
            LoaderConfidence::High
        );
    }

    #[test]
    fn alkatraz_accepts_modest_documented_pilot_jitter() {
        let analysis = analyze_tape(&alkatraz_fixture(252, 12_600, true)).unwrap();
        assert_eq!(
            analysis.loader.as_ref().unwrap().class,
            LoaderClass::KnownFamily(KnownLoaderFamily::Alkatraz)
        );
    }

    #[test]
    fn alkatraz_near_misses_stay_generic() {
        for fixture in [
            alkatraz_fixture(240, 8_000, true),
            alkatraz_fixture(350, 12_000, true),
            alkatraz_fixture(240, 12_000, false),
        ] {
            let analysis = analyze_tape(&fixture).unwrap();
            assert!(!matches!(
                analysis.loader.unwrap().class,
                LoaderClass::KnownFamily(_)
            ));
        }
    }

    #[test]
    fn alkatraz_standard_header_framing_is_not_named() {
        let mut fixture = tzx_header();
        fixture.extend(standard_block(&[0, 1, 2]));
        fixture.extend(turbo_block(240, 0, 3));
        fixture.extend(pause_block(12_000));
        fixture.extend(turbo_block(300, 0xff, 3));
        let analysis = analyze_tape(&fixture).unwrap();
        assert!(!matches!(
            analysis.loader.unwrap().class,
            LoaderClass::KnownFamily(_)
        ));
    }
}
