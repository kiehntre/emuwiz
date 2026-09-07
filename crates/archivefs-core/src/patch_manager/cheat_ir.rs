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
    DolphinOnFrame,
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
    OnFrameWrite8 {
        address: u64,
        value: u8,
    },
    OnFrameWrite16 {
        address: u64,
        value: u16,
    },
    OnFrameWrite32 {
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
    DsActionReplayUnsupported(DsActionReplayUnsupportedKind),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DsActionReplayUnsupportedKind {
    Malformed,
    NonCanonical,
    Misaligned,
    HookOrSpecial,
    Conditional,
    Activator,
    Pointer,
    LoopOrMultiWrite,
    CopyFill,
    OffsetRegister,
    MasterInit,
    EncryptedOrUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsActionReplayUnsupported {
    pub kind: DsActionReplayUnsupportedKind,
    pub raw: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DsActionReplayClassification {
    DirectWrite(CheatOperation),
    Unsupported(DsActionReplayUnsupported),
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
    DolphinOnFrame,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCapability {
    pub target: CheatTargetFormat,
    pub capability: ConversionCapability,
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

fn parse_on_frame_hex(raw: &str) -> Option<u32> {
    let value = raw.trim();
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    (!value.is_empty() && value.len() <= 8)
        .then(|| u32::from_str_radix(value, 16).ok())
        .flatten()
}

/// Maps only unconditional Dolphin OnFrame memory writes. OnFrame entries are
/// applied on Dolphin's frame patch cycle, so they use distinct IR variants
/// rather than losing their execution policy as ordinary writes.
pub fn dolphin_on_frame_line_to_ir(raw: &str) -> CheatOperation {
    let mut normalized = raw.trim().to_string();
    if let Some(index) = normalized.find('=') {
        normalized.replace_range(index..=index, ":");
    }
    let fields: Vec<_> = normalized.split(':').map(str::trim).collect();
    if fields.len() != 3 {
        return CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::DolphinOnFrame,
            raw: raw.to_string(),
            reason: "malformed or conditional OnFrame patch".into(),
        };
    }
    let Some(address) = parse_on_frame_hex(fields[0]) else {
        return CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::DolphinOnFrame,
            raw: raw.to_string(),
            reason: "OnFrame address is not a valid 32-bit hexadecimal value".into(),
        };
    };
    let Some(value) = parse_on_frame_hex(fields[2]) else {
        return CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::DolphinOnFrame,
            raw: raw.to_string(),
            reason: "OnFrame value is not a valid 32-bit hexadecimal value".into(),
        };
    };
    match fields[1].to_ascii_lowercase().as_str() {
        "byte" if value <= u32::from(u8::MAX) => CheatOperation::OnFrameWrite8 {
            address: u64::from(address),
            value: value as u8,
        },
        "word" if value <= u32::from(u16::MAX) => CheatOperation::OnFrameWrite16 {
            address: u64::from(address),
            value: value as u16,
        },
        "dword" => CheatOperation::OnFrameWrite32 {
            address: u64::from(address),
            value,
        },
        "byte" | "word" => CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::DolphinOnFrame,
            raw: raw.to_string(),
            reason: "OnFrame value exceeds the declared write width".into(),
        },
        _ => CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::DolphinOnFrame,
            raw: raw.to_string(),
            reason: "unsupported or conditional OnFrame patch type".into(),
        },
    }
}

/// Encodes one neutral direct-write operation as canonical Nintendo DS Action
/// Replay text. This is pure in-memory formatting; it never writes an
/// emulator file.
pub fn encode_ds_action_replay_operation(operation: &CheatOperation) -> Option<String> {
    match operation {
        CheatOperation::Write32 { address, value }
            if *address <= 0x0FFF_FFFF && *address != 0 && address % 4 == 0 =>
        {
            Some(format!("0{address:07X} {value:08X}"))
        }
        CheatOperation::Write16 { address, value }
            if *address <= 0x0FFF_FFFF && address % 2 == 0 =>
        {
            Some(format!("1{address:07X} 0000{value:04X}"))
        }
        CheatOperation::Write8 { address, value } if *address <= 0x0FFF_FFFF => {
            Some(format!("2{address:07X} 000000{value:02X}"))
        }
        CheatOperation::Write8 { .. }
        | CheatOperation::Write16 { .. }
        | CheatOperation::Write32 { .. }
        | CheatOperation::OnFrameWrite8 { .. }
        | CheatOperation::OnFrameWrite16 { .. }
        | CheatOperation::OnFrameWrite32 { .. }
        | CheatOperation::UnsupportedRaw { .. } => None,
    }
}

