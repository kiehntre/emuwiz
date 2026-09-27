//! Clean-room Nintendo 64 GameShark / Action Replay interoperability.
//!
//! This module deliberately implements a small, documented subset.  Direct
//! 8-bit and 16-bit writes are normalized; conditionals and master/enabler
//! records retain their native meaning and are never flattened into ordinary
//! writes.  Unknown, repeat, pointer, serial, and encrypted records remain
//! raw and preview-only.

use serde::{Deserialize, Serialize};

use super::{
    CheatCompatibilityEntry, CheatDocument, CheatIssue, CheatMasterCodeRequirement, CheatOperation,
    CheatPlatform, CheatRevisionEvidence, CheatSourceFormat,
};

const MAX_LINES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum N64CheatRegion {
    Japan,
    Usa,
    Europe,
    Australia,
    Other(u8),
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum N64CheatRevisionEvidence {
    ExactRomHash { hash: String },
    VerifiedIdentity { game_code: String, revision: u8 },
    HeaderIdentity { game_code: String, revision: u8 },
    RegionOnly(N64CheatRegion),
    TitleOnly(String),
    Unknown,
}

impl N64CheatRevisionEvidence {
    pub fn as_compatibility_evidence(&self) -> CheatRevisionEvidence {
        match self {
            Self::ExactRomHash { hash } => CheatRevisionEvidence::ExactHash { hash: hash.clone() },
            Self::VerifiedIdentity {
                game_code,
                revision,
            } => CheatRevisionEvidence::VerifiedIdentity {
                identity: game_code.clone(),
                revision: Some(revision.to_string()),
            },
            Self::HeaderIdentity {
                game_code,
                revision,
            } => CheatRevisionEvidence::ProviderDeclared {
                revision: format!("{game_code} rev {revision}"),
            },
            Self::RegionOnly(region) => CheatRevisionEvidence::ProviderDeclared {
                revision: format!("region:{region:?}"),
            },
            Self::TitleOnly(title) => CheatRevisionEvidence::TitleOnly {
                title: title.clone(),
            },
            Self::Unknown => CheatRevisionEvidence::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum N64CheatOpcode {
    Write8,
    Write16,
    ConditionalEqual16,
    ConditionalNotEqual16,
    MasterOrEnabler,
    RepeatOrSerial,
    PointerOrOffset,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum N64CheatIssue {
    Malformed {
        raw: String,
        reason: String,
    },
    UnsupportedOpcode {
        raw: String,
        reason: String,
    },
    EnablerRequired {
        code: String,
    },
    ConflictingEnablers {
        codes: Vec<String>,
    },
    WrongRegion {
        expected: N64CheatRegion,
        actual: N64CheatRegion,
    },
    RevisionUnverified,
    TitleOnlyIdentity,
    AmbiguousFormat,
    PlatformMismatch,
    NativeWriterUnavailable {
        target: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum N64NormalizedOperation {
    Write8 {
        address: u32,
        value: u8,
    },
    Write16 {
        address: u32,
        value: u16,
    },
    ConditionalEqual16 {
        address: u32,
        expected: u16,
        operation: Box<N64NormalizedOperation>,
    },
    ConditionalNotEqual16 {
        address: u32,
        expected: u16,
        operation: Box<N64NormalizedOperation>,
    },
    MasterOrEnabler {
        raw: String,
    },
    NativeRaw {
        raw: String,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum N64CheatReadiness {
    ReadyExactIdentity,
    ReadyVerifiedRevision,
    PreviewOnly,
    NotReady,
    Ambiguous,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum N64MasterCodeState {
    NotPresent,
    Present { code: String },
    Conflicting { codes: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct N64CheatCode {
    pub raw: String,
    pub opcode: N64CheatOpcode,
    pub normalized: N64NormalizedOperation,
    /// The existing neutral IR is populated only for unconditional direct
    /// writes. Conditional/master/native records stay explicit in `normalized`.
    pub ir_operation: Option<CheatOperation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct N64CheatDecodeResult {
    pub title: String,
    pub codes: Vec<N64CheatCode>,
    pub issues: Vec<N64CheatIssue>,
    pub readiness: N64CheatReadiness,
    pub region: N64CheatRegion,
    pub revision: N64CheatRevisionEvidence,
    pub required_master_codes: Vec<String>,
    pub master_code_state: N64MasterCodeState,
    pub provenance: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum N64ProjectionTarget {
    RetroArchMupen64PlusNext,
    Mupen64PlusStandalone,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct N64CheatProjection {
    pub target: N64ProjectionTarget,
    pub native_format: String,
    pub supported: bool,
    pub preview_only: bool,
    pub lines: Vec<String>,
    pub restart_required: bool,
    pub issues: Vec<N64CheatIssue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum N64RomByteOrder {
    Z64,
    N64,
    V64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct N64RomIdentity {
    pub byte_order: N64RomByteOrder,
    pub game_code: String,
    pub region: N64CheatRegion,
    pub revision: u8,
}

#[must_use]
pub fn inspect_n64_rom_header(bytes: &[u8]) -> Option<N64RomIdentity> {
    if bytes.len() < 0x40 {
        return None;
    }
    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    let (byte_order, canonical) = match magic {
        [0x80, 0x37, 0x12, 0x40] => (N64RomByteOrder::Z64, bytes.to_vec()),
        [0x40, 0x12, 0x37, 0x80] => (
            N64RomByteOrder::N64,
            bytes
                .chunks_exact(4)
                .flat_map(|c| c.iter().rev().copied())
                .collect(),
        ),
        [0x37, 0x80, 0x40, 0x12] => (
            N64RomByteOrder::V64,
            bytes
                .chunks_exact(2)
                .flat_map(|c| c.iter().rev().copied())
                .collect(),
        ),
        _ => return None,
    };
    let code = canonical.get(0x3b..0x3f)?;
    let game_code = String::from_utf8(code.to_vec()).ok()?.trim().to_string();
    Some(N64RomIdentity {
        byte_order,
        game_code,
        region: region_from_byte(*canonical.get(0x3e)?),
        revision: *canonical.get(0x3f)?,
    })
}

fn region_from_byte(byte: u8) -> N64CheatRegion {
    match byte.to_ascii_uppercase() {
        b'J' => N64CheatRegion::Japan,
        b'U' => N64CheatRegion::Usa,
        b'E' => N64CheatRegion::Europe,
        b'A' => N64CheatRegion::Australia,
        other => N64CheatRegion::Other(other),
    }
}

fn parse_words(raw: &str) -> Option<(u32, u32)> {
    let fields: Vec<_> = raw.split_whitespace().collect();
    if fields.len() != 2 || fields[0].len() != 8 || fields[1].len() != 4 {
        return None;
    }
    let address = u32::from_str_radix(fields[0], 16).ok()?;
    let value = u32::from_str_radix(fields[1], 16).ok()?;
    Some((address, value))
}

fn direct_code(raw: &str, address: u32, value: u32, width: u8) -> N64CheatCode {
    let (opcode, normalized, ir_operation) = if width == 1 {
        (
            N64CheatOpcode::Write8,
            N64NormalizedOperation::Write8 {
                address: address & 0x00ff_ffff,
                value: value as u8,
            },
            Some(CheatOperation::Write8 {
                address: u64::from(address & 0x00ff_ffff),
                value: value as u8,
            }),
        )
    } else {
        (
            N64CheatOpcode::Write16,
            N64NormalizedOperation::Write16 {
                address: address & 0x00ff_ffff,
                value: value as u16,
            },
            Some(CheatOperation::Write16 {
                address: u64::from(address & 0x00ff_ffff),
                value: value as u16,
            }),
        )
    };
    N64CheatCode {
        raw: raw.into(),
        opcode,
        normalized,
        ir_operation,
    }
}

fn decode_one(raw: &str) -> Result<N64CheatCode, N64CheatIssue> {
    let Some((address, value)) = parse_words(raw) else {
        return Err(N64CheatIssue::Malformed {
            raw: raw.into(),
            reason: "expected two hexadecimal words".into(),
        });
    };
    match address >> 24 {
        0x80 => Ok(direct_code(raw, address, value, 1)),
        0x81 => Ok(direct_code(raw, address, value, 2)),
        0xd0 => Ok(N64CheatCode {
            raw: raw.into(),
            opcode: N64CheatOpcode::ConditionalEqual16,
            normalized: N64NormalizedOperation::ConditionalEqual16 {
                address: address & 0x00ff_ffff,
                expected: value as u16,
                operation: Box::new(N64NormalizedOperation::NativeRaw {
                    raw: "next code".into(),
                    reason: "conditional body is ordered with the following native record".into(),
                }),
            },
            ir_operation: None,
        }),
        0xd1 => Ok(N64CheatCode {
            raw: raw.into(),
            opcode: N64CheatOpcode::ConditionalNotEqual16,
            normalized: N64NormalizedOperation::ConditionalNotEqual16 {
                address: address & 0x00ff_ffff,
                expected: value as u16,
                operation: Box::new(N64NormalizedOperation::NativeRaw {
                    raw: "next code".into(),
                    reason: "conditional body is ordered with the following native record".into(),
                }),
            },
            ir_operation: None,
        }),
        0xf0 | 0xf1 | 0xf2 | 0xf3 => Ok(N64CheatCode {
            raw: raw.into(),
            opcode: N64CheatOpcode::MasterOrEnabler,
            normalized: N64NormalizedOperation::MasterOrEnabler { raw: raw.into() },
            ir_operation: None,
        }),
        0x50..=0x5f => Ok(N64CheatCode {
            raw: raw.into(),
            opcode: N64CheatOpcode::RepeatOrSerial,
            normalized: N64NormalizedOperation::NativeRaw {
                raw: raw.into(),
                reason: "repeat/serial semantics are retained but not normalized".into(),
            },
            ir_operation: None,
        }),
        0x60..=0x7f => Ok(N64CheatCode {
            raw: raw.into(),
            opcode: N64CheatOpcode::PointerOrOffset,
            normalized: N64NormalizedOperation::NativeRaw {
                raw: raw.into(),
                reason: "pointer/offset semantics are retained but not normalized".into(),
            },
            ir_operation: None,
        }),
        _ => Ok(N64CheatCode {
            raw: raw.into(),
            opcode: N64CheatOpcode::Unsupported,
            normalized: N64NormalizedOperation::NativeRaw {
                raw: raw.into(),
                reason: "opcode is outside the proven direct-write subset".into(),
            },
            ir_operation: None,
        }),
    }
}

#[must_use]
pub fn decode_n64_gameshark(
    title: impl Into<String>,
    input: &str,
    revision: N64CheatRevisionEvidence,
    region: N64CheatRegion,
) -> N64CheatDecodeResult {
    let title = title.into();
    let mut codes = Vec::new();
    let mut issues = Vec::new();
    let mut required_master_codes = Vec::new();
    let lines: Vec<_> = input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() || lines.len() > MAX_LINES {
        issues.push(N64CheatIssue::Malformed {
            raw: input.into(),
            reason: "code list is empty or exceeds the bounded line limit".into(),
        });
    }
    for raw in lines.into_iter().take(MAX_LINES) {
        match decode_one(raw) {
            Ok(code) => {
                if code.opcode == N64CheatOpcode::MasterOrEnabler {
                    required_master_codes.push(code.raw.clone());
                }
                if matches!(
                    code.opcode,
                    N64CheatOpcode::Unsupported
                        | N64CheatOpcode::RepeatOrSerial
                        | N64CheatOpcode::PointerOrOffset
                ) {
                    issues.push(N64CheatIssue::UnsupportedOpcode {
                        raw: raw.into(),
                        reason: "native record retained without invented semantics".into(),
                    });
                }
                codes.push(code);
            }
            Err(issue) => issues.push(issue),
        }
    }
    if matches!(revision, N64CheatRevisionEvidence::TitleOnly(_)) {
        issues.push(N64CheatIssue::TitleOnlyIdentity);
    }
    let master_code_state = match required_master_codes.as_slice() {
        [] => N64MasterCodeState::NotPresent,
        [code] => N64MasterCodeState::Present { code: code.clone() },
        codes => {
            let codes = codes.to_vec();
            issues.push(N64CheatIssue::ConflictingEnablers {
                codes: codes.clone(),
            });
            N64MasterCodeState::Conflicting { codes }
        }
    };
    let readiness = if issues
        .iter()
        .any(|issue| matches!(issue, N64CheatIssue::Malformed { .. }))
    {
        N64CheatReadiness::NotReady
    } else if matches!(master_code_state, N64MasterCodeState::Conflicting { .. }) {
        N64CheatReadiness::Ambiguous
    } else if matches!(revision, N64CheatRevisionEvidence::ExactRomHash { .. }) {
        if issues.is_empty() {
            N64CheatReadiness::ReadyExactIdentity
        } else {
            N64CheatReadiness::PreviewOnly
        }
    } else if matches!(revision, N64CheatRevisionEvidence::VerifiedIdentity { .. }) {
        if issues.is_empty() {
            N64CheatReadiness::ReadyVerifiedRevision
        } else {
            N64CheatReadiness::PreviewOnly
        }
    } else if matches!(revision, N64CheatRevisionEvidence::TitleOnly(_)) {
        N64CheatReadiness::NotReady
    } else {
        N64CheatReadiness::Ambiguous
    };
    N64CheatDecodeResult { title, codes, issues, readiness, region, revision, required_master_codes, master_code_state, provenance: "independently implemented from public N64 GameShark documentation and emulator behavior; local-only, no database".into() }
}

impl N64CheatDecodeResult {
    pub fn to_document(&self) -> CheatDocument {
        CheatDocument {
            title: self.title.clone(),
            platform: CheatPlatform::Nintendo64,
            source_format: CheatSourceFormat::N64GameShark,
            operations: self
                .codes
                .iter()
                .filter_map(|code| code.ir_operation.clone())
                .collect(),
            issues: self
                .codes
                .iter()
                .filter(|code| code.ir_operation.is_none())
                .map(|code| CheatIssue::UnsupportedOperation(code.raw.clone()))
                .collect(),
            provenance: vec![self.provenance.clone()],
        }
    }

    pub fn compatibility_entry(
        &self,
        id: impl Into<String>,
        provider: impl Into<String>,
        source: impl Into<String>,
    ) -> CheatCompatibilityEntry {
        CheatCompatibilityEntry::from_document(
            id,
            &self.to_document(),
            provider,
            source,
            self.revision.as_compatibility_evidence(),
            if let Some(code) = self.required_master_codes.first() {
                CheatMasterCodeRequirement::Required { code: code.clone() }
            } else {
                CheatMasterCodeRequirement::None
            },
        )
    }
}

#[must_use]
pub fn project_n64_cheat(
    result: &N64CheatDecodeResult,
    target: N64ProjectionTarget,
) -> N64CheatProjection {
    let target_name = match target {
        N64ProjectionTarget::RetroArchMupen64PlusNext => "RetroArch Mupen64Plus-Next",
        N64ProjectionTarget::Mupen64PlusStandalone => "Mupen64Plus standalone",
    };
    N64CheatProjection {
        target,
        native_format: "N64 GameShark / Action Replay".into(),
        supported: false,
        preview_only: true,
        lines: result.codes.iter().map(|code| code.raw.clone()).collect(),
        restart_required: true,
        issues: vec![N64CheatIssue::NativeWriterUnavailable {
            target: target_name.into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact() -> N64CheatRevisionEvidence {
        N64CheatRevisionEvidence::ExactRomHash {
            hash: "a".repeat(64),
        }
    }

    #[test]
    fn decodes_direct_eight_and_sixteen_bit_writes() {
        let result = decode_n64_gameshark(
            "test",
            "80012345 00aa\n81012346 1234",
            exact(),
            N64CheatRegion::Usa,
        );
        assert_eq!(result.readiness, N64CheatReadiness::ReadyExactIdentity);
        assert!(matches!(
            result.codes[0].normalized,
            N64NormalizedOperation::Write8 {
                address: 0x012345,
                value: 0xaa
            }
        ));
        assert!(matches!(
            result.codes[1].ir_operation,
            Some(CheatOperation::Write16 {
                address: 0x012346,
                value: 0x1234
            })
        ));
    }

    #[test]
    fn preserves_conditionals_and_master_codes_without_flattening() {
        let result = decode_n64_gameshark(
            "test",
            "f1000318 2400\nd0012345 0001\n80012346 00ff",
            exact(),
            N64CheatRegion::Usa,
        );
        assert_eq!(result.required_master_codes, vec!["f1000318 2400"]);
        assert!(matches!(
            result.codes[1].opcode,
            N64CheatOpcode::ConditionalEqual16
        ));
        assert!(result.codes[1].ir_operation.is_none());
        assert!(result.issues.is_empty());
    }

    #[test]
    fn incompatible_master_codes_are_ambiguous() {
        let result = decode_n64_gameshark(
            "test",
            "f1000318 2400\nf1000320 2400\n80012346 00ff",
            exact(),
            N64CheatRegion::Usa,
        );
        assert_eq!(result.readiness, N64CheatReadiness::Ambiguous);
        assert!(matches!(
            result.master_code_state,
            N64MasterCodeState::Conflicting { .. }
        ));
        assert!(
            result
                .issues
                .iter()
                .any(|issue| matches!(issue, N64CheatIssue::ConflictingEnablers { .. }))
        );
    }

    #[test]
    fn unknown_and_malformed_records_are_retained_or_refused() {
        let result = decode_n64_gameshark(
            "test",
            "50012345 0001\nnot-code",
            exact(),
            N64CheatRegion::Usa,
        );
        assert_eq!(result.readiness, N64CheatReadiness::NotReady);
        assert!(
            result
                .issues
                .iter()
                .any(|issue| matches!(issue, N64CheatIssue::UnsupportedOpcode { .. }))
        );
        assert!(
            result
                .issues
                .iter()
                .any(|issue| matches!(issue, N64CheatIssue::Malformed { .. }))
        );
    }

    #[test]
    fn title_only_identity_is_not_apply_ready() {
        let result = decode_n64_gameshark(
            "GoldenEye",
            "80012345 00aa",
            N64CheatRevisionEvidence::TitleOnly("GoldenEye".into()),
            N64CheatRegion::Usa,
        );
        assert_eq!(result.readiness, N64CheatReadiness::NotReady);
        assert!(result.issues.contains(&N64CheatIssue::TitleOnlyIdentity));
    }

    #[test]
    fn byte_order_variants_share_header_identity() {
        let mut z64 = vec![0u8; 0x40];
        z64[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        z64[0x3b..0x3f].copy_from_slice(b"NXXU");
        z64[0x3f] = 1;
        let n64: Vec<_> = z64
            .chunks_exact(4)
            .flat_map(|c| c.iter().rev().copied())
            .collect();
        let v64: Vec<_> = z64
            .chunks_exact(2)
            .flat_map(|c| c.iter().rev().copied())
            .collect();
        let a = inspect_n64_rom_header(&z64).unwrap();
        let b = inspect_n64_rom_header(&n64).unwrap();
        let c = inspect_n64_rom_header(&v64).unwrap();
        assert_eq!(a.game_code, b.game_code);
        assert_eq!(a.game_code, c.game_code);
        assert_eq!((a.region, a.revision), (N64CheatRegion::Usa, 1));
    }

    #[test]
    fn projection_is_explicitly_preview_only_without_native_writer() {
        let result = decode_n64_gameshark("test", "80012345 00aa", exact(), N64CheatRegion::Usa);
        let projection = project_n64_cheat(&result, N64ProjectionTarget::RetroArchMupen64PlusNext);
        assert!(projection.preview_only);
        assert!(!projection.supported);
    }
}
