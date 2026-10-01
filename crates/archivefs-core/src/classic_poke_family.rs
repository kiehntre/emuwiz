//! Neutral, bounded POKE/direct-memory cheat data for classic microcomputers.
//!
//! This is an interchange and preview model, not a machine-memory writer.
//! Addresses are never considered equivalent across memory spaces or banks.
//! Platform-specific native writers remain emulator adapters' responsibility.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::open_retro_cheat_providers::{ZxPokOperation, ZxPokTrainer};
use crate::patch_manager::CheatApplicabilityMatch;

pub const POKE_MAX_TITLE_BYTES: usize = 256;
pub const POKE_MAX_LINES: usize = 4096;
pub const POKE_MAX_OPERATIONS: usize = 512;
pub const POKE_MAX_EXPRESSION_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PokePlatform {
    ZxSpectrum,
    AmstradCpc,
    Commodore64,
    AtariSt,
    Msx,
    BbcMicro,
    Acorn8Bit,
}

impl PokePlatform {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ZxSpectrum => "ZX Spectrum",
            Self::AmstradCpc => "Amstrad CPC",
            Self::Commodore64 => "Commodore 64",
            Self::AtariSt => "Atari ST",
            Self::Msx => "MSX",
            Self::BbcMicro => "BBC Micro",
            Self::Acorn8Bit => "Acorn 8-bit",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum PokeMemorySpace {
    MainRam,
    BankedRam,
    VideoRam,
    Io,
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum PokeBank {
    Unspecified,
    Number(u16),
    Page(u16),
    SlotPage { slot: u8, page: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PokeAddress(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PokeValue {
    pub value: u32,
    pub width_bits: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PokeOriginalValue {
    pub value: u32,
    pub width_bits: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokeCondition {
    OriginalEquals(PokeOriginalValue),
    Native(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokeOperation {
    pub memory_space: PokeMemorySpace,
    pub bank: PokeBank,
    pub address: PokeAddress,
    pub value: PokeValue,
    pub original_value: Option<PokeOriginalValue>,
    pub condition: Option<PokeCondition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokeCheat {
    pub title: String,
    pub platform: PokePlatform,
    pub operations: Vec<PokeOperation>,
    pub target_identity: Option<String>,
    pub target_verified: bool,
    pub source: String,
    pub provenance: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokePlatformSemantics {
    pub platform: PokePlatform,
    pub address_bits: u8,
    pub value_bits: u8,
    pub default_memory_space: PokeMemorySpace,
    pub banking_proven: bool,
    pub banking_description: String,
}

pub fn poke_platform_semantics(platform: PokePlatform) -> PokePlatformSemantics {
    match platform {
        PokePlatform::ZxSpectrum => PokePlatformSemantics {
            platform,
            address_bits: 16,
            value_bits: 8,
            default_memory_space: PokeMemorySpace::BankedRam,
            banking_proven: true,
            banking_description: "128K .pok bank 0-7 or current mapping 8".into(),
        },
        PokePlatform::AmstradCpc => PokePlatformSemantics {
            platform,
            address_bits: 16,
            value_bits: 8,
            default_memory_space: PokeMemorySpace::MainRam,
            banking_proven: false,
            banking_description:
                "simple 16-bit POKE only; banked CPC variants require a proven native format".into(),
        },
        PokePlatform::Commodore64 => PokePlatformSemantics {
            platform,
            address_bits: 16,
            value_bits: 8,
            default_memory_space: PokeMemorySpace::MainRam,
            banking_proven: false,
            banking_description:
                "simple CPU address space; VICE memory banks are not inferred from a BASIC POKE"
                    .into(),
        },
        PokePlatform::AtariSt => PokePlatformSemantics {
            platform,
            address_bits: 24,
            value_bits: 8,
            default_memory_space: PokeMemorySpace::MainRam,
            banking_proven: false,
            banking_description:
                "manual 24-bit memory entry; no stable bundled trainer interchange".into(),
        },
        PokePlatform::Msx => PokePlatformSemantics {
            platform,
            address_bits: 16,
            value_bits: 8,
            default_memory_space: PokeMemorySpace::BankedRam,
            banking_proven: true,
            banking_description:
                "mapper slot/page must be explicit; simple address alone is not a bank claim".into(),
        },
        PokePlatform::BbcMicro | PokePlatform::Acorn8Bit => PokePlatformSemantics {
            platform,
            address_bits: 16,
            value_bits: 8,
            default_memory_space: PokeMemorySpace::MainRam,
            banking_proven: false,
            banking_description:
                "manual/import model only; machine and memory map must be supplied by the user"
                    .into(),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokeIssue {
    EmptyTitle,
    TooManyLines,
    TooManyOperations,
    InvalidSyntax {
        line: usize,
        detail: String,
    },
    InvalidAddress {
        value: u32,
        maximum: u32,
    },
    ValueOverflow {
        value: u32,
        width_bits: u8,
    },
    OriginalValueOverflow {
        value: u32,
        width_bits: u8,
    },
    UnsupportedBanking {
        platform: PokePlatform,
    },
    PlatformMismatch {
        expected: PokePlatform,
        actual: PokePlatform,
    },
    IdentityRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokeExpressionResult {
    ParsedDirectWrite(PokeOperation),
    ParsedWriteSequence(Vec<PokeOperation>),
    UnsupportedExpression(String),
    AmbiguousExpression(String),
    UnsafeExpression(String),
}

impl fmt::Display for PokeIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PokeParseError(pub PokeIssue);
impl fmt::Display for PokeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for PokeParseError {}

fn validate_title(title: &str) -> Result<(), PokeParseError> {
    if title.trim().is_empty() {
        return Err(PokeParseError(PokeIssue::EmptyTitle));
    }
    if title.len() > POKE_MAX_TITLE_BYTES {
        return Err(PokeParseError(PokeIssue::InvalidSyntax {
            line: 0,
            detail: "title is too long".into(),
        }));
    }
    Ok(())
}

fn parse_number(raw: &str) -> Option<u32> {
    let raw = raw.trim();
    if let Some(value) = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .or_else(|| raw.strip_prefix('$'))
    {
        u32::from_str_radix(value, 16).ok()
    } else {
        raw.parse().ok()
    }
}

fn checked_value(value: u32, width_bits: u8, original: bool) -> Result<PokeValue, PokeParseError> {
    if !matches!(width_bits, 8 | 16 | 32) {
        return Err(PokeParseError(PokeIssue::InvalidSyntax {
            line: 0,
            detail: "width must be 8, 16 or 32 bits".into(),
        }));
    }
    let maximum = if width_bits == 32 {
        u32::MAX
    } else {
        (1u32 << width_bits) - 1
    };
    if value > maximum {
        return Err(PokeParseError(if original {
            PokeIssue::OriginalValueOverflow { value, width_bits }
        } else {
            PokeIssue::ValueOverflow { value, width_bits }
        }));
    }
    Ok(PokeValue { value, width_bits })
}

pub fn manual_poke(
    platform: PokePlatform,
    title: &str,
    address: u32,
    value: u32,
    original_value: Option<u32>,
    bank: PokeBank,
    memory_space: Option<PokeMemorySpace>,
    target_identity: Option<String>,
    target_verified: bool,
) -> Result<PokeCheat, PokeParseError> {
    validate_title(title)?;
    let semantics = poke_platform_semantics(platform);
    let maximum_address = (1u32 << semantics.address_bits.min(31)) - 1;
    if address > maximum_address {
        return Err(PokeParseError(PokeIssue::InvalidAddress {
            value: address,
            maximum: maximum_address,
        }));
    }
    if !semantics.banking_proven && !matches!(bank, PokeBank::Unspecified) {
        return Err(PokeParseError(PokeIssue::UnsupportedBanking { platform }));
    }
    let value = checked_value(value, semantics.value_bits, false)?;
    let original = original_value
        .map(|raw| checked_value(raw, semantics.value_bits, true))
        .transpose()?
        .map(|v| PokeOriginalValue {
            value: v.value,
            width_bits: v.width_bits,
        });
    let condition = original.map(PokeCondition::OriginalEquals);
    Ok(PokeCheat {
        title: title.into(),
        platform,
        operations: vec![PokeOperation {
            memory_space: memory_space.unwrap_or(semantics.default_memory_space),
            bank,
            address: PokeAddress(address),
            value,
            original_value: original,
            condition,
        }],
        target_identity,
        target_verified,
        source: "manual".into(),
        provenance: vec!["User-entered POKE; no media mutation".into()],
    })
}

/// Parse only the deliberately small, non-executable `POKE address,value[,original]`
/// notation. BASIC programs, expressions and trainer scripts are refused.
pub fn parse_simple_pokes(
    platform: PokePlatform,
    title: &str,
    input: &str,
) -> Result<PokeCheat, PokeParseError> {
    validate_title(title)?;
    let mut operations = Vec::new();
    for (index, raw) in input.lines().enumerate() {
        if index >= POKE_MAX_LINES {
            return Err(PokeParseError(PokeIssue::TooManyLines));
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        for segment in line.split(':') {
            let operation = parse_poke_segment(platform, title, segment, index + 1)?;
            operations.push(operation);
        }
        if operations.len() > POKE_MAX_OPERATIONS {
            return Err(PokeParseError(PokeIssue::TooManyOperations));
        }
    }
    if operations.is_empty() {
        return Err(PokeParseError(PokeIssue::InvalidSyntax {
            line: 0,
            detail: "no POKE operations".into(),
        }));
    }
    Ok(PokeCheat {
        title: title.into(),
        platform,
        operations,
        target_identity: None,
        target_verified: false,
        source: "local manual/import".into(),
        provenance: vec!["Local parse only; no network and no media mutation".into()],
    })
}

fn parse_poke_segment(
    platform: PokePlatform,
    title: &str,
    segment: &str,
    line: usize,
) -> Result<PokeOperation, PokeParseError> {
    let body = segment
        .trim()
        .strip_prefix("POKE")
        .or_else(|| segment.trim().strip_prefix("poke"))
        .ok_or_else(|| {
            PokeParseError(PokeIssue::InvalidSyntax {
                line,
                detail: "expected POKE address,value[,original]".into(),
            })
        })?
        .trim();
    let values: Vec<_> = body.split(',').map(str::trim).collect();
    if !(2..=3).contains(&values.len()) {
        return Err(PokeParseError(PokeIssue::InvalidSyntax {
            line,
            detail: "expected two or three comma-separated values".into(),
        }));
    }
    let address = parse_number(values[0]).ok_or_else(|| {
        PokeParseError(PokeIssue::InvalidSyntax {
            line,
            detail: "invalid address".into(),
        })
    })?;
    let value = parse_number(values[1]).ok_or_else(|| {
        PokeParseError(PokeIssue::InvalidSyntax {
            line,
            detail: "invalid value".into(),
        })
    })?;
    let original = values
        .get(2)
        .map(|v| {
            parse_number(v).ok_or_else(|| {
                PokeParseError(PokeIssue::InvalidSyntax {
                    line,
                    detail: "invalid original value".into(),
                })
            })
        })
        .transpose()?;
    Ok(manual_poke(
        platform,
        title,
        address,
        value,
        original,
        PokeBank::Unspecified,
        None,
        None,
        false,
    )?
    .operations
    .remove(0))
}

pub fn parse_poke_expression(
    platform: PokePlatform,
    title: &str,
    expression: &str,
) -> PokeExpressionResult {
    if expression.len() > POKE_MAX_EXPRESSION_BYTES {
        return PokeExpressionResult::UnsafeExpression(
            "expression exceeds the bounded input limit".into(),
        );
    }
    let upper = expression.to_ascii_uppercase();
    for token in [
        "FOR",
        "NEXT",
        "DATA",
        "READ",
        "SYS",
        "CALL",
        "USR",
        "RANDOMIZE",
    ] {
        if upper
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|part| part == token)
        {
            return PokeExpressionResult::UnsafeExpression(format!(
                "BASIC token {token} is not executable or reducible"
            ));
        }
    }
    if expression.contains('=') || expression.contains(';') {
        return PokeExpressionResult::UnsupportedExpression(expression.into());
    }
    match parse_simple_pokes(platform, title, expression) {
        Ok(cheat) if cheat.operations.len() == 1 => PokeExpressionResult::ParsedDirectWrite(
            cheat.operations.into_iter().next().expect("one operation"),
        ),
        Ok(cheat) => PokeExpressionResult::ParsedWriteSequence(cheat.operations),
        Err(error) => PokeExpressionResult::AmbiguousExpression(error.to_string()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokeRuntimeSupport {
    SupportedNativeRuntime,
    SupportedGeneratedScript,
    SupportedMonitorCommand,
    PreviewOnly,
    Unsupported,
    UnknownUnproven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokeRuntimeEmulator {
    Fuse,
    Vice,
    Hatari,
    Caprice32,
    OpenMsx,
    BeebEm,
    BEm,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokeRuntimeCapability {
    pub emulator: PokeRuntimeEmulator,
    pub platform: PokePlatform,
    pub support: PokeRuntimeSupport,
    pub preserves_banking: bool,
    pub preserves_original_guards: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokeProjectionRefusal {
    WrongPlatform,
    UnsupportedRuntime,
    BankingUnproven,
    MemorySpaceUnproven,
    WidthUnsupported,
    OriginalGuardUnsupported,
    InvalidAddress,
    IdentityNotEligible,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokeRuntimeProjection {
    pub capability: PokeRuntimeCapability,
    pub commands_or_files: Vec<String>,
    pub preview: String,
    pub refusal: Option<PokeProjectionRefusal>,
}

pub fn poke_runtime_capability(emulator: PokeRuntimeEmulator) -> PokeRuntimeCapability {
    match emulator {
        PokeRuntimeEmulator::Fuse => PokeRuntimeCapability { emulator, platform: PokePlatform::ZxSpectrum, support: PokeRuntimeSupport::SupportedNativeRuntime, preserves_banking: true, preserves_original_guards: true, detail: "Fuse multiface .pok files document bank, address, value and original value".into() },
        PokeRuntimeEmulator::Vice => PokeRuntimeCapability { emulator, platform: PokePlatform::Commodore64, support: PokeRuntimeSupport::SupportedMonitorCommand, preserves_banking: false, preserves_original_guards: false, detail: "VICE monitor documents the > memory-write command; this adapter only projects explicit unbanked byte writes".into() },
        PokeRuntimeEmulator::Hatari => PokeRuntimeCapability { emulator, platform: PokePlatform::AtariSt, support: PokeRuntimeSupport::PreviewOnly, preserves_banking: false, preserves_original_guards: false, detail: "Hatari documents memwrite and --parse, but exact scripted command syntax and guard semantics are not proven here".into() },
        PokeRuntimeEmulator::Caprice32 => PokeRuntimeCapability { emulator, platform: PokePlatform::AmstradCpc, support: PokeRuntimeSupport::PreviewOnly, preserves_banking: false, preserves_original_guards: false, detail: "Caprice32 documents --autocmd but not a stable memory-write command contract".into() },
        PokeRuntimeEmulator::OpenMsx => PokeRuntimeCapability { emulator, platform: PokePlatform::Msx, support: PokeRuntimeSupport::PreviewOnly, preserves_banking: false, preserves_original_guards: false, detail: "openMSX documents interactive poke/poke16 and trainer tooling; mapper projection is not proven".into() },
        PokeRuntimeEmulator::BeebEm | PokeRuntimeEmulator::BEm => PokeRuntimeCapability { emulator, platform: PokePlatform::BbcMicro, support: PokeRuntimeSupport::PreviewOnly, preserves_banking: false, preserves_original_guards: false, detail: "debugger memory inspection is documented, but a safe scripted memory-write path is not proven".into() },
    }
}

pub fn project_poke_runtime(
    cheat: &PokeCheat,
    emulator: PokeRuntimeEmulator,
) -> PokeRuntimeProjection {
    let capability = poke_runtime_capability(emulator);
    if !cheat.target_verified
        || cheat
            .target_identity
            .as_deref()
            .is_none_or(|id| id.trim().is_empty())
    {
        return PokeRuntimeProjection {
            capability,
            commands_or_files: Vec::new(),
            preview: "Runtime projection blocked: verified game/release identity is required."
                .into(),
            refusal: Some(PokeProjectionRefusal::IdentityNotEligible),
        };
    }
    if cheat.platform != capability.platform {
        return PokeRuntimeProjection {
            capability,
            commands_or_files: Vec::new(),
            preview: "Runtime projection blocked: platform does not match the emulator.".into(),
            refusal: Some(PokeProjectionRefusal::WrongPlatform),
        };
    }
    match emulator {
        PokeRuntimeEmulator::Fuse => project_fuse_pok(cheat, capability),
        PokeRuntimeEmulator::Vice => project_vice_monitor(cheat, capability),
        _ => PokeRuntimeProjection { capability, commands_or_files: Vec::new(), preview: "Preview only: this emulator's documented path is insufficient for exact projection.".into(), refusal: Some(PokeProjectionRefusal::UnsupportedRuntime) },
    }
}

fn project_fuse_pok(cheat: &PokeCheat, capability: PokeRuntimeCapability) -> PokeRuntimeProjection {
    let mut lines = vec![format!("N{}", cheat.title)];
    for (index, operation) in cheat.operations.iter().enumerate() {
        if operation.memory_space != PokeMemorySpace::BankedRam
            || operation.value.width_bits != 8
            || operation.original_value.is_none()
        {
            return PokeRuntimeProjection { capability, commands_or_files: Vec::new(), preview: "Fuse projection blocked: every write must be an 8-bit banked write with an original-value field.".into(), refusal: Some(if operation.memory_space != PokeMemorySpace::BankedRam { PokeProjectionRefusal::MemorySpaceUnproven } else if operation.value.width_bits != 8 { PokeProjectionRefusal::WidthUnsupported } else { PokeProjectionRefusal::OriginalGuardUnsupported }) };
        }
        let PokeBank::Number(bank) = operation.bank else {
            return PokeRuntimeProjection {
                capability,
                commands_or_files: Vec::new(),
                preview: "Fuse projection blocked: bank 0-8 must be explicit.".into(),
                refusal: Some(PokeProjectionRefusal::BankingUnproven),
            };
        };
        if bank > 8
            || operation.address.0 > u16::MAX as u32
            || operation.value.value > u8::MAX as u32
        {
            return PokeRuntimeProjection {
                capability,
                commands_or_files: Vec::new(),
                preview:
                    "Fuse projection blocked: address, bank or value is outside documented limits."
                        .into(),
                refusal: Some(PokeProjectionRefusal::InvalidAddress),
            };
        }
        let original = operation.original_value.expect("checked above").value;
        let kind = if index + 1 == cheat.operations.len() {
            'Z'
        } else {
            'M'
        };
        lines.push(format!(
            "{kind} {bank} {} {} {original}",
            operation.address.0, operation.value.value
        ));
    }
    lines.push("Y".into());
    let file = lines.join("\n") + "\n";
    PokeRuntimeProjection {
        capability,
        commands_or_files: vec![file.clone()],
        preview: file,
        refusal: None,
    }
}

fn project_vice_monitor(
    cheat: &PokeCheat,
    capability: PokeRuntimeCapability,
) -> PokeRuntimeProjection {
    let mut commands = Vec::new();
    for operation in &cheat.operations {
        if operation.memory_space != PokeMemorySpace::MainRam
            || !matches!(operation.bank, PokeBank::Unspecified)
        {
            return PokeRuntimeProjection { capability, commands_or_files: Vec::new(), preview: "VICE projection blocked: only explicit unbanked main-memory writes are representable.".into(), refusal: Some(if operation.memory_space != PokeMemorySpace::MainRam { PokeProjectionRefusal::MemorySpaceUnproven } else { PokeProjectionRefusal::BankingUnproven }) };
        }
        if operation.value.width_bits != 8 {
            return PokeRuntimeProjection {
                capability,
                commands_or_files: Vec::new(),
                preview: "VICE projection blocked: only byte writes are currently emitted.".into(),
                refusal: Some(PokeProjectionRefusal::WidthUnsupported),
            };
        }
        if operation.original_value.is_some() {
            return PokeRuntimeProjection { capability, commands_or_files: Vec::new(), preview: "VICE projection blocked: monitor command projection cannot preserve an original-value guard.".into(), refusal: Some(PokeProjectionRefusal::OriginalGuardUnsupported) };
        }
        commands.push(format!(
            "> ${:04x} {:02x}",
            operation.address.0, operation.value.value
        ));
    }
    PokeRuntimeProjection {
        capability,
        preview: commands.join("\n") + "\n",
        commands_or_files: commands,
        refusal: None,
    }
}

pub fn normalize_zx_pok_family(trainer: &ZxPokTrainer) -> PokeCheat {
    let operations = trainer
        .operations
        .iter()
        .map(|operation: &ZxPokOperation| PokeOperation {
            memory_space: PokeMemorySpace::BankedRam,
            bank: PokeBank::Number(operation.bank as u16),
            address: PokeAddress(operation.address as u32),
            value: PokeValue {
                value: operation.value as u32,
                width_bits: 8,
            },
            original_value: operation.original_value.map(|value| PokeOriginalValue {
                value: value as u32,
                width_bits: 8,
            }),
            condition: operation.original_value.map(|value| {
                PokeCondition::OriginalEquals(PokeOriginalValue {
                    value: value as u32,
                    width_bits: 8,
                })
            }),
        })
        .collect();
    PokeCheat {
        title: trainer.title.clone(),
        platform: PokePlatform::ZxSpectrum,
        operations,
        target_identity: None,
        target_verified: false,
        source: "ZX .pok".into(),
        provenance: vec!["Existing ZX .pok parser; bank and original-value guard retained".into()],
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokeConflictKind {
    SameLocationDifferentValue,
    OriginalGuardMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokeConflict {
    pub left: usize,
    pub right: usize,
    pub kind: PokeConflictKind,
}

fn same_memory_context(left: &PokeOperation, right: &PokeOperation) -> bool {
    left.memory_space == right.memory_space && left.bank == right.bank
}

fn operation_ranges_overlap(left: &PokeOperation, right: &PokeOperation) -> bool {
    let left_end = left
        .address
        .0
        .saturating_add((left.value.width_bits / 8).saturating_sub(1) as u32);
    let right_end = right
        .address
        .0
        .saturating_add((right.value.width_bits / 8).saturating_sub(1) as u32);
    left.address.0 <= right_end && right.address.0 <= left_end
}

pub fn find_poke_conflicts(cheats: &[PokeCheat]) -> Vec<PokeConflict> {
    let mut flattened = Vec::new();
    for (cheat_index, cheat) in cheats.iter().enumerate() {
        for operation in &cheat.operations {
            flattened.push((cheat_index, operation));
        }
    }
    let mut conflicts = Vec::new();
    for left in 0..flattened.len() {
        for right in (left + 1)..flattened.len() {
            let (li, lop) = flattened[left];
            let (ri, rop) = flattened[right];
            if li == ri || !same_memory_context(lop, rop) || !operation_ranges_overlap(lop, rop) {
                continue;
            }
            if lop.address != rop.address || lop.value != rop.value {
                conflicts.push(PokeConflict {
                    left: li,
                    right: ri,
                    kind: PokeConflictKind::SameLocationDifferentValue,
                });
            } else if lop.original_value != rop.original_value {
                conflicts.push(PokeConflict {
                    left: li,
                    right: ri,
                    kind: PokeConflictKind::OriginalGuardMismatch,
                });
            }
        }
    }
    conflicts
}

/// Identity strength for a POKE target is the one canonical ladder
/// ([`CheatApplicabilityMatch`]), not a private one. Callers must pass a hash
/// only when it was verified against the selected media; a claimed or
/// filename-derived value must be passed as a title/identity string instead.
pub fn poke_identity_state(
    media_hash: Option<&str>,
    game_identity: Option<&str>,
    verified: bool,
) -> CheatApplicabilityMatch {
    if media_hash.is_some_and(|value| !value.is_empty()) {
        CheatApplicabilityMatch::ExactHash
    } else if verified && game_identity.is_some_and(|value| !value.is_empty()) {
        CheatApplicabilityMatch::VerifiedIdentifier
    } else if game_identity.is_some_and(|value| !value.is_empty()) {
        CheatApplicabilityMatch::TitleOnly
    } else {
        CheatApplicabilityMatch::Unknown
    }
}

/// Only an exact hash or a verified identifier may be prepared for a runtime;
/// a title-only or missing identity stays preview-only.
pub fn poke_apply_allowed(state: CheatApplicabilityMatch) -> bool {
    matches!(
        state,
        CheatApplicabilityMatch::ExactHash | CheatApplicabilityMatch::VerifiedIdentifier
    )
}

impl PokeRuntimeSupport {
    /// Whether this adapter can generate a real, exact artifact (a Fuse `.pok`
    /// file or VICE monitor commands) once identity is verified. Preview-only
    /// and unsupported runtimes never produce one. Generating an artifact is
    /// not launching an emulator: no launch path consumes it yet.
    #[must_use]
    pub const fn can_prepare(self) -> bool {
        matches!(
            self,
            Self::SupportedNativeRuntime
                | Self::SupportedGeneratedScript
                | Self::SupportedMonitorCommand
        )
    }

    /// Plain wording for normal screens; the enum stays for details.
    #[must_use]
    pub const fn plain_label(self) -> &'static str {
        match self {
            Self::SupportedNativeRuntime
            | Self::SupportedGeneratedScript
            | Self::SupportedMonitorCommand => "Can be prepared once the exact game is confirmed",
            Self::PreviewOnly => "Preview only",
            Self::Unsupported => "Not supported",
            Self::UnknownUnproven => "Not proven",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_retro_cheat_providers::{ZxPokOperation, parse_zx_pok};

    #[test]
    fn zx_pok_preserves_bank_and_original_guard() {
        let trainer = parse_zx_pok("N Lives\nZ 5 32768 255 3\nY\n")
            .unwrap()
            .remove(0);
        let cheat = normalize_zx_pok_family(&trainer);
        assert_eq!(cheat.operations[0].bank, PokeBank::Number(5));
        assert_eq!(cheat.operations[0].original_value.unwrap().value, 3);
    }

    #[test]
    fn simple_pokes_cover_cpc_and_c64_without_executing_basic() {
        let cpc = parse_simple_pokes(PokePlatform::AmstradCpc, "Lives", "POKE 32768,255").unwrap();
        let c64 =
            parse_simple_pokes(PokePlatform::Commodore64, "Lives", "POKE $c000,$ff,0").unwrap();
        assert_eq!(cpc.operations[0].value.value, 255);
        assert_eq!(c64.operations[0].original_value.unwrap().value, 0);
    }

    #[test]
    fn banked_msx_and_unbanked_cpc_are_fail_closed() {
        assert!(matches!(
            manual_poke(
                PokePlatform::AmstradCpc,
                "x",
                1,
                1,
                None,
                PokeBank::Number(1),
                None,
                None,
                false
            ),
            Err(PokeParseError(PokeIssue::UnsupportedBanking { .. }))
        ));
        assert!(
            manual_poke(
                PokePlatform::Msx,
                "x",
                1,
                1,
                None,
                PokeBank::SlotPage { slot: 2, page: 3 },
                None,
                None,
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn invalid_values_and_title_only_identity_are_safe() {
        assert!(parse_simple_pokes(PokePlatform::Commodore64, "x", "POKE 65536,1").is_err());
        assert!(!poke_apply_allowed(poke_identity_state(
            None,
            Some("title"),
            false
        )));
    }

    #[test]
    fn conflicts_are_bank_aware() {
        let a = manual_poke(
            PokePlatform::ZxSpectrum,
            "a",
            1,
            2,
            None,
            PokeBank::Number(1),
            None,
            None,
            false,
        )
        .unwrap();
        let b = manual_poke(
            PokePlatform::ZxSpectrum,
            "b",
            1,
            3,
            None,
            PokeBank::Number(1),
            None,
            None,
            false,
        )
        .unwrap();
        let c = manual_poke(
            PokePlatform::ZxSpectrum,
            "c",
            1,
            3,
            None,
            PokeBank::Number(2),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(find_poke_conflicts(&[a, b.clone()]).len(), 1);
        assert!(find_poke_conflicts(&[b, c]).is_empty());
    }

    #[test]
    fn deterministic_and_no_media_mutation_model() {
        let first = parse_simple_pokes(PokePlatform::AtariSt, "x", "# local\nPOKE 100,1").unwrap();
        let second = parse_simple_pokes(PokePlatform::AtariSt, "x", "# local\nPOKE 100,1").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.source, "local manual/import");
    }

    #[test]
    fn keep_existing_zx_type_compile_contract() {
        let _ = ZxPokOperation {
            bank: 0,
            address: 1,
            value: 2,
            original_value: Some(0),
        };
    }

    #[test]
    fn trainer_expression_parser_accepts_sequence_and_rejects_basic() {
        let result = parse_poke_expression(
            PokePlatform::Commodore64,
            "Lives",
            "POKE 1024,1:POKE $0401,$02",
        );
        assert!(
            matches!(result, PokeExpressionResult::ParsedWriteSequence(writes) if writes.len() == 2)
        );
        assert!(matches!(
            parse_poke_expression(
                PokePlatform::ZxSpectrum,
                "x",
                "FOR i=1 TO 5: POKE 1,i:NEXT i"
            ),
            PokeExpressionResult::UnsafeExpression(_)
        ));
    }

    #[test]
    fn fuse_native_and_vice_monitor_projection_are_deterministic() {
        let mut fuse = manual_poke(
            PokePlatform::ZxSpectrum,
            "Lives",
            32768,
            255,
            Some(3),
            PokeBank::Number(5),
            None,
            Some("verified-release".into()),
            true,
        )
        .unwrap();
        fuse.operations.push(
            manual_poke(
                PokePlatform::ZxSpectrum,
                "Lives",
                32769,
                1,
                Some(0),
                PokeBank::Number(5),
                None,
                Some("verified-release".into()),
                true,
            )
            .unwrap()
            .operations
            .remove(0),
        );
        let projection = project_poke_runtime(&fuse, PokeRuntimeEmulator::Fuse);
        assert!(projection.refusal.is_none());
        assert_eq!(
            projection.commands_or_files[0],
            "NLives\nM 5 32768 255 3\nZ 5 32769 1 0\nY\n"
        );
        let vice = manual_poke(
            PokePlatform::Commodore64,
            "Lives",
            1024,
            255,
            None,
            PokeBank::Unspecified,
            None,
            Some("verified-release".into()),
            true,
        )
        .unwrap();
        let vice_projection = project_poke_runtime(&vice, PokeRuntimeEmulator::Vice);
        assert_eq!(vice_projection.commands_or_files, vec!["> $0400 ff"]);
    }

    #[test]
    fn runtime_projection_refuses_identity_guards_and_banking_it_cannot_represent() {
        let unverified = manual_poke(
            PokePlatform::Commodore64,
            "Lives",
            1024,
            1,
            None,
            PokeBank::Unspecified,
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            project_poke_runtime(&unverified, PokeRuntimeEmulator::Vice).refusal,
            Some(PokeProjectionRefusal::IdentityNotEligible)
        );
        let guarded = manual_poke(
            PokePlatform::Commodore64,
            "Lives",
            1024,
            1,
            Some(0),
            PokeBank::Unspecified,
            None,
            Some("verified".into()),
            true,
        )
        .unwrap();
        assert_eq!(
            project_poke_runtime(&guarded, PokeRuntimeEmulator::Vice).refusal,
            Some(PokeProjectionRefusal::OriginalGuardUnsupported)
        );
        let banked = manual_poke(
            PokePlatform::Commodore64,
            "Lives",
            1024,
            1,
            None,
            PokeBank::Number(1),
            None,
            Some("verified".into()),
            true,
        );
        assert!(banked.is_err());
    }

    #[test]
    fn overlapping_writes_conflict_even_when_start_addresses_differ() {
        let mut first = manual_poke(
            PokePlatform::AtariSt,
            "a",
            100,
            0x34,
            None,
            PokeBank::Unspecified,
            None,
            None,
            false,
        )
        .unwrap();
        first.operations[0].value.width_bits = 16;
        let second = manual_poke(
            PokePlatform::AtariSt,
            "b",
            101,
            0xff,
            None,
            PokeBank::Unspecified,
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(find_poke_conflicts(&[first, second]).len(), 1);
    }

    #[test]
    fn only_fuse_and_vice_can_be_prepared_everything_else_is_preview_only() {
        use PokeRuntimeEmulator as E;
        for emulator in [E::Fuse, E::Vice] {
            assert!(poke_runtime_capability(emulator).support.can_prepare());
        }
        for emulator in [E::Hatari, E::Caprice32, E::OpenMsx, E::BeebEm, E::BEm] {
            let capability = poke_runtime_capability(emulator);
            assert!(!capability.support.can_prepare(), "{emulator:?}");
            assert_eq!(capability.support.plain_label(), "Preview only");
            // Even with a verified identity a preview-only runtime refuses.
            let cheat = manual_poke(
                capability.platform,
                "Lives",
                0x8000,
                255,
                None,
                PokeBank::Unspecified,
                None,
                Some("exact-game".into()),
                true,
            )
            .unwrap();
            let projection = project_poke_runtime(&cheat, emulator);
            assert_eq!(
                projection.refusal,
                Some(PokeProjectionRefusal::UnsupportedRuntime),
                "{emulator:?}"
            );
            assert!(projection.commands_or_files.is_empty());
        }
    }

    #[test]
    fn identity_ladder_is_the_canonical_applicability_match() {
        use CheatApplicabilityMatch as M;
        assert_eq!(poke_identity_state(Some("h"), None, false), M::ExactHash);
        assert_eq!(
            poke_identity_state(None, Some("id"), true),
            M::VerifiedIdentifier
        );
        assert_eq!(
            poke_identity_state(None, Some("title"), false),
            M::TitleOnly
        );
        assert_eq!(poke_identity_state(None, None, false), M::Unknown);
        assert!(poke_apply_allowed(M::ExactHash) && poke_apply_allowed(M::VerifiedIdentifier));
        assert!(!poke_apply_allowed(M::TitleOnly) && !poke_apply_allowed(M::Strong));
    }

    #[test]
    fn unverified_target_blocks_even_a_supported_runtime() {
        let cheat = manual_poke(
            PokePlatform::ZxSpectrum,
            "Lives",
            0x8000,
            255,
            None,
            PokeBank::Unspecified,
            None,
            None,
            false,
        )
        .unwrap();
        assert!(!cheat.target_verified);
        let projection = project_poke_runtime(&cheat, PokeRuntimeEmulator::Fuse);
        assert_eq!(
            projection.refusal,
            Some(PokeProjectionRefusal::IdentityNotEligible)
        );
    }
}

#[cfg(test)]
mod identity_gate_regressions {
    use super::*;
    #[test]
    fn fuse_and_vice_require_both_verification_and_a_concrete_identity() {
        for (platform, emulator, bank, original) in [
            (
                PokePlatform::ZxSpectrum,
                PokeRuntimeEmulator::Fuse,
                PokeBank::Number(5),
                Some(3),
            ),
            (
                PokePlatform::Commodore64,
                PokeRuntimeEmulator::Vice,
                PokeBank::Unspecified,
                None,
            ),
        ] {
            for verified in [false, true] {
                for identity in [
                    None,
                    Some(""),
                    Some("  "),
                    Some("sha256:exact-reviewed-game"),
                ] {
                    let cheat = manual_poke(
                        platform,
                        "Lives",
                        32768,
                        255,
                        original,
                        bank.clone(),
                        None,
                        identity.map(str::to_owned),
                        verified,
                    )
                    .unwrap();
                    let projection = project_poke_runtime(&cheat, emulator);
                    let allowed = verified && identity.is_some_and(|s| !s.trim().is_empty());
                    assert_eq!(
                        projection.refusal.is_none(),
                        allowed,
                        "{emulator:?}: {identity:?}, {verified}"
                    );
                    assert_eq!(!projection.commands_or_files.is_empty(), allowed);
                }
            }
        }
    }
}