fn ds_action_replay_unsupported(
    raw: &str,
    kind: DsActionReplayUnsupportedKind,
    reason: impl Into<String>,
) -> DsActionReplayClassification {
    DsActionReplayClassification::Unsupported(DsActionReplayUnsupported {
        kind,
        raw: raw.to_string(),
        reason: reason.into(),
    })
}

/// Classifies one canonical Nintendo DS Action Replay pair.
///
/// Only the three constant-write families are projected into the neutral IR.
/// The complete record must be direct-only because the real AR interpreter has
/// mutable offset/data/condition state.  Everything else remains raw and
/// carries a typed refusal reason.
pub fn ds_action_replay_line_to_ir(raw: &str) -> DsActionReplayClassification {
    let fields: Vec<_> = raw.split_whitespace().collect();
    if fields.len() != 2 {
        return ds_action_replay_unsupported(
            raw,
            DsActionReplayUnsupportedKind::Malformed,
            "a DS Action Replay pair must contain exactly two hexadecimal words",
        );
    }
    let Ok(first) = u32::from_str_radix(fields[0], 16) else {
        return ds_action_replay_unsupported(
            raw,
            DsActionReplayUnsupportedKind::Malformed,
            "the DS Action Replay address/opcode word is not hexadecimal",
        );
    };
    let Ok(value) = u32::from_str_radix(fields[1], 16) else {
        return ds_action_replay_unsupported(
            raw,
            DsActionReplayUnsupportedKind::Malformed,
            "the DS Action Replay value word is not hexadecimal",
        );
    };
    if fields[0].len() != 8 || fields[1].len() != 8 {
        return ds_action_replay_unsupported(
            raw,
            DsActionReplayUnsupportedKind::Malformed,
            "DS Action Replay words must be exactly eight hexadecimal digits",
        );
    }

    let family = first >> 28;
    let address = u64::from(first & 0x0FFF_FFFF);
    let direct = match family {
        0 => {
            if first == 0 {
                return ds_action_replay_unsupported(
                    raw,
                    DsActionReplayUnsupportedKind::HookOrSpecial,
                    "00000000 is the Action Replay manual-hook special case",
                );
            }
            if address % 4 != 0 {
                return ds_action_replay_unsupported(
                    raw,
                    DsActionReplayUnsupportedKind::Misaligned,
                    "a 32-bit DS Action Replay write must be four-byte aligned",
                );
            }
            Some(CheatOperation::Write32 { address, value })
        }
        1 => {
            if value > 0xFFFF {
                return ds_action_replay_unsupported(
                    raw,
                    DsActionReplayUnsupportedKind::NonCanonical,
                    "a 16-bit DS Action Replay write must have zero unused value bits",
                );
            }
            if address % 2 != 0 {
                return ds_action_replay_unsupported(
                    raw,
                    DsActionReplayUnsupportedKind::Misaligned,
                    "a 16-bit DS Action Replay write must be two-byte aligned",
                );
            }
            Some(CheatOperation::Write16 {
                address,
                value: value as u16,
            })
        }
        2 => {
            if value > 0xFF {
                return ds_action_replay_unsupported(
                    raw,
                    DsActionReplayUnsupportedKind::NonCanonical,
                    "an 8-bit DS Action Replay write must have zero unused value bits",
                );
            }
            Some(CheatOperation::Write8 {
                address,
                value: value as u8,
            })
        }
        3..=6 => {
            return ds_action_replay_unsupported(
                raw,
                DsActionReplayUnsupportedKind::Conditional,
                "conditional comparison families are not representable by a direct write",
            );
        }
        7..=10 => {
            return ds_action_replay_unsupported(
                raw,
                DsActionReplayUnsupportedKind::Conditional,
                "masked conditional families are not representable by a direct write",
            );
        }
        11 => {
            return ds_action_replay_unsupported(
                raw,
                DsActionReplayUnsupportedKind::Pointer,
                "the B family loads a mutable offset from memory",
            );
        }
        12 => {
            return ds_action_replay_unsupported(
                raw,
                DsActionReplayUnsupportedKind::LoopOrMultiWrite,
                "C-family trainer and loop operations are stateful",
            );
        }
        13 => {
            let subtype = (first >> 24) & 0xFF;
            let (kind, reason) = match subtype {
                0xD0..=0xD2 => (
                    DsActionReplayUnsupportedKind::LoopOrMultiWrite,
                    "D0-D2 terminator and loop operations are stateful",
                ),
                0xD3 | 0xDC => (
                    DsActionReplayUnsupportedKind::OffsetRegister,
                    "D3/DC mutate the Action Replay offset register",
                ),
                0xD4..=0xDB => (
                    DsActionReplayUnsupportedKind::OffsetRegister,
                    "D4-DB use mutable data/offset state",
                ),
                0xDF => (
                    DsActionReplayUnsupportedKind::MasterInit,
                    "DF changes the emulated processor context",
                ),
                _ => (
                    DsActionReplayUnsupportedKind::EncryptedOrUnknown,
                    "unknown D-family operation",
                ),
            };
            return ds_action_replay_unsupported(raw, kind, reason);
        }
        14 | 15 => {
            return ds_action_replay_unsupported(
                raw,
                DsActionReplayUnsupportedKind::CopyFill,
                "E/F families copy or fill memory and are not direct writes",
            );
        }
        _ => {
            return ds_action_replay_unsupported(
                raw,
                DsActionReplayUnsupportedKind::EncryptedOrUnknown,
                "unknown or encrypted DS Action Replay opcode family",
            );
        }
    };
    DsActionReplayClassification::DirectWrite(direct.expect("direct family has an operation"))
}

