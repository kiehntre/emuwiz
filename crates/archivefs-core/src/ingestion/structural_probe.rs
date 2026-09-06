//! Bounded structural fallback for direct files whose name is not enough to
//! select an existing media parser. This is deliberately a small registry of
//! parsers EmuWiz already owns, not a generic magic-number catalogue.

use super::content_registry::ContentKind;
use crate::commodore_tape::{
    COMMODORE_TAP_HEADER_BYTES, CommodoreTapeMachine, T64_READ_BYTES, parse_commodore_tap,
    parse_t64,
};
use crate::safe_read::{TrustedRoots, open_bounded_read};
use crate::tape_identity::{parse_tzx, parse_zx_tap};
use crate::zx_spectrum_snapshot::{parse_sna_snapshot, parse_szx_snapshot, parse_z80_snapshot};
use std::path::Path;

/// Whole-file parsers in this fallback never receive more than this many
/// bytes. Fixed-header probes remain bounded far below it.
pub const MAX_STRUCTURAL_PROBE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuralProbeConfidence {
    /// A complete, format-specific structure parsed successfully. This is
    /// about the media container, never a verified game or DAT identity.
    StrongStructural,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralProbe {
    pub format: &'static str,
    pub content: ContentKind,
    pub platform_hint: Option<&'static str>,
    pub confidence: StructuralProbeConfidence,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructuralProbeReport {
    pub matches: Vec<StructuralProbe>,
    /// A distinctive signature was present but its declared structure was
    /// invalid. Kept separate from an unrecognised random file.
    pub malformed: Vec<String>,
    pub refused_too_large: bool,
}

impl StructuralProbeReport {
    pub fn is_ambiguous(&self) -> bool {
        self.matches.len() > 1
    }
}

/// Opens one regular direct file through the shared read-only policy and runs
/// the fallback registry. A refusal is intentionally reported as no match:
/// discovery must not turn an unreadable path into an identity claim.
pub fn probe_unknown_file(path: &Path) -> StructuralProbeReport {
    let Ok(mut file) = open_bounded_read(path, &TrustedRoots::none()) else {
        return StructuralProbeReport::default();
    };
    let file_len = file.len();
    if file_len > MAX_STRUCTURAL_PROBE_BYTES {
        return StructuralProbeReport {
            refused_too_large: true,
            ..StructuralProbeReport::default()
        };
    }
    let Some(bytes) = file.read_exact_at(0, file_len as usize, MAX_STRUCTURAL_PROBE_BYTES as usize)
    else {
        return StructuralProbeReport::default();
    };
    probe_bytes(&bytes, file_len)
}

fn probe_bytes(bytes: &[u8], file_len: u64) -> StructuralProbeReport {
    let mut report = StructuralProbeReport::default();

    // Parsers with an unambiguous container signature are allowed to explain
    // malformed input. Parsers without magic (ZX TAP/Z80/SNA) only contribute
    // on full validation, so random bytes are never called malformed.
    if bytes.starts_with(b"ZXTape!\x1a") {
        match parse_tzx(bytes) {
            Ok(observation) => report.matches.push(StructuralProbe {
                format: "ZX Spectrum TZX tape",
                content: ContentKind::TapeImage,
                platform_hint: None,
                confidence: StructuralProbeConfidence::StrongStructural,
                evidence: format!(
                    "Header matches TZX; {} block(s) validated",
                    observation.blocks.len()
                ),
            }),
            Err(error) => report
                .malformed
                .push(format!("TZX signature present but {error}")),
        }
    }
    if bytes.starts_with(b"C64-TAPE-RAW") {
        match parse_commodore_tap(
            &bytes[..bytes.len().min(COMMODORE_TAP_HEADER_BYTES)],
            file_len,
        ) {
            Ok(observation) => report.matches.push(StructuralProbe {
                format: "Commodore TAP tape",
                content: ContentKind::TapeImage,
                platform_hint: match observation.machine {
                    CommodoreTapeMachine::C64 => Some("Commodore 64"),
                    CommodoreTapeMachine::Vic20 | CommodoreTapeMachine::C16Plus4 => None,
                },
                confidence: StructuralProbeConfidence::StrongStructural,
                evidence: format!(
                    "C64-TAPE-RAW header v{} validated for {}; pulse data was not decoded",
                    observation.version,
                    observation.machine.label()
                ),
            }),
            Err(error) => report
                .malformed
                .push(format!("Commodore TAP signature present but {error}")),
        }
    }
    if bytes.starts_with(b"C64S tape image file") {
        match parse_t64(&bytes[..bytes.len().min(T64_READ_BYTES)], file_len) {
            Ok(observation) => report.matches.push(StructuralProbe {
                format: "Commodore T64 tape",
                content: ContentKind::TapeImage,
                platform_hint: Some("Commodore 64"),
                confidence: StructuralProbeConfidence::StrongStructural,
                evidence: format!(
                    "T64 directory validated: {} active entr{}",
                    observation.entries.len(),
                    if observation.entries.len() == 1 {
                        "y"
                    } else {
                        "ies"
                    }
                ),
            }),
            Err(error) => report
                .malformed
                .push(format!("T64 signature present but {error}")),
        }
    }
    if bytes.starts_with(b"ZXST") {
        match parse_szx_snapshot(bytes) {
            Ok(facts) => report.matches.push(snapshot_probe(facts.format.label())),
            Err(error) => report
                .malformed
                .push(format!("ZXST signature present but {error}")),
        }
    }
    if let Ok(facts) = parse_zx_tap(bytes) {
        report.matches.push(StructuralProbe {
            format: "ZX Spectrum TAP tape",
            content: ContentKind::TapeImage,
            platform_hint: Some("ZX Spectrum"),
            confidence: StructuralProbeConfidence::StrongStructural,
            evidence: format!(
                "Block layout and XOR checksums match ZX Spectrum TAP ({} block(s))",
                facts.blocks.len()
            ),
        });
    }
    if let Ok(facts) = parse_z80_snapshot(bytes) {
        report.matches.push(snapshot_probe(facts.format.label()));
    }
    if let Ok(facts) = parse_sna_snapshot(bytes) {
        report.matches.push(snapshot_probe(facts.format.label()));
    }
    report
}

fn snapshot_probe(format: &'static str) -> StructuralProbe {
    StructuralProbe {
        format,
        content: ContentKind::MachineSnapshot,
        platform_hint: Some("ZX Spectrum"),
        confidence: StructuralProbeConfidence::StrongStructural,
        evidence: format!("Structure validates as {format}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_signed_tzx_is_not_silently_accepted() {
        let report = probe_bytes(b"ZXTape!\x1a\x01", 9);
        assert!(report.matches.is_empty());
        assert_eq!(report.malformed.len(), 1);
    }

    #[test]
    fn random_bytes_remain_unknown() {
        let report = probe_bytes(b"not retro media", 15);
        assert!(report.matches.is_empty());
        assert!(report.malformed.is_empty());
    }

    #[test]
    fn validated_commodore_tap_is_reported_from_its_header() {
        let mut bytes = Vec::from(&b"C64-TAPE-RAW"[..]);
        bytes.extend_from_slice(&[1, 0, 0, 0]);
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        let report = probe_bytes(&bytes, bytes.len() as u64);
        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.matches[0].format, "Commodore TAP tape");
        assert_eq!(report.matches[0].platform_hint, Some("Commodore 64"));
    }
}
