//! Neutral, bounded POKE/direct-memory cheat data for classic microcomputers.
//!
//! This is an interchange and preview model, not a machine-memory writer.
//! Addresses are never considered equivalent across memory spaces or banks.
//! Platform-specific native writers remain emulator adapters' responsibility.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::open_retro_cheat_providers::{ZxPokOperation, ZxPokTrainer};

pub const POKE_MAX_TITLE_BYTES: usize = 256;
pub const POKE_MAX_LINES: usize = 4096;
pub const POKE_MAX_OPERATIONS: usize = 512;

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
        let body = line
            .strip_prefix("POKE")
            .or_else(|| line.strip_prefix("poke"))
            .ok_or_else(|| {
                PokeParseError(PokeIssue::InvalidSyntax {
                    line: index + 1,
                    detail: "expected POKE address,value[,original]".into(),
                })
            })?
            .trim();
        let values: Vec<_> = body.split(',').map(str::trim).collect();
        if !(2..=3).contains(&values.len()) {
            return Err(PokeParseError(PokeIssue::InvalidSyntax {
                line: index + 1,
                detail: "expected two or three comma-separated values".into(),
            }));
        }
        let address = parse_number(values[0]).ok_or_else(|| {
            PokeParseError(PokeIssue::InvalidSyntax {
                line: index + 1,
                detail: "invalid address".into(),
            })
        })?;
        let value = parse_number(values[1]).ok_or_else(|| {
            PokeParseError(PokeIssue::InvalidSyntax {
                line: index + 1,
                detail: "invalid value".into(),
            })
        })?;
        let original = values
            .get(2)
            .map(|v| {
                parse_number(v).ok_or_else(|| {
                    PokeParseError(PokeIssue::InvalidSyntax {
                        line: index + 1,
                        detail: "invalid original value".into(),
                    })
                })
            })
            .transpose()?;
        let one = manual_poke(
            platform,
            title,
            address,
            value,
            original,
            PokeBank::Unspecified,
            None,
            None,
            false,
        )?;
        operations.extend(one.operations);
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

fn same_location(left: &PokeOperation, right: &PokeOperation) -> bool {
    left.memory_space == right.memory_space
        && left.bank == right.bank
        && left.address == right.address
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
            if li == ri || !same_location(lop, rop) {
                continue;
            }
            if lop.value != rop.value {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokeEmulatorProjection {
    RuntimeMemoryPoke,
    NativeCheatFile,
    ManualRuntimeAction,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokeEmulatorCapability {
    pub emulator: &'static str,
    pub platform: PokePlatform,
    pub projection: PokeEmulatorProjection,
    pub detail: &'static str,
}

pub fn poke_emulator_capabilities() -> Vec<PokeEmulatorCapability> {
    vec![
        PokeEmulatorCapability {
            emulator: "Fuse / RetroArch Fuse",
            platform: PokePlatform::ZxSpectrum,
            projection: PokeEmulatorProjection::NativeCheatFile,
            detail: "ZX .pok semantics are available; exact game identity is still required before Apply",
        },
        PokeEmulatorCapability {
            emulator: "Caprice32",
            platform: PokePlatform::AmstradCpc,
            projection: PokeEmulatorProjection::RuntimeMemoryPoke,
            detail: "runtime/manual projection only",
        },
        PokeEmulatorCapability {
            emulator: "VICE",
            platform: PokePlatform::Commodore64,
            projection: PokeEmulatorProjection::RuntimeMemoryPoke,
            detail: "monitor memory writes are runtime actions; bank must be explicit",
        },
        PokeEmulatorCapability {
            emulator: "Hatari",
            platform: PokePlatform::AtariSt,
            projection: PokeEmulatorProjection::RuntimeMemoryPoke,
            detail: "debugger memwrite is a runtime action; no stable native cheat file claimed",
        },
        PokeEmulatorCapability {
            emulator: "openMSX",
            platform: PokePlatform::Msx,
            projection: PokeEmulatorProjection::RuntimeMemoryPoke,
            detail: "poke/poke16 and trainer tooling are runtime-oriented; mapper context is explicit",
        },
        PokeEmulatorCapability {
            emulator: "BeebEm / b-em",
            platform: PokePlatform::BbcMicro,
            projection: PokeEmulatorProjection::ManualRuntimeAction,
            detail: "manual/import preview only in this phase",
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PokeIdentityState {
    ExactMediaHash,
    VerifiedRelease,
    WeakTitleOnly,
    Missing,
}

pub fn poke_identity_state(
    media_hash: Option<&str>,
    game_identity: Option<&str>,
    verified: bool,
) -> PokeIdentityState {
    if media_hash.is_some_and(|value| !value.is_empty()) {
        PokeIdentityState::ExactMediaHash
    } else if verified && game_identity.is_some_and(|value| !value.is_empty()) {
        PokeIdentityState::VerifiedRelease
    } else if game_identity.is_some_and(|value| !value.is_empty()) {
        PokeIdentityState::WeakTitleOnly
    } else {
        PokeIdentityState::Missing
    }
}

pub fn poke_apply_allowed(state: PokeIdentityState) -> bool {
    matches!(
        state,
        PokeIdentityState::ExactMediaHash | PokeIdentityState::VerifiedRelease
    )
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
}