/// Parses a bounded, line-oriented DS Action Replay document into the
/// existing neutral IR. Unsupported lines remain in-order as `UnsupportedRaw`
/// operations and add a typed issue; they are never dropped.
pub fn parse_ds_action_replay_document(
    title: impl Into<String>,
    text: &str,
    provenance: Vec<String>,
) -> CheatDocument {
    let mut operations = Vec::new();
    let mut issues = Vec::new();
    for raw in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        match ds_action_replay_line_to_ir(raw) {
            DsActionReplayClassification::DirectWrite(operation) => operations.push(operation),
            DsActionReplayClassification::Unsupported(unsupported) => {
                issues.push(CheatIssue::DsActionReplayUnsupported(
                    unsupported.kind.clone(),
                ));
                operations.push(CheatOperation::UnsupportedRaw {
                    source_format: CheatSourceFormat::ActionReplayDs,
                    raw: unsupported.raw,
                    reason: unsupported.reason,
                });
            }
        }
    }
    CheatDocument {
        title: title.into(),
        platform: CheatPlatform::NintendoDs,
        source_format: CheatSourceFormat::ActionReplayDs,
        operations,
        issues,
        provenance,
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
    if matches!(target, CheatTargetFormat::DolphinOnFrame) {
        return encode_on_frame_operation(operation);
    }
    let (prefix, address, value) = match operation {
        CheatOperation::Write8 { address, value } => (0x00u8, *address, u32::from(*value)),
        CheatOperation::Write16 { address, value } => (0x02u8, *address, u32::from(*value)),
        CheatOperation::Write32 { address, value } => (0x04u8, *address, *value),
        CheatOperation::OnFrameWrite8 { .. }
        | CheatOperation::OnFrameWrite16 { .. }
        | CheatOperation::OnFrameWrite32 { .. } => return None,
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
                CheatOperation::OnFrameWrite8 { .. }
                | CheatOperation::OnFrameWrite16 { .. }
                | CheatOperation::OnFrameWrite32 { .. } => return None,
                CheatOperation::UnsupportedRaw { .. } => return None,
            };
            let rendered = match operation {
                CheatOperation::Write8 { value, .. } => format!("{value:02X}"),
                CheatOperation::Write16 { value, .. } => format!("{value:04X}"),
                CheatOperation::Write32 { value, .. } => format!("{value:08X}"),
                CheatOperation::OnFrameWrite8 { .. }
                | CheatOperation::OnFrameWrite16 { .. }
                | CheatOperation::OnFrameWrite32 { .. } => return None,
                CheatOperation::UnsupportedRaw { .. } => return None,
            };
            Some(format!("patch=1,EE,{address:08X},{width},{rendered}"))
        }
        CheatTargetFormat::ActionReplayDs => encode_ds_action_replay_operation(operation),
        CheatTargetFormat::DolphinOnFrame => unreachable!("handled before target dispatch"),
        CheatTargetFormat::RetroArch
        | CheatTargetFormat::GameSharkPs2
        | CheatTargetFormat::CodeBreakerPs2 => None,
    }
}

