//! Clean-room, local-only Action Replay/GameShark decoding foundation.
//!
//! This module implements a small interoperability subset. It recognises
//! structural code shapes, decodes direct-write records into the existing
//! neutral cheat IR, and retains unsupported records verbatim. It does not
//! ship a database, contact a provider, or attempt proprietary encrypted-code
//! recovery. Encrypted or format-ambiguous input is reported, not guessed.

use super::{
    CheatOperation, CheatSourceFormat, DsActionReplayClassification, ds_action_replay_line_to_ir,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatCodeFormat {
    GbaActionReplay,
    Ps2ActionReplay,
    GameCubeActionReplay,
    NintendoDsActionReplay,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatCodeEncoding {
    Raw,
    Encrypted,
    Decoded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatTargetPlatform {
    GameBoyAdvance,
    PlayStation2,
    GameCube,
    NintendoDs,
}

impl Default for CheatTargetPlatform {
    fn default() -> Self {
        Self::GameBoyAdvance
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatValidationIssue {
    Malformed {
        line: String,
        reason: String,
    },
    UnsupportedOpcode {
        line: String,
        reason: String,
    },
    AmbiguousFormat {
        candidates: Vec<CheatCodeFormat>,
    },
    EncryptedInput {
        reason: String,
    },
    PlatformMismatch {
        expected: CheatTargetPlatform,
        detected: CheatCodeFormat,
    },
    ChecksumUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatDecoderProvenance {
    pub method: String,
    pub reference_basis: String,
    pub local_only: bool,
    pub database_used: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatInstruction {
    pub original: String,
    pub operation: CheatOperation,
    pub opcode_family: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatCodeDecodeResult {
    pub format: CheatCodeFormat,
    pub encoding: CheatCodeEncoding,
    pub platform: CheatTargetPlatform,
    pub original: String,
    pub instructions: Vec<CheatInstruction>,
    pub issues: Vec<CheatValidationIssue>,
    pub provenance: CheatDecoderProvenance,
}

impl CheatCodeDecodeResult {
    #[must_use]
    pub fn status(&self) -> &'static str {
        if self.issues.iter().any(|issue| {
            matches!(
                issue,
                CheatValidationIssue::Malformed { .. }
                    | CheatValidationIssue::PlatformMismatch { .. }
                    | CheatValidationIssue::EncryptedInput { .. }
                    | CheatValidationIssue::AmbiguousFormat { .. }
            )
        }) {
            "Invalid or ambiguous"
        } else if self.instructions.iter().any(|instruction| {
            matches!(instruction.operation, CheatOperation::UnsupportedRaw { .. })
        }) {
            "Decoded with unsupported instructions"
        } else {
            "Decoded successfully"
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatDetection {
    pub candidates: Vec<CheatCodeFormat>,
    pub issues: Vec<CheatValidationIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatAdapterProjection {
    pub target: String,
    pub supported: bool,
    pub lines: Vec<String>,
    pub issues: Vec<CheatValidationIssue>,
}

fn lines(input: &str) -> Vec<String> {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.replace('-', " ").replace(':', " "))
        .collect()
}

fn words(line: &str) -> Vec<&str> {
    line.split_whitespace().collect()
}

fn is_hex_word(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len()) && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// Detects only shape-level evidence. A sixteen-digit pair is deliberately
/// ambiguous between console families until the caller supplies a target.
#[must_use]
pub fn detect_action_replay_format(input: &str) -> CheatDetection {
    let input_lines = lines(input);
    if input_lines.is_empty() {
        return CheatDetection {
            candidates: vec![CheatCodeFormat::Unknown],
            issues: vec![CheatValidationIssue::Malformed {
                line: String::new(),
                reason: "no code lines were supplied".into(),
            }],
        };
    }
    let mut eight = true;
    let mut sixteen = true;
    for line in &input_lines {
        let fields = words(line);
        eight &= fields.len() == 2 && is_hex_word(fields[0], &[8]) && is_hex_word(fields[1], &[4]);
        sixteen &=
            fields.len() == 2 && is_hex_word(fields[0], &[8]) && is_hex_word(fields[1], &[8]);
    }
    let candidates = if eight {
        vec![CheatCodeFormat::GbaActionReplay]
    } else if sixteen {
        vec![
            CheatCodeFormat::Ps2ActionReplay,
            CheatCodeFormat::GameCubeActionReplay,
            CheatCodeFormat::NintendoDsActionReplay,
        ]
    } else {
        vec![CheatCodeFormat::Unknown]
    };
    let issues = if candidates.len() > 1 {
        vec![CheatValidationIssue::AmbiguousFormat {
            candidates: candidates.clone(),
        }]
    } else if candidates == [CheatCodeFormat::Unknown] {
        vec![CheatValidationIssue::Malformed {
            line: input_lines.join(" | "),
            reason: "code shape is not a supported Action Replay record".into(),
        }]
    } else {
        Vec::new()
    };
    CheatDetection { candidates, issues }
}

fn provenance() -> CheatDecoderProvenance {
    CheatDecoderProvenance {
        method: "independent structural Action Replay interoperability decoder".into(),
        reference_basis: "public emulator/documentation behavior; independently implemented".into(),
        local_only: true,
        database_used: false,
    }
}

fn unsupported(format: CheatSourceFormat, raw: &str, reason: &str) -> CheatOperation {
    CheatOperation::UnsupportedRaw {
        source_format: format,
        raw: raw.into(),
        reason: reason.into(),
    }
}

fn direct_write(
    line: &str,
    format: CheatSourceFormat,
) -> Result<CheatOperation, CheatValidationIssue> {
    let fields = words(line);
    if fields.len() != 2 || !is_hex_word(fields[0], &[8]) || !is_hex_word(fields[1], &[4, 8]) {
        return Err(CheatValidationIssue::Malformed {
            line: line.into(),
            reason: "expected an eight-digit address and a four/eight-digit value".into(),
        });
    }
    let address =
        u64::from_str_radix(fields[0], 16).map_err(|_| CheatValidationIssue::Malformed {
            line: line.into(),
            reason: "address is not hexadecimal".into(),
        })?;
    let value =
        u32::from_str_radix(fields[1], 16).map_err(|_| CheatValidationIssue::Malformed {
            line: line.into(),
            reason: "value is not hexadecimal".into(),
        })?;
    if address == 0 || address > 0x0fff_ffff {
        return Ok(unsupported(
            format,
            line,
            "address is outside the conservative direct-write range",
        ));
    }
    Ok(match fields[1].len() {
        4 => CheatOperation::Write16 {
            address,
            value: value as u16,
        },
        8 => CheatOperation::Write32 { address, value },
        _ => unreachable!(),
    })
}

fn decode_lines(
    input: &str,
    format: CheatCodeFormat,
    platform: CheatTargetPlatform,
    parser: impl Fn(&str) -> Result<CheatOperation, CheatValidationIssue>,
) -> CheatCodeDecodeResult {
    let mut instructions = Vec::new();
    let mut issues = Vec::new();
    for line in lines(input) {
        match parser(&line) {
            Ok(operation) => instructions.push(CheatInstruction {
                original: line,
                opcode_family: format!("{format:?} direct-write subset"),
                operation,
            }),
            Err(issue) => issues.push(issue),
        }
    }
    CheatCodeDecodeResult {
        format,
        encoding: CheatCodeEncoding::Raw,
        platform,
        original: input.into(),
        instructions,
        issues,
        provenance: provenance(),
    }
}

/// Decodes the conservative raw direct-write subset for GBA Action Replay.
#[must_use]
pub fn decode_gba_action_replay(input: &str) -> CheatCodeDecodeResult {
    decode_lines(
        input,
        CheatCodeFormat::GbaActionReplay,
        CheatTargetPlatform::GameBoyAdvance,
        |line| {
            direct_write(
                line,
                CheatSourceFormat::Other("GBA Action Replay raw".into()),
            )
        },
    )
}

/// Decodes direct-write GameCube records using the existing Dolphin-compatible
/// opcode mapping. Control records remain visible as unsupported raw entries.
#[must_use]
pub fn decode_gamecube_action_replay(input: &str) -> CheatCodeDecodeResult {
    decode_lines(
        input,
        CheatCodeFormat::GameCubeActionReplay,
        CheatTargetPlatform::GameCube,
        |line| {
            Ok(super::dolphin_line_to_ir(
                line,
                CheatSourceFormat::DolphinActionReplay,
            ))
        },
    )
}

/// Decodes a conservative PS2 raw direct-write subset. Encrypted/MAX records
/// are not inferred from shape and must be supplied already decoded.
#[must_use]
pub fn decode_ps2_action_replay(input: &str) -> CheatCodeDecodeResult {
    decode_lines(
        input,
        CheatCodeFormat::Ps2ActionReplay,
        CheatTargetPlatform::PlayStation2,
        |line| direct_write(line, CheatSourceFormat::GameSharkPs2),
    )
}

/// Decodes canonical Nintendo DS raw Action Replay pairs, reusing the existing
/// family classifier so conditionals, pointers and master codes remain raw.
#[must_use]
pub fn decode_ds_action_replay(input: &str) -> CheatCodeDecodeResult {
    decode_lines(
        input,
        CheatCodeFormat::NintendoDsActionReplay,
        CheatTargetPlatform::NintendoDs,
        |line| match ds_action_replay_line_to_ir(line) {
            DsActionReplayClassification::DirectWrite(operation) => Ok(operation),
            DsActionReplayClassification::Unsupported(detail) => {
                Err(CheatValidationIssue::UnsupportedOpcode {
                    line: detail.raw,
                    reason: detail.reason,
                })
            }
        },
    )
}

/// Dispatches only when the caller has supplied an unambiguous target.
#[must_use]
pub fn decode_action_replay(input: &str, target: CheatTargetPlatform) -> CheatCodeDecodeResult {
    match target {
        CheatTargetPlatform::GameBoyAdvance => decode_gba_action_replay(input),
        CheatTargetPlatform::PlayStation2 => decode_ps2_action_replay(input),
        CheatTargetPlatform::GameCube => decode_gamecube_action_replay(input),
        CheatTargetPlatform::NintendoDs => decode_ds_action_replay(input),
    }
}

/// Records an encrypted/proprietary variant without attempting to recover or
/// infer its key material. This keeps the decode pipeline explicit for callers
/// that already know the source is encrypted.
#[must_use]
pub fn refuse_encrypted_action_replay(
    input: &str,
    format: CheatCodeFormat,
    platform: CheatTargetPlatform,
) -> CheatCodeDecodeResult {
    CheatCodeDecodeResult {
        format,
        encoding: CheatCodeEncoding::Encrypted,
        platform,
        original: input.into(),
        instructions: Vec::new(),
        issues: vec![CheatValidationIssue::EncryptedInput {
            reason: "encrypted/proprietary code recovery is outside this clean-room subset".into(),
        }],
        provenance: provenance(),
    }
}

#[must_use]
pub fn project_action_replay(
    result: &CheatCodeDecodeResult,
    target: &str,
) -> CheatAdapterProjection {
    let supported = result.issues.is_empty()
        && result
            .instructions
            .iter()
            .all(|entry| !matches!(entry.operation, CheatOperation::UnsupportedRaw { .. }));
    CheatAdapterProjection {
        target: target.into(),
        supported,
        lines: result
            .instructions
            .iter()
            .map(|entry| format!("{:?}", entry.operation))
            .collect(),
        issues: result.issues.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gba_direct_write_is_normalized_without_network_or_database() {
        let result = decode_gba_action_replay("02000000 1234");
        assert_eq!(result.status(), "Decoded successfully");
        assert_eq!(
            result.instructions[0].operation,
            CheatOperation::Write16 {
                address: 0x0200_0000,
                value: 0x1234
            }
        );
        assert!(result.provenance.local_only);
        assert!(!result.provenance.database_used);
    }

    #[test]
    fn gamecube_control_opcode_is_retained_not_reinterpreted() {
        let result = decode_gamecube_action_replay("06000000 00000001");
        assert!(matches!(
            result.instructions[0].operation,
            CheatOperation::UnsupportedRaw { .. }
        ));
    }

    #[test]
    fn ds_direct_and_conditional_records_have_distinct_outcomes() {
        let result = decode_ds_action_replay("20000010 0000007F\n30000000 00000001");
        assert_eq!(result.instructions.len(), 1);
        assert_eq!(result.issues.len(), 1);
        assert!(matches!(
            result.instructions[0].operation,
            CheatOperation::Write8 { .. }
        ));
    }

    #[test]
    fn sixteen_digit_shape_requires_platform_context() {
        let detection = detect_action_replay_format("02000000 0000007F");
        assert_eq!(detection.candidates.len(), 3);
        assert!(matches!(
            detection.issues[0],
            CheatValidationIssue::AmbiguousFormat { .. }
        ));
    }

    #[test]
    fn malformed_and_repeated_decode_are_deterministic() {
        let first = decode_ps2_action_replay("not-a-code");
        let second = decode_ps2_action_replay("not-a-code");
        assert_eq!(first, second);
        assert_eq!(first.status(), "Invalid or ambiguous");
    }

    #[test]
    fn encrypted_variant_is_refused_explicitly() {
        let result = refuse_encrypted_action_replay(
            "encrypted-vector",
            CheatCodeFormat::Ps2ActionReplay,
            CheatTargetPlatform::PlayStation2,
        );
        assert_eq!(result.encoding, CheatCodeEncoding::Encrypted);
        assert_eq!(result.status(), "Invalid or ambiguous");
        assert!(matches!(
            result.issues[0],
            CheatValidationIssue::EncryptedInput { .. }
        ));
    }
}
