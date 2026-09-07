//! Small, format-neutral cheat representation and conservative conversion audit.
//!
//! This module is deliberately an analysis seam, not an installer.  Parsers and
//! existing native writers remain authoritative; operations which cannot be
//! proven to be direct writes are retained as opaque entries.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatPlatform {
    GameCube,
    Wii,
    Ps2,
    NintendoDs,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatSourceFormat {
    DolphinActionReplay,
    Gecko,
    Pnach,
    RetroArch,
    ActionReplayDs,
    GameSharkPs2,
    CodeBreakerPs2,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatOperation {
    Write8 {
        address: u64,
        value: u8,
    },
    Write16 {
        address: u64,
        value: u16,
    },
    Write32 {
        address: u64,
        value: u32,
    },
    UnsupportedRaw {
        source_format: CheatSourceFormat,
        raw: String,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatIssue {
    UnsupportedOperation(String),
    AmbiguousOperation(String),
    EncryptedVariant,
    MasterCodeRequired,
    PlatformMismatch,
    LossyMapping(String),
    UnknownWidth,
    RawPreserved,
    MissingTargetEncoder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatDocument {
    pub title: String,
    pub platform: CheatPlatform,
    pub source_format: CheatSourceFormat,
    pub operations: Vec<CheatOperation>,
    pub issues: Vec<CheatIssue>,
    pub provenance: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatTargetFormat {
    DolphinActionReplay,
    Gecko,
    Pnach,
    RetroArch,
    ActionReplayDs,
    GameSharkPs2,
    CodeBreakerPs2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversionCapability {
    Exact,
    Lossy { issues: Vec<CheatIssue> },
    Unsupported { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatConversionPreview {
    pub title: String,
    pub source_format: CheatSourceFormat,
    pub target_format: CheatTargetFormat,
    pub platform: CheatPlatform,
    pub operation_count: usize,
    pub exact_operations: usize,
    pub lossy_operations: usize,
    pub unsupported_operations: usize,
    pub warnings: Vec<CheatIssue>,
    pub output_preview: Option<String>,
    pub can_apply: bool,
    pub operation_status: Vec<OperationConversionStatus>,
    pub provenance: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationConversionStatus {
    Exact,
    Unsupported { reason: String },
}

fn parse_pair(raw: &str) -> Option<(u32, u32)> {
    let mut fields = raw.split_whitespace();
    let a = u32::from_str_radix(fields.next()?, 16).ok()?;
    let v = u32::from_str_radix(fields.next()?, 16).ok()?;
    (fields.next().is_none()).then_some((a, v))
}

/// Maps only direct-write Dolphin lines.  Control codes, encrypted forms and
/// malformed lines remain visible as `UnsupportedRaw`.
pub fn dolphin_line_to_ir(raw: &str, format: CheatSourceFormat) -> CheatOperation {
    let Some((word, value)) = parse_pair(raw) else {
        return CheatOperation::UnsupportedRaw {
            source_format: format,
            raw: raw.to_string(),
            reason: "malformed or non-direct-write code".into(),
        };
    };
    let prefix = word >> 24;
    let address = u64::from(word & 0x00ff_ffff);
    match prefix {
        0x00 => CheatOperation::Write8 {
            address,
            value: value as u8,
        },
        0x02 => CheatOperation::Write16 {
            address,
            value: value as u16,
        },
        0x04 => CheatOperation::Write32 { address, value },
        _ => CheatOperation::UnsupportedRaw {
            source_format: format,
            raw: raw.to_string(),
            reason: "control or format-specific operation".into(),
        },
    }
}

/// Converts a PNACH line when its width is one of the neutral IR widths.
pub fn pnach_line_to_ir(raw: &str) -> CheatOperation {
    let Some(fields) = raw
        .strip_prefix("patch=")
        .map(|s| s.split(',').map(str::trim).collect::<Vec<_>>())
    else {
        return CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::Pnach,
            raw: raw.into(),
            reason: "not a PNACH patch line".into(),
        };
    };
    if fields.len() != 5 {
        return CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::Pnach,
            raw: raw.into(),
            reason: "malformed PNACH line".into(),
        };
    }
    let Ok(address) = u64::from_str_radix(fields[2], 16) else {
        return CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::Pnach,
            raw: raw.into(),
            reason: "invalid PNACH address".into(),
        };
    };
    let Ok(value) = u32::from_str_radix(fields[4], 16) else {
        return CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::Pnach,
            raw: raw.into(),
            reason: "invalid PNACH value".into(),
        };
    };
    match fields[3] {
        "byte" => CheatOperation::Write8 {
            address,
            value: value as u8,
        },
        "short" => CheatOperation::Write16 {
            address,
            value: value as u16,
        },
        "word" => CheatOperation::Write32 { address, value },
        _ => CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::Pnach,
            raw: raw.into(),
            reason: "PNACH width is not representable by neutral IR".into(),
        },
    }
}

/// Encodes one neutral direct-write operation for a proven target grammar.
/// This is pure text generation; it never writes emulator files.
pub fn encode_operation(operation: &CheatOperation, target: &CheatTargetFormat) -> Option<String> {
    let (prefix, address, value) = match operation {
        CheatOperation::Write8 { address, value } => (0x00u8, *address, u32::from(*value)),
        CheatOperation::Write16 { address, value } => (0x02u8, *address, u32::from(*value)),
        CheatOperation::Write32 { address, value } => (0x04u8, *address, *value),
        CheatOperation::UnsupportedRaw { .. } => return None,
    };
    match target {
        CheatTargetFormat::DolphinActionReplay | CheatTargetFormat::Gecko => {
            if address > 0x00ff_ffff {
                return None;
            }
            Some(format!("{prefix:02X}{address:06X} {value:08X}"))
        }
        CheatTargetFormat::Pnach => {
            let width = match operation {
                CheatOperation::Write8 { .. } => "byte",
                CheatOperation::Write16 { .. } => "short",
                CheatOperation::Write32 { .. } => "word",
                CheatOperation::UnsupportedRaw { .. } => return None,
            };
            let rendered = match operation {
                CheatOperation::Write8 { value, .. } => format!("{value:02X}"),
                CheatOperation::Write16 { value, .. } => format!("{value:04X}"),
                CheatOperation::Write32 { value, .. } => format!("{value:08X}"),
                CheatOperation::UnsupportedRaw { .. } => return None,
            };
            Some(format!("patch=1,EE,{address:08X},{width},{rendered}"))
        }
        CheatTargetFormat::RetroArch
        | CheatTargetFormat::ActionReplayDs
        | CheatTargetFormat::GameSharkPs2
        | CheatTargetFormat::CodeBreakerPs2 => None,
    }
}

pub fn assess_document_conversion(
    document: &CheatDocument,
    target: CheatTargetFormat,
) -> CheatConversionPreview {
    let platform_ok = matches!(
        (&document.platform, &target),
        (
            CheatPlatform::GameCube | CheatPlatform::Wii,
            CheatTargetFormat::DolphinActionReplay | CheatTargetFormat::Gecko
        ) | (
            CheatPlatform::Ps2,
            CheatTargetFormat::Pnach
                | CheatTargetFormat::GameSharkPs2
                | CheatTargetFormat::CodeBreakerPs2
        ) | (
            CheatPlatform::NintendoDs,
            CheatTargetFormat::ActionReplayDs | CheatTargetFormat::RetroArch
        ) | (_, CheatTargetFormat::RetroArch)
    );
    let mut exact = 0;
    let mut unsupported = 0;
    let mut warnings = document.issues.clone();
    let mut operation_status = Vec::with_capacity(document.operations.len());
    let mut output = Vec::new();
    for op in &document.operations {
        if matches!(op, CheatOperation::UnsupportedRaw { .. })
            || encode_operation(op, &target).is_none()
        {
            unsupported += 1;
            let reason = match op {
                CheatOperation::UnsupportedRaw { reason, .. } => reason.clone(),
                _ => "target cannot represent this direct write".into(),
            };
            operation_status.push(OperationConversionStatus::Unsupported { reason });
        } else {
            exact += 1;
            operation_status.push(OperationConversionStatus::Exact);
            if let Some(line) = encode_operation(op, &target) {
                output.push(line);
            }
        }
    }
    if !platform_ok {
        warnings.push(CheatIssue::PlatformMismatch);
    }
    let missing_encoder = matches!(
        target,
        CheatTargetFormat::GameSharkPs2 | CheatTargetFormat::CodeBreakerPs2
    );
    if missing_encoder {
        warnings.push(CheatIssue::MissingTargetEncoder);
    }
    let can_apply = platform_ok && unsupported == 0 && !missing_encoder && warnings.is_empty();
    CheatConversionPreview {
        title: document.title.clone(),
        source_format: document.source_format.clone(),
        target_format: target,
        platform: document.platform.clone(),
        operation_count: document.operations.len(),
        exact_operations: exact,
        lossy_operations: 0,
        unsupported_operations: unsupported,
        warnings,
        output_preview: (!output.is_empty()).then(|| output.join("\n")),
        can_apply,
        operation_status,
        provenance: document.provenance.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dolphin_direct_writes_preserve_width() {
        assert!(matches!(
            dolphin_line_to_ir("042318AC 3B8003E7", CheatSourceFormat::Gecko),
            CheatOperation::Write32 { .. }
        ));
        assert!(matches!(
            dolphin_line_to_ir("0224CD50 00003E7F", CheatSourceFormat::DolphinActionReplay),
            CheatOperation::Write16 { .. }
        ));
    }
    #[test]
    fn pnach_widths_are_typed() {
        assert!(matches!(
            pnach_line_to_ir("patch=1,EE,20123456,byte,AB"),
            CheatOperation::Write8 { .. }
        ));
        assert!(matches!(
            pnach_line_to_ir("patch=1,EE,20123456,word,DEADBEEF"),
            CheatOperation::Write32 { .. }
        ));
    }
    #[test]
    fn unsupported_is_not_exact() {
        let d = CheatDocument {
            title: "x".into(),
            platform: CheatPlatform::Ps2,
            source_format: CheatSourceFormat::Pnach,
            operations: vec![CheatOperation::UnsupportedRaw {
                source_format: CheatSourceFormat::Pnach,
                raw: "x".into(),
                reason: "condition".into(),
            }],
            issues: vec![],
            provenance: vec![],
        };
        assert!(!assess_document_conversion(&d, CheatTargetFormat::Pnach).can_apply);
    }

    #[test]
    fn direct_writes_have_exact_target_previews() {
        let d = CheatDocument {
            title: "demo".into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::DolphinActionReplay,
            operations: vec![
                CheatOperation::Write8 {
                    address: 0x123456,
                    value: 0xab,
                },
                CheatOperation::Write16 {
                    address: 0x123458,
                    value: 0xcdef,
                },
                CheatOperation::Write32 {
                    address: 0x12345c,
                    value: 0x01234567,
                },
            ],
            issues: vec![],
            provenance: vec!["local:test".into()],
        };
        let preview = assess_document_conversion(&d, CheatTargetFormat::Gecko);
        assert!(preview.can_apply);
        assert_eq!(preview.exact_operations, 3);
        assert_eq!(preview.unsupported_operations, 0);
        assert_eq!(
            preview.output_preview.as_deref(),
            Some("00123456 000000AB\n02123458 0000CDEF\n0412345C 01234567")
        );
        assert_eq!(preview.provenance, d.provenance);
    }

    #[test]
    fn ps2_missing_encoder_is_explicit() {
        let d = CheatDocument {
            title: "ps2".into(),
            platform: CheatPlatform::Ps2,
            source_format: CheatSourceFormat::Pnach,
            operations: vec![CheatOperation::Write32 {
                address: 0x20123456,
                value: 1,
            }],
            issues: vec![],
            provenance: vec![],
        };
        let preview = assess_document_conversion(&d, CheatTargetFormat::GameSharkPs2);
        assert!(!preview.can_apply);
        assert!(preview.output_preview.is_none());
        assert!(preview.warnings.contains(&CheatIssue::MissingTargetEncoder));
    }
}