fn encode_on_frame_operation(operation: &CheatOperation) -> Option<String> {
    let (address, kind, value) = match operation {
        CheatOperation::OnFrameWrite8 { address, value } => (*address, "byte", u32::from(*value)),
        CheatOperation::OnFrameWrite16 { address, value } => (*address, "word", u32::from(*value)),
        CheatOperation::OnFrameWrite32 { address, value } => (*address, "dword", *value),
        CheatOperation::Write8 { .. }
        | CheatOperation::Write16 { .. }
        | CheatOperation::Write32 { .. } => return None,
        CheatOperation::UnsupportedRaw { .. } => return None,
    };
    (address <= u64::from(u32::MAX)).then(|| format!("0x{address:08X}:{kind}:0x{value:08X}"))
}

pub fn assess_document_conversion(
    document: &CheatDocument,
    target: CheatTargetFormat,
) -> CheatConversionPreview {
    let platform_ok = matches!(
        (&document.platform, &target),
        (
            CheatPlatform::GameCube | CheatPlatform::Wii,
            CheatTargetFormat::DolphinActionReplay
                | CheatTargetFormat::Gecko
                | CheatTargetFormat::DolphinOnFrame
        ) | (
            CheatPlatform::Ps2,
            CheatTargetFormat::Pnach
                | CheatTargetFormat::GameSharkPs2
                | CheatTargetFormat::CodeBreakerPs2
        ) | (
            CheatPlatform::NintendoDs,
            CheatTargetFormat::ActionReplayDs | CheatTargetFormat::RetroArch
        )
    );
    let mut exact = 0;
    let mut unsupported = 0;
    let mut warnings = document.issues.clone();
    let mut operation_status = Vec::with_capacity(document.operations.len());
    let mut output = Vec::new();
    for op in &document.operations {
        if matches!(op, CheatOperation::UnsupportedRaw { .. }) {
            unsupported += 1;
            let reason = match op {
                CheatOperation::UnsupportedRaw { reason, .. } => reason.clone(),
                _ => "target cannot represent this direct write".into(),
            };
            operation_status.push(OperationConversionStatus::Unsupported { reason });
        } else if let Some(line) = encode_operation(op, &target) {
            exact += 1;
            operation_status.push(OperationConversionStatus::Exact);
            output.push(line);
        } else {
            unsupported += 1;
            operation_status.push(OperationConversionStatus::Unsupported {
                reason: "operation or execution policy is not representable by the target".into(),
            });
        }
    }
    if !platform_ok {
        warnings.push(CheatIssue::PlatformMismatch);
    }
    let missing_encoder = matches!(
        target,
        CheatTargetFormat::GameSharkPs2
            | CheatTargetFormat::CodeBreakerPs2
            | CheatTargetFormat::RetroArch
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
        output_preview: (can_apply && !output.is_empty()).then(|| output.join("\n")),
        can_apply,
        operation_status,
        provenance: document.provenance.clone(),
    }
}

