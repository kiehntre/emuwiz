//! Clean-room decoding of documented classic Game Genie code families.
//!
//! This is an inspection/normalisation layer only. It never patches ROMs,
//! downloads code databases, or treats a title-only match as safe to apply.

use super::cheat_ir::{CheatOperation, CheatPlatform, CheatSourceFormat};
use serde::{Deserialize, Serialize};

const NES_ALPHABET: &str = "APZLGITYEOXUKSVN";
const SNES_ALPHABET: &str = "DF4709156BC8A23E";
const GENESIS_ALPHABET: &str = "ABCDEFGHJKLMNPRSTVWXYZ0123456789";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassicCheatFormat {
    NesGameGenie6,
    NesGameGenie8,
    SnesGameGenie,
    GenesisGameGenie,
    MasterSystemGameGenie,
    GameGearGameGenie,
    GameBoyGameGenie6,
    GameBoyGameGenie9,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameGeniePlatform {
    Nes,
    Snes,
    Genesis,
    MasterSystem,
    GameGear,
    GameBoy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameGenieRevisionSafety {
    ExactRomHash,
    VerifiedRomIdentity,
    VerifiedRegionOrRevision,
    TitleOnlyWarning,
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameGenieRevisionEvidence {
    pub rom_sha256: Option<String>,
    pub verified_rom_sha256: bool,
    pub verified_identity: bool,
    pub verified_region_or_revision: bool,
    pub title_only: bool,
}

impl GameGenieRevisionEvidence {
    pub fn unverified() -> Self {
        Self {
            rom_sha256: None,
            verified_rom_sha256: false,
            verified_identity: false,
            verified_region_or_revision: false,
            title_only: false,
        }
    }

    fn safety(&self) -> GameGenieRevisionSafety {
        if self.verified_rom_sha256 && self.rom_sha256.is_some() {
            GameGenieRevisionSafety::ExactRomHash
        } else if self.verified_identity {
            GameGenieRevisionSafety::VerifiedRomIdentity
        } else if self.verified_region_or_revision {
            GameGenieRevisionSafety::VerifiedRegionOrRevision
        } else if self.title_only {
            GameGenieRevisionSafety::TitleOnlyWarning
        } else {
            GameGenieRevisionSafety::Unverified
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameGenieInstruction {
    pub address: u32,
    pub value: u32,
    pub width_bits: u8,
    pub compare: Option<u8>,
    pub operation: CheatOperation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameGenieIssue {
    InvalidLength { expected: String, actual: usize },
    InvalidCharacter { character: char },
    AmbiguousShape { candidates: Vec<GameGeniePlatform> },
    RevisionUnverified,
    TitleOnlyMatch,
    AddressOutOfRange,
    InvalidCheckCharacter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameGenieProvenance {
    pub method: String,
    pub references: Vec<String>,
    pub clean_room: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameGenieDecodeStatus {
    Decoded,
    Ambiguous,
    Unsupported,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameGenieDecodeResult {
    pub original: String,
    pub normalized_code: String,
    pub platform: Option<GameGeniePlatform>,
    pub format: Option<ClassicCheatFormat>,
    pub instruction: Option<GameGenieInstruction>,
    pub status: GameGenieDecodeStatus,
    pub issues: Vec<GameGenieIssue>,
    pub revision_safety: GameGenieRevisionSafety,
    pub provenance: GameGenieProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameGenieDetection {
    pub normalized_code: String,
    pub candidates: Vec<(GameGeniePlatform, ClassicCheatFormat)>,
    pub status: GameGenieDecodeStatus,
    pub issues: Vec<GameGenieIssue>,
}

fn clean_code(raw: &str) -> Result<String, GameGenieIssue> {
    let trimmed = raw.trim().to_ascii_uppercase();
    let mut code = String::with_capacity(trimmed.len());
    for character in trimmed.chars() {
        if character != '-' && character != ' ' {
            if !character.is_ascii_alphanumeric() {
                return Err(GameGenieIssue::InvalidCharacter { character });
            }
            code.push(character);
        }
    }
    if code.is_empty() {
        return Err(GameGenieIssue::InvalidLength {
            expected: "a documented Game Genie length".into(),
            actual: 0,
        });
    }
    Ok(code)
}

fn lookup_nibbles(code: &str, alphabet: &str) -> Result<Vec<u8>, GameGenieIssue> {
    code.chars()
        .map(|character| {
            alphabet
                .find(character)
                .map(|value| value as u8)
                .ok_or(GameGenieIssue::InvalidCharacter { character })
        })
        .collect()
}

fn packed(nibbles: &[u8], bits: u8) -> u64 {
    nibbles
        .iter()
        .fold(0_u64, |value, nibble| (value << bits) | u64::from(*nibble))
}

fn nes_instruction(nibbles: &[u8]) -> GameGenieInstruction {
    let value = packed(nibbles, 4);
    let address = 0x8000
        | ((value >> 12) as u32 & 0x07)
        | ((value >> 16) as u32 & 0x78)
        | ((value >> 20) as u32 & 0x80)
        | (value as u32 & 0x0700)
        | ((value >> 4) as u32 & 0x7800);
    let replacement = ((value >> 28) as u32 & 0x07)
        | (value as u32 & 0x08)
        | ((value >> 20) as u32 & 0x70)
        | ((value >> 24) as u32 & 0x80);
    let compare = (nibbles.len() == 8).then(|| {
        ((value >> 4) as u8 & 0x07)
            | ((value >> 8) as u8 & 0x08)
            | ((value << 4) as u8 & 0x70)
            | (value as u8 & 0x80)
    });
    GameGenieInstruction {
        address,
        value: replacement,
        width_bits: 8,
        compare,
        operation: CheatOperation::Write8 {
            address: u64::from(address),
            value: replacement as u8,
        },
    }
}

fn snes_instruction(nibbles: &[u8]) -> GameGenieInstruction {
    let value = packed(nibbles, 4);
    let address = ((value >> 6) & 0x0f)
        | ((value >> 12) & 0x00f0)
        | ((value >> 6) & 0x0300)
        | ((value << 10) & 0x0c00)
        | ((value >> 8) & 0xf000)
        | ((value << 14) & 0x0f0000)
        | ((value << 10) & 0xf00000);
    let replacement = (value >> 24) & 0xff;
    GameGenieInstruction {
        address: address as u32,
        value: replacement as u32,
        width_bits: 8,
        compare: None,
        operation: CheatOperation::Write8 {
            address,
            value: replacement as u8,
        },
    }
}

fn genesis_instruction(nibbles: &[u8]) -> GameGenieInstruction {
    let value = packed(nibbles, 5);
    let replacement = ((value >> 32) & 0xff) | ((value >> 3) & 0x1f00) | ((value << 5) & 0xe000);
    let address = (value & 0xff00ff) | ((value >> 16) & 0xff00);
    GameGenieInstruction {
        address: address as u32,
        value: replacement as u32,
        width_bits: 16,
        compare: None,
        operation: CheatOperation::Write16 {
            address,
            value: replacement as u16,
        },
    }
}

fn sms_or_gg_instruction(code: &str) -> Result<GameGenieInstruction, GameGenieIssue> {
    let replacement = u8::from_str_radix(&code[0..2], 16)
        .map_err(|_| GameGenieIssue::InvalidCharacter { character: '?' })?;
    let raw_address = u16::from_str_radix(&format!("{}{}", &code[5..6], &code[2..5]), 16)
        .map_err(|_| GameGenieIssue::InvalidCharacter { character: '?' })?;
    let address = ((!raw_address & 0xf000) | (raw_address & 0x0fff)) as u32;
    if address > 0x7fff {
        return Err(GameGenieIssue::AddressOutOfRange);
    }
    let compare = if code.len() == 9 {
        let encoded = u8::from_str_radix(&format!("{}{}", &code[6..7], &code[8..9]), 16)
            .map_err(|_| GameGenieIssue::InvalidCharacter { character: '?' })?;
        Some(!(encoded.rotate_right(2) ^ 0x45))
    } else {
        None
    };
    Ok(GameGenieInstruction {
        address,
        value: u32::from(replacement),
        width_bits: 8,
        compare,
        operation: CheatOperation::Write8 {
            address: u64::from(address),
            value: replacement,
        },
    })
}

fn game_boy_instruction(code: &str) -> Result<GameGenieInstruction, GameGenieIssue> {
    let nibbles = code
        .chars()
        .map(|character| {
            character
                .to_digit(16)
                .map(|value| value as u8)
                .ok_or(GameGenieIssue::InvalidCharacter { character })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let replacement = (nibbles[0] << 4) | nibbles[1];
    let address = (u16::from(nibbles[2]) << 8)
        | (u16::from(nibbles[3]) << 4)
        | u16::from(nibbles[4])
        | (u16::from(!nibbles[5] & 0x0f) << 12);
    if !(0x0002..=0x7fff).contains(&address) {
        return Err(GameGenieIssue::AddressOutOfRange);
    }
    let compare = if nibbles.len() == 9 {
        if matches!(nibbles[6] ^ nibbles[7], 1..=7) {
            return Err(GameGenieIssue::InvalidCheckCharacter);
        }
        let encoded = nibbles[8] | (nibbles[6] << 4);
        Some(encoded.rotate_right(2) ^ 0xba)
    } else {
        None
    };
    let operation = match compare {
        Some(compare) => CheatOperation::ConditionalWrite8 {
            address: u64::from(address),
            value: replacement,
            compare,
        },
        None => CheatOperation::Write8 {
            address: u64::from(address),
            value: replacement,
        },
    };
    Ok(GameGenieInstruction {
        address: u32::from(address),
        value: u32::from(replacement),
        width_bits: 8,
        compare,
        operation,
    })
}

pub fn detect_classic_game_genie(raw: &str) -> GameGenieDetection {
    let normalized_code = match clean_code(raw) {
        Ok(code) => code,
        Err(issue) => {
            return GameGenieDetection {
                normalized_code: String::new(),
                candidates: Vec::new(),
                status: GameGenieDecodeStatus::Invalid,
                issues: vec![issue],
            };
        }
    };
    let length = normalized_code.len();
    let mut candidates = Vec::new();
    if length == 6 {
        candidates.push((GameGeniePlatform::Nes, ClassicCheatFormat::NesGameGenie6));
    } else if length == 8 {
        candidates.push((GameGeniePlatform::Nes, ClassicCheatFormat::NesGameGenie8));
        candidates.push((GameGeniePlatform::Snes, ClassicCheatFormat::SnesGameGenie));
        candidates.push((
            GameGeniePlatform::Genesis,
            ClassicCheatFormat::GenesisGameGenie,
        ));
    }
    if matches!(length, 6 | 9) && normalized_code.chars().all(|c| c.is_ascii_hexdigit()) {
        candidates.push((
            GameGeniePlatform::GameBoy,
            if length == 6 {
                ClassicCheatFormat::GameBoyGameGenie6
            } else {
                ClassicCheatFormat::GameBoyGameGenie9
            },
        ));
        candidates.push((
            GameGeniePlatform::MasterSystem,
            ClassicCheatFormat::MasterSystemGameGenie,
        ));
        candidates.push((
            GameGeniePlatform::GameGear,
            ClassicCheatFormat::GameGearGameGenie,
        ));
    }
    let status = match candidates.len() {
        0 => GameGenieDecodeStatus::Unsupported,
        1 => GameGenieDecodeStatus::Decoded,
        _ => GameGenieDecodeStatus::Ambiguous,
    };
    let issues = if status == GameGenieDecodeStatus::Ambiguous {
        vec![GameGenieIssue::AmbiguousShape {
            candidates: candidates.iter().map(|(platform, _)| *platform).collect(),
        }]
    } else {
        Vec::new()
    };
    GameGenieDetection {
        normalized_code,
        candidates,
        status,
        issues,
    }
}

pub fn decode_classic_game_genie(
    platform: GameGeniePlatform,
    raw: &str,
    evidence: &GameGenieRevisionEvidence,
) -> GameGenieDecodeResult {
    let provenance = GameGenieProvenance {
        method: "independent documented bit permutation".into(),
        references: vec![
            "NES: public NES Game Genie mapping documentation".into(),
            "SNES/Genesis/SMS: public emulator interoperability documentation".into(),
        ],
        clean_room: true,
    };
    let original = raw.to_string();
    let normalized_code = match clean_code(raw) {
        Ok(code) => code,
        Err(issue) => {
            return GameGenieDecodeResult {
                original,
                normalized_code: String::new(),
                platform: Some(platform),
                format: None,
                instruction: None,
                status: GameGenieDecodeStatus::Invalid,
                issues: vec![issue],
                revision_safety: evidence.safety(),
                provenance,
            };
        }
    };
    let decoded = match platform {
        GameGeniePlatform::Nes => {
            let format = match normalized_code.len() {
                6 => ClassicCheatFormat::NesGameGenie6,
                8 => ClassicCheatFormat::NesGameGenie8,
                actual => {
                    return invalid_result(
                        original,
                        normalized_code,
                        platform,
                        evidence,
                        provenance,
                        GameGenieIssue::InvalidLength {
                            expected: "6 or 8".into(),
                            actual,
                        },
                    );
                }
            };
            lookup_nibbles(&normalized_code, NES_ALPHABET).map(|n| (format, nes_instruction(&n)))
        }
        GameGeniePlatform::Snes => alphabet_decode(
            &normalized_code,
            8,
            SNES_ALPHABET,
            ClassicCheatFormat::SnesGameGenie,
            snes_instruction,
        ),
        GameGeniePlatform::Genesis => alphabet_decode(
            &normalized_code,
            8,
            GENESIS_ALPHABET,
            ClassicCheatFormat::GenesisGameGenie,
            genesis_instruction,
        ),
        GameGeniePlatform::MasterSystem => {
            sms_decode(&normalized_code, ClassicCheatFormat::MasterSystemGameGenie)
        }
        GameGeniePlatform::GameGear => {
            sms_decode(&normalized_code, ClassicCheatFormat::GameGearGameGenie)
        }
        GameGeniePlatform::GameBoy => {
            if !matches!(normalized_code.len(), 6 | 9) {
                Err(GameGenieIssue::InvalidLength {
                    expected: "6 or 9 hexadecimal characters".into(),
                    actual: normalized_code.len(),
                })
            } else {
                game_boy_instruction(&normalized_code).map(|instruction| {
                    (
                        if normalized_code.len() == 6 {
                            ClassicCheatFormat::GameBoyGameGenie6
                        } else {
                            ClassicCheatFormat::GameBoyGameGenie9
                        },
                        instruction,
                    )
                })
            }
        }
    };
    let (format, instruction) = match decoded {
        Ok(decoded) => decoded,
        Err(issue) => {
            return invalid_result(
                original,
                normalized_code,
                platform,
                evidence,
                provenance,
                issue,
            );
        }
    };
    let mut issues = Vec::new();
    match evidence.safety() {
        GameGenieRevisionSafety::Unverified => issues.push(GameGenieIssue::RevisionUnverified),
        GameGenieRevisionSafety::TitleOnlyWarning => issues.push(GameGenieIssue::TitleOnlyMatch),
        _ => {}
    }
    GameGenieDecodeResult {
        original,
        normalized_code,
        platform: Some(platform),
        format: Some(format),
        instruction: Some(instruction),
        status: GameGenieDecodeStatus::Decoded,
        issues,
        revision_safety: evidence.safety(),
        provenance,
    }
}

fn invalid_result(
    original: String,
    normalized_code: String,
    platform: GameGeniePlatform,
    evidence: &GameGenieRevisionEvidence,
    provenance: GameGenieProvenance,
    issue: GameGenieIssue,
) -> GameGenieDecodeResult {
    GameGenieDecodeResult {
        original,
        normalized_code,
        platform: Some(platform),
        format: None,
        instruction: None,
        status: GameGenieDecodeStatus::Invalid,
        issues: vec![issue],
        revision_safety: evidence.safety(),
        provenance,
    }
}

fn alphabet_decode(
    code: &str,
    expected: usize,
    alphabet: &str,
    format: ClassicCheatFormat,
    decoder: fn(&[u8]) -> GameGenieInstruction,
) -> Result<(ClassicCheatFormat, GameGenieInstruction), GameGenieIssue> {
    if code.len() != expected {
        return Err(GameGenieIssue::InvalidLength {
            expected: expected.to_string(),
            actual: code.len(),
        });
    }
    Ok((format, decoder(&lookup_nibbles(code, alphabet)?)))
}

fn sms_decode(
    code: &str,
    format: ClassicCheatFormat,
) -> Result<(ClassicCheatFormat, GameGenieInstruction), GameGenieIssue> {
    if !matches!(code.len(), 6 | 9) {
        return Err(GameGenieIssue::InvalidLength {
            expected: "6 or 9 hexadecimal characters".into(),
            actual: code.len(),
        });
    }
    Ok((format, sms_or_gg_instruction(code)?))
}

pub fn game_genie_to_document(
    result: &GameGenieDecodeResult,
    title: impl Into<String>,
) -> Option<super::cheat_ir::CheatDocument> {
    let instruction = result.instruction.as_ref()?;
    let platform = match result.platform? {
        GameGeniePlatform::Nes => CheatPlatform::Other("NES".into()),
        GameGeniePlatform::Snes => CheatPlatform::Other("SNES".into()),
        GameGeniePlatform::Genesis => CheatPlatform::Other("Mega Drive / Genesis".into()),
        GameGeniePlatform::MasterSystem => CheatPlatform::Other("Master System".into()),
        GameGeniePlatform::GameGear => CheatPlatform::Other("Game Gear".into()),
        GameGeniePlatform::GameBoy => CheatPlatform::Other("Game Boy".into()),
    };
    let source_format = CheatSourceFormat::Other(result.format.map_or_else(
        || "Classic Game Genie".into(),
        |format| format!("{format:?}"),
    ));
    let mut issues = result
        .issues
        .iter()
        .map(|issue| super::cheat_ir::CheatIssue::UnsupportedOperation(format!("{issue:?}")))
        .collect::<Vec<_>>();
    if instruction.compare.is_some() {
        issues.push(super::cheat_ir::CheatIssue::AmbiguousOperation(
            "Game Genie compare semantics are preserved as a conditional write".into(),
        ));
    }
    let operations = vec![instruction.operation.clone()];
    let title = title.into();
    let mut evidence = super::cheat_provenance::CheatRecordProvenance::original(
        Some(title.clone()),
        Some(result.original.clone()),
    );
    evidence.note_comparison(None, Some(result.normalized_code.clone()));
    Some(super::cheat_ir::CheatDocument {
        source_evidence: vec![evidence],
        title,
        platform,
        source_format,
        operations,
        issues,
        provenance: result.provenance.references.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nes_six_and_eight_character_codes_decode_with_compare() {
        let evidence = GameGenieRevisionEvidence::unverified();
        let six = decode_classic_game_genie(GameGeniePlatform::Nes, "GOSSIP", &evidence);
        assert_eq!(six.status, GameGenieDecodeStatus::Decoded);
        assert_eq!(six.instruction.as_ref().unwrap().address, 0x9d4d);
        assert_eq!(six.instruction.as_ref().unwrap().value, 0x00);
        assert_eq!(six.instruction.as_ref().unwrap().compare, None);
        let eight = decode_classic_game_genie(GameGeniePlatform::Nes, "APZLGITY", &evidence);
        assert_eq!(eight.status, GameGenieDecodeStatus::Decoded);
        assert!(eight.instruction.as_ref().unwrap().compare.is_some());
    }

    #[test]
    fn snes_and_genesis_vectors_decode_deterministically() {
        let evidence = GameGenieRevisionEvidence {
            rom_sha256: Some("a".repeat(64)),
            verified_rom_sha256: true,
            verified_identity: true,
            verified_region_or_revision: true,
            title_only: false,
        };
        let snes = decode_classic_game_genie(GameGeniePlatform::Snes, "DF47-0915", &evidence);
        let genesis = decode_classic_game_genie(GameGeniePlatform::Genesis, "ABCD-EFGH", &evidence);
        assert_eq!(snes.status, GameGenieDecodeStatus::Decoded);
        assert_eq!(genesis.status, GameGenieDecodeStatus::Decoded);
        assert_eq!(
            snes.instruction,
            decode_classic_game_genie(GameGeniePlatform::Snes, "DF470915", &evidence).instruction
        );
    }

    #[test]
    fn sms_and_game_gear_hex_codes_keep_optional_compare() {
        let result = decode_classic_game_genie(
            GameGeniePlatform::GameGear,
            "00A-12B-34C",
            &GameGenieRevisionEvidence::unverified(),
        );
        assert_eq!(result.status, GameGenieDecodeStatus::Decoded);
        assert!(result.instruction.as_ref().unwrap().compare.is_some());
    }

    #[test]
    fn unscoped_overlapping_shapes_are_ambiguous_and_bad_input_is_invalid() {
        assert_eq!(
            detect_classic_game_genie("DF470915").status,
            GameGenieDecodeStatus::Ambiguous
        );
        assert_eq!(
            detect_classic_game_genie("DF47_915").status,
            GameGenieDecodeStatus::Invalid
        );
        let hexadecimal = detect_classic_game_genie("0A1B9F");
        assert!(
            hexadecimal
                .candidates
                .iter()
                .any(|(platform, _)| *platform == GameGeniePlatform::GameBoy)
        );
    }

    #[test]
    fn title_only_is_a_warning_and_not_verified_revision_evidence() {
        let result = decode_classic_game_genie(
            GameGeniePlatform::Nes,
            "GOSSIP",
            &GameGenieRevisionEvidence {
                title_only: true,
                ..GameGenieRevisionEvidence::unverified()
            },
        );
        assert_eq!(
            result.revision_safety,
            GameGenieRevisionSafety::TitleOnlyWarning
        );
        assert!(result.issues.contains(&GameGenieIssue::TitleOnlyMatch));
    }

    #[test]
    fn game_boy_public_vectors_decode_value_address_and_compare() {
        let six = decode_classic_game_genie(
            GameGeniePlatform::GameBoy,
            "0A1-B9F",
            &GameGenieRevisionEvidence::unverified(),
        );
        assert_eq!(six.status, GameGenieDecodeStatus::Decoded);
        let six_instruction = six.instruction.unwrap();
        assert_eq!(six_instruction.address, 0x01b9);
        assert_eq!(six_instruction.value, 0x0a);
        assert_eq!(six_instruction.compare, None);
        assert!(matches!(
            six_instruction.operation,
            CheatOperation::Write8 { .. }
        ));

        let nine = decode_classic_game_genie(
            GameGeniePlatform::GameBoy,
            "068-5FF-E66",
            &GameGenieRevisionEvidence::unverified(),
        );
        assert_eq!(nine.status, GameGenieDecodeStatus::Decoded);
        let nine_instruction = nine.instruction.unwrap();
        assert_eq!(nine_instruction.address, 0x085f);
        assert_eq!(nine_instruction.value, 0x06);
        assert_eq!(nine_instruction.compare, Some(0x03));
        assert!(matches!(
            nine_instruction.operation,
            CheatOperation::ConditionalWrite8 {
                address: 0x085f,
                value: 0x06,
                compare: 0x03
            }
        ));
    }

    #[test]
    fn game_boy_rejects_bad_shape_check_address_and_non_ascii() {
        let evidence = GameGenieRevisionEvidence::unverified();
        assert!(matches!(
            decode_classic_game_genie(GameGeniePlatform::GameBoy, "GGGGGG", &evidence).status,
            GameGenieDecodeStatus::Invalid
        ));
        assert!(matches!(
            decode_classic_game_genie(GameGeniePlatform::GameBoy, "000-000", &evidence).issues[0],
            GameGenieIssue::AddressOutOfRange
        ));
        assert!(matches!(
            decode_classic_game_genie(GameGeniePlatform::GameBoy, "068-5FF-EF6", &evidence).issues
                [0],
            GameGenieIssue::InvalidCheckCharacter
        ));
        assert!(matches!(
            decode_classic_game_genie(GameGeniePlatform::GameBoy, "068-5FF-É66", &evidence).status,
            GameGenieDecodeStatus::Invalid
        ));
    }

    #[test]
    fn game_boy_compare_is_not_flattened_in_the_generic_document() {
        let result = decode_classic_game_genie(
            GameGeniePlatform::GameBoy,
            "068-5FF-E66",
            &GameGenieRevisionEvidence::unverified(),
        );
        let document = game_genie_to_document(&result, "Lives").unwrap();
        assert!(matches!(
            document.operations.as_slice(),
            [CheatOperation::ConditionalWrite8 {
                address: 0x085f,
                value: 0x06,
                compare: 0x03
            }]
        ));
    }

    #[test]
    fn game_boy_conditional_and_unconditional_writes_reach_conflict_analysis() {
        let evidence = GameGenieRevisionEvidence {
            rom_sha256: Some("a".repeat(64)),
            verified_rom_sha256: true,
            ..GameGenieRevisionEvidence::unverified()
        };
        let conditional = game_genie_to_document(
            &decode_classic_game_genie(GameGeniePlatform::GameBoy, "068-5FF-E66", &evidence),
            "Conditional",
        )
        .unwrap();
        let unconditional = game_genie_to_document(
            &decode_classic_game_genie(GameGeniePlatform::GameBoy, "078-5FF", &evidence),
            "Unconditional",
        )
        .unwrap();
        let left = super::super::cheat_compatibility::CheatCompatibilityEntry::from_document(
            "conditional",
            &conditional,
            "game-genie",
            "local",
            super::super::cheat_compatibility::CheatRevisionEvidence::ExactHash {
                hash: "same".into(),
            },
            super::super::cheat_compatibility::CheatMasterCodeRequirement::None,
        );
        let right = super::super::cheat_compatibility::CheatCompatibilityEntry::from_document(
            "unconditional",
            &unconditional,
            "game-genie",
            "local",
            super::super::cheat_compatibility::CheatRevisionEvidence::ExactHash {
                hash: "same".into(),
            },
            super::super::cheat_compatibility::CheatMasterCodeRequirement::None,
        );
        let report = super::super::cheat_compatibility::analyze_cheat_stack(&[left, right]);
        assert!(report.conflicts.iter().any(|conflict| {
            conflict.kind
                == super::super::cheat_compatibility::CheatConflictKind::ConditionalOverlap
        }));
    }
}