/// Stable service seam for callers such as the GUI; it deliberately delegates
/// to the pure assessor so parser and installer internals stay out of UI code.
pub fn convert_cheat_document(
    document: &CheatDocument,
    target: CheatTargetFormat,
) -> CheatConversionPreview {
    assess_document_conversion(document, target)
}

/// Returns a complete, pure-text conversion only when the requested target is
/// fully supported. Mixed or otherwise non-applicable documents never expose a
/// partial export.
pub fn export_conversion_preview(
    document: &CheatDocument,
    target: CheatTargetFormat,
) -> Option<String> {
    convert_cheat_document(document, target).output_preview
}

/// Enumerates targets without inventing encoders.  Unsupported entries are
/// still returned so a GUI can explain why a target is unavailable.
pub fn supported_targets_for(document: &CheatDocument) -> Vec<TargetCapability> {
    let targets = match &document.platform {
        CheatPlatform::GameCube | CheatPlatform::Wii => vec![
            CheatTargetFormat::DolphinActionReplay,
            CheatTargetFormat::Gecko,
            CheatTargetFormat::DolphinOnFrame,
        ],
        CheatPlatform::Ps2 => vec![
            CheatTargetFormat::Pnach,
            CheatTargetFormat::GameSharkPs2,
            CheatTargetFormat::CodeBreakerPs2,
        ],
        CheatPlatform::NintendoDs => vec![
            CheatTargetFormat::ActionReplayDs,
            CheatTargetFormat::RetroArch,
        ],
        CheatPlatform::Other(_) => Vec::new(),
    };
    targets
        .into_iter()
        .map(|target| {
            let preview = assess_document_conversion(document, target.clone());
            let capability = if preview.can_apply {
                ConversionCapability::Exact
            } else if preview.unsupported_operations > 0 {
                ConversionCapability::Unsupported {
                    reason: "one or more operations are not safely representable".into(),
                }
            } else {
                ConversionCapability::Unsupported {
                    reason: preview
                        .warnings
                        .first()
                        .map_or_else(|| "target unavailable".into(), |issue| format!("{issue:?}")),
                }
            };
            TargetCapability { target, capability }
        })
        .collect()
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

    #[test]
    fn service_lists_only_platform_targets_and_preserves_mixed_status() {
        let d = CheatDocument {
            title: "mixed".into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::Gecko,
            operations: vec![
                CheatOperation::Write8 {
                    address: 1,
                    value: 2,
                },
                CheatOperation::UnsupportedRaw {
                    source_format: CheatSourceFormat::Gecko,
                    raw: "conditional".into(),
                    reason: "conditional operation".into(),
                },
            ],
            issues: vec![],
            provenance: vec!["fixture".into()],
        };
        let preview = convert_cheat_document(&d, CheatTargetFormat::Gecko);
        assert_eq!(preview.operation_count, 2);
        assert_eq!(preview.exact_operations, 1);
        assert_eq!(preview.unsupported_operations, 1);
        assert!(!preview.can_apply);
        assert_eq!(supported_targets_for(&d).len(), 3);
    }

    #[test]
    fn ds_action_replay_direct_write_families_are_typed() {
        assert_eq!(
            ds_action_replay_line_to_ir("02345678 DEADBEEF"),
            DsActionReplayClassification::DirectWrite(CheatOperation::Write32 {
                address: 0x0234_5678,
                value: 0xDEAD_BEEF,
            })
        );
        assert_eq!(
            ds_action_replay_line_to_ir("12345678 0000BEEF"),
            DsActionReplayClassification::DirectWrite(CheatOperation::Write16 {
                address: 0x0234_5678,
                value: 0xBEEF,
            })
        );
        assert_eq!(
            ds_action_replay_line_to_ir("22345679 000000EF"),
            DsActionReplayClassification::DirectWrite(CheatOperation::Write8 {
                address: 0x0234_5679,
                value: 0xEF,
            })
        );
    }

    #[test]
    fn ds_action_replay_encoder_emits_canonical_fixed_width_words() {
        assert_eq!(
            encode_ds_action_replay_operation(&CheatOperation::Write32 {
                address: 0x0234_5678,
                value: 0xDEAD_BEEF,
            }),
            Some("02345678 DEADBEEF".into())
        );
        assert_eq!(
            encode_ds_action_replay_operation(&CheatOperation::Write16 {
                address: 0x0234_5678,
                value: 0xBEEF,
            }),
            Some("12345678 0000BEEF".into())
        );
        assert_eq!(
            encode_ds_action_replay_operation(&CheatOperation::Write8 {
                address: 0x0234_5679,
                value: 0xEF,
            }),
            Some("22345679 000000EF".into())
        );
    }

    #[test]
    fn ds_action_replay_encoder_refuses_invalid_or_opaque_operations() {
        for operation in [
            CheatOperation::Write32 {
                address: 0x0234_5679,
                value: 1,
            },
            CheatOperation::Write16 {
                address: 0x0234_5679,
                value: 1,
            },
            CheatOperation::Write32 {
                address: 0x1000_0000,
                value: 1,
            },
            CheatOperation::Write8 {
                address: 0x1000_0000,
                value: 1,
            },
            CheatOperation::UnsupportedRaw {
                source_format: CheatSourceFormat::ActionReplayDs,
                raw: "conditional".into(),
                reason: "stateful".into(),
            },
        ] {
            assert_eq!(encode_ds_action_replay_operation(&operation), None);
        }
    }

    #[test]
    fn ds_action_replay_rejects_malformed_noncanonical_and_misaligned_lines() {
        let cases = [
            ("not-hex 00000000", DsActionReplayUnsupportedKind::Malformed),
            ("02345678", DsActionReplayUnsupportedKind::Malformed),
            (
                "12345678 1000BEEF",
                DsActionReplayUnsupportedKind::NonCanonical,
            ),
            (
                "22345679 000001EF",
                DsActionReplayUnsupportedKind::NonCanonical,
            ),
            (
                "02345679 DEADBEEF",
                DsActionReplayUnsupportedKind::Misaligned,
            ),
            (
                "12345679 0000BEEF",
                DsActionReplayUnsupportedKind::Misaligned,
            ),
            (
                "00000000 12345678",
                DsActionReplayUnsupportedKind::HookOrSpecial,
            ),
        ];
        for (raw, expected) in cases {
            let DsActionReplayClassification::Unsupported(result) =
                ds_action_replay_line_to_ir(raw)
            else {
                panic!("{raw} unexpectedly parsed as direct write");
            };
            assert_eq!(result.kind, expected, "{raw}");
        }
    }

    #[test]
    fn ds_action_replay_classifies_stateful_families_without_guessing() {
        let cases = [
            (
                "32345678 00000001",
                DsActionReplayUnsupportedKind::Conditional,
            ),
            ("B2345678 00000000", DsActionReplayUnsupportedKind::Pointer),
            (
                "C0000000 00000001",
                DsActionReplayUnsupportedKind::LoopOrMultiWrite,
            ),
            (
                "D3000000 00000010",
                DsActionReplayUnsupportedKind::OffsetRegister,
            ),
            ("E2345678 00000004", DsActionReplayUnsupportedKind::CopyFill),
            ("F2345678 00000004", DsActionReplayUnsupportedKind::CopyFill),
            (
                "DFFFFFFF 99999999",
                DsActionReplayUnsupportedKind::MasterInit,
            ),
            (
                "A2345678 0000FFFF",
                DsActionReplayUnsupportedKind::Conditional,
            ),
            (
                "F2345678 00000004 extra",
                DsActionReplayUnsupportedKind::Malformed,
            ),
        ];
        for (raw, expected) in cases {
            let DsActionReplayClassification::Unsupported(result) =
                ds_action_replay_line_to_ir(raw)
            else {
                panic!("{raw} unexpectedly parsed as direct write");
            };
            assert_eq!(result.kind, expected, "{raw}");
            assert_eq!(result.raw, raw);
        }
    }

    #[test]
    fn ds_action_replay_document_preserves_mixed_operations_and_identity() {
        let document = parse_ds_action_replay_document(
            "Mario DS",
            "02345678 DEADBEEF\n12345678 0000BEEF\n32345678 00000001\n22345679 000000EF",
            vec!["CheatBase:rom-42".into()],
        );
        assert_eq!(document.title, "Mario DS");
        assert_eq!(document.platform, CheatPlatform::NintendoDs);
        assert_eq!(document.source_format, CheatSourceFormat::ActionReplayDs);
        assert_eq!(document.provenance, vec!["CheatBase:rom-42".to_string()]);
        assert_eq!(document.operations.len(), 4);
        assert_eq!(
            document
                .operations
                .iter()
                .filter(|operation| !matches!(operation, CheatOperation::UnsupportedRaw { .. }))
                .count(),
            3
        );
        assert!(matches!(
            &document.operations[2],
            CheatOperation::UnsupportedRaw {
                source_format: CheatSourceFormat::ActionReplayDs,
                raw,
                ..
            } if raw == "32345678 00000001"
        ));
        assert_eq!(document.issues.len(), 1);

        let preview = convert_cheat_document(&document, CheatTargetFormat::ActionReplayDs);
        assert_eq!(preview.exact_operations, 3);
        assert_eq!(preview.unsupported_operations, 1);
        assert!(!preview.can_apply);
        assert!(!preview.warnings.contains(&CheatIssue::MissingTargetEncoder));
        assert_eq!(preview.provenance, document.provenance);
    }

    #[test]
    fn ds_action_replay_service_does_not_offer_cross_platform_writers() {
        let document =
            parse_ds_action_replay_document("DS", "02345678 DEADBEEF", vec!["fixture".into()]);
        let capabilities = supported_targets_for(&document);
        assert_eq!(capabilities.len(), 2);
        assert!(matches!(
            capabilities
                .iter()
                .find(|capability| capability.target == CheatTargetFormat::ActionReplayDs)
                .map(|capability| &capability.capability),
            Some(ConversionCapability::Exact)
        ));
        assert!(matches!(
            capabilities
                .iter()
                .find(|capability| capability.target == CheatTargetFormat::RetroArch)
                .map(|capability| &capability.capability),
            Some(ConversionCapability::Unsupported { .. })
        ));
        let dolphin = convert_cheat_document(&document, CheatTargetFormat::Gecko);
        assert!(!dolphin.can_apply);
        assert!(dolphin.warnings.contains(&CheatIssue::PlatformMismatch));
    }

    #[test]
    fn ds_action_replay_round_trip_and_export_are_complete_only() {
        let document = parse_ds_action_replay_document(
            "DS",
            "02345678 DEADBEEF\n12345678 0000BEEF\n22345679 000000EF",
            vec!["CheatBase:rom-42".into()],
        );
        let preview = convert_cheat_document(&document, CheatTargetFormat::ActionReplayDs);
        assert!(preview.can_apply);
        assert_eq!(preview.exact_operations, 3);
        assert_eq!(preview.unsupported_operations, 0);
        assert_eq!(
            export_conversion_preview(&document, CheatTargetFormat::ActionReplayDs).as_deref(),
            Some("02345678 DEADBEEF\n12345678 0000BEEF\n22345679 000000EF")
        );

        let mixed = parse_ds_action_replay_document(
            "DS",
            "02345678 DEADBEEF\n32345678 00000001\n22345679 000000EF",
            vec!["fixture".into()],
        );
        let mixed_preview = convert_cheat_document(&mixed, CheatTargetFormat::ActionReplayDs);
        assert_eq!(mixed_preview.exact_operations, 2);
        assert_eq!(mixed_preview.unsupported_operations, 1);
        assert!(!mixed_preview.can_apply);
        assert_eq!(mixed_preview.output_preview, None);
        assert_eq!(
            export_conversion_preview(&mixed, CheatTargetFormat::ActionReplayDs),
            None
        );
        assert_eq!(mixed.provenance, vec!["fixture".to_string()]);
    }

    #[test]
    fn ds_action_replay_target_rejects_non_ds_platform_documents() {
        let document = CheatDocument {
            title: "PS2".into(),
            platform: CheatPlatform::Ps2,
            source_format: CheatSourceFormat::Pnach,
            operations: vec![CheatOperation::Write32 {
                address: 0x0234_5678,
                value: 1,
            }],
            issues: vec![],
            provenance: vec![],
        };
        let preview = convert_cheat_document(&document, CheatTargetFormat::ActionReplayDs);
        assert!(!preview.can_apply);
        assert!(preview.warnings.contains(&CheatIssue::PlatformMismatch));
        assert_eq!(
            export_conversion_preview(&document, CheatTargetFormat::ActionReplayDs),
            None
        );
    }

    #[test]
    fn dolphin_on_frame_direct_writes_preserve_width_and_timing() {
        assert_eq!(
            dolphin_on_frame_line_to_ir("0x80001234:byte:0xAB"),
            CheatOperation::OnFrameWrite8 {
                address: 0x8000_1234,
                value: 0xAB,
            }
        );
        assert_eq!(
            dolphin_on_frame_line_to_ir("80001234:word:0000BEEF"),
            CheatOperation::OnFrameWrite16 {
                address: 0x8000_1234,
                value: 0xBEEF,
            }
        );
        assert_eq!(
            dolphin_on_frame_line_to_ir("80001234:dword:DEADBEEF"),
            CheatOperation::OnFrameWrite32 {
                address: 0x8000_1234,
                value: 0xDEAD_BEEF,
            }
        );
    }

    #[test]
    fn dolphin_on_frame_rejects_conditional_and_truncating_forms() {
        for raw in [
            "80001234:byte:0xAB:0xCD",
            "80001234:byte:0x1FF",
            "80001234:word:0x1_0000",
            "80001234:float:0x3F800000",
            "not-hex:dword:1",
        ] {
            assert!(matches!(
                dolphin_on_frame_line_to_ir(raw),
                CheatOperation::UnsupportedRaw {
                    source_format: CheatSourceFormat::DolphinOnFrame,
                    ..
                }
            ));
        }
    }

    #[test]
    fn dolphin_on_frame_encoder_is_canonical_and_same_platform() {
        let operation = CheatOperation::OnFrameWrite32 {
            address: 0x8000_1234,
            value: 0xDEAD_BEEF,
        };
        assert_eq!(
            encode_operation(&operation, &CheatTargetFormat::DolphinOnFrame),
            Some("0x80001234:dword:0xDEADBEEF".into())
        );
        assert_eq!(
            encode_operation(&operation, &CheatTargetFormat::Gecko),
            None
        );
    }

    #[test]
    fn dolphin_on_frame_document_requires_complete_supported_output() {
        let document = CheatDocument {
            title: "frame patch".into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::DolphinOnFrame,
            operations: vec![
                CheatOperation::OnFrameWrite8 {
                    address: 0x8000_1234,
                    value: 1,
                },
                CheatOperation::UnsupportedRaw {
                    source_format: CheatSourceFormat::DolphinOnFrame,
                    raw: "80001234:byte:1:2".into(),
                    reason: "conditional OnFrame patch".into(),
                },
            ],
            issues: vec![],
            provenance: vec!["fixture".into()],
        };
        let preview = convert_cheat_document(&document, CheatTargetFormat::DolphinOnFrame);
        assert_eq!(preview.exact_operations, 1);
        assert_eq!(preview.unsupported_operations, 1);
        assert!(!preview.can_apply);
        assert!(preview.output_preview.is_none());
        assert_eq!(preview.provenance, document.provenance);
    }

    #[test]
    fn dolphin_direct_write_conversion_to_on_frame_is_not_assumed() {
        let document = CheatDocument {
            title: "direct".into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::Gecko,
            operations: vec![CheatOperation::Write16 {
                address: 0x8000_1234,
                value: 0xBEEF,
            }],
            issues: vec![],
            provenance: vec![],
        };
        let preview = convert_cheat_document(&document, CheatTargetFormat::DolphinOnFrame);
        assert!(!preview.can_apply);
        assert_eq!(preview.output_preview, None);
        assert!(supported_targets_for(&document)
            .iter()
            .any(|capability| capability.target == CheatTargetFormat::DolphinOnFrame));
    }
}
