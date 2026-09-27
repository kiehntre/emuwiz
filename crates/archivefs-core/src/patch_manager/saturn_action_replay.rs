//! Conservative Sega Saturn Action Replay code inspection.
//!
//! The accepted direct-write syntax is the public Saturn form
//! `TTAAAAAA VVVV`: `16` is a 16-bit write and `36` is an 8-bit write.
//! Control, conditional, master and unknown forms remain opaque; they are
//! never silently reduced to unconditional writes.

use super::cheat_ir::{
    CheatDocument, CheatIssue, CheatOperation, CheatPlatform, CheatSourceFormat,
};
use serde::{Deserialize, Serialize};

pub const SATURN_ACTION_REPLAY_MAX_BYTES: usize = 16 * 1024;
pub const SATURN_ACTION_REPLAY_MAX_CODES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnCheatOpcode {
    Write16,
    Write8,
    Conditional16,
    Master,
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnCheatIssue {
    Empty,
    NonAscii,
    Malformed(String),
    UnsupportedOpcode { opcode: String },
    MasterCodeRequired,
    WrongProduct { expected: String, actual: String },
    WrongRegion { expected: String, actual: String },
    WrongDisc { expected: u8, actual: u8 },
    TitleOnlyIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnCheatReadiness {
    ReadyToPreview,
    PreviewOnly,
    WrongRevision,
    Unsupported,
    Malformed,
    Ambiguous,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnCheatIdentityEvidence {
    pub exact_disc_hash: Option<String>,
    pub product_number: Option<String>,
    pub revision: Option<String>,
    pub region: Option<String>,
    pub disc_number: Option<u8>,
    pub disc_count: Option<u8>,
    pub title: Option<String>,
}

impl SaturnCheatIdentityEvidence {
    pub fn exact(hash: impl Into<String>) -> Self {
        Self {
            exact_disc_hash: Some(hash.into()),
            product_number: None,
            revision: None,
            region: None,
            disc_number: None,
            disc_count: None,
            title: None,
        }
    }
    pub fn title_only(title: impl Into<String>) -> Self {
        Self {
            exact_disc_hash: None,
            product_number: None,
            revision: None,
            region: None,
            disc_number: None,
            disc_count: None,
            title: Some(title.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnCheatCode {
    pub original: String,
    pub normalized: String,
    pub opcode: SaturnCheatOpcode,
    pub address: u32,
    pub value: u16,
    pub width_bytes: Option<u8>,
    pub operations: Vec<CheatOperation>,
    pub master_code: bool,
    pub requires_master: bool,
    pub issues: Vec<SaturnCheatIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnCheatDecodeResult {
    pub codes: Vec<SaturnCheatCode>,
    pub issues: Vec<SaturnCheatIssue>,
    pub readiness: SaturnCheatReadiness,
    pub provenance: Vec<String>,
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s, 16).ok()
}

/// Decode local/imported Saturn Action Replay text without network access.
pub fn decode_saturn_action_replay(text: &str) -> SaturnCheatDecodeResult {
    let mut result = SaturnCheatDecodeResult {
        codes: Vec::new(),
        issues: Vec::new(),
        readiness: SaturnCheatReadiness::ReadyToPreview,
        provenance: vec!["Local/imported Saturn Action Replay text".into()],
    };
    if text.len() > SATURN_ACTION_REPLAY_MAX_BYTES {
        result.issues.push(SaturnCheatIssue::Malformed(
            "input exceeds bounded size".into(),
        ));
        result.readiness = SaturnCheatReadiness::Malformed;
        return result;
    }
    for line in text.lines().filter(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with(';') && !t.starts_with('#')
    }) {
        if result.codes.len() >= SATURN_ACTION_REPLAY_MAX_CODES {
            result
                .issues
                .push(SaturnCheatIssue::Malformed("too many codes".into()));
            result.readiness = SaturnCheatReadiness::Malformed;
            break;
        }
        let original = line.trim().to_string();
        if !original.is_ascii() {
            result.issues.push(SaturnCheatIssue::NonAscii);
            result.readiness = SaturnCheatReadiness::Malformed;
            continue;
        }
        let fields: Vec<&str> = original
            .split(|c: char| c.is_ascii_whitespace() || c == ':')
            .filter(|s| !s.is_empty())
            .collect();
        if fields.len() != 2
            || fields[0].len() != 8
            || fields[1].len() != 4
            || !fields
                .iter()
                .all(|s| s.chars().all(|c| c.is_ascii_hexdigit()))
        {
            result.issues.push(SaturnCheatIssue::Malformed(original));
            result.readiness = SaturnCheatReadiness::Malformed;
            continue;
        }
        let word = fields[0].to_ascii_uppercase();
        let opcode = word[..2].to_string();
        let address = hex(&word[2..]).unwrap();
        let value = hex(fields[1]).unwrap() as u16;
        let (kind, width, operation, master, opaque) = match opcode.as_str() {
            "16" => (
                SaturnCheatOpcode::Write16,
                Some(2),
                Some(CheatOperation::Write16 {
                    address: address as u64,
                    value,
                }),
                false,
                false,
            ),
            "36" => (
                SaturnCheatOpcode::Write8,
                Some(1),
                Some(CheatOperation::Write8 {
                    address: address as u64,
                    value: value as u8,
                }),
                false,
                false,
            ),
            "F6" | "B6" => (SaturnCheatOpcode::Master, None, None, true, true),
            op if op.starts_with('D') => {
                (SaturnCheatOpcode::Conditional16, None, None, false, true)
            }
            _ => (
                SaturnCheatOpcode::Unsupported(opcode.clone()),
                None,
                None,
                false,
                true,
            ),
        };
        let mut issues = Vec::new();
        if opaque {
            issues.push(SaturnCheatIssue::UnsupportedOpcode {
                opcode: opcode.clone(),
            });
            result.readiness = SaturnCheatReadiness::PreviewOnly;
        }
        result.codes.push(SaturnCheatCode {
            original,
            normalized: format!("{} {:04X}", word, value),
            opcode: kind,
            address,
            value,
            width_bytes: width,
            operations: operation.into_iter().collect(),
            master_code: master,
            requires_master: !master && !matches!(opcode.as_str(), "16" | "36"),
            issues,
        });
    }
    if result.codes.is_empty() && result.issues.is_empty() {
        result.issues.push(SaturnCheatIssue::Empty);
        result.readiness = SaturnCheatReadiness::Malformed;
    }
    result
}

pub fn saturn_cheat_document(
    result: &SaturnCheatDecodeResult,
    title: impl Into<String>,
) -> CheatDocument {
    CheatDocument {
        title: title.into(),
        platform: CheatPlatform::Other("Sega Saturn".into()),
        source_format: CheatSourceFormat::Other("Saturn Action Replay".into()),
        operations: result
            .codes
            .iter()
            .flat_map(|c| c.operations.clone())
            .collect(),
        issues: result
            .codes
            .iter()
            .flat_map(|c| {
                c.issues
                    .iter()
                    .map(|i| CheatIssue::UnsupportedOperation(format!("{i:?}")))
            })
            .collect(),
        provenance: result.provenance.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_writes_normalize() {
        let r = decode_saturn_action_replay("16012345 00FF\n36012346 007A");
        assert_eq!(r.readiness, SaturnCheatReadiness::ReadyToPreview);
        assert_eq!(r.codes[0].width_bytes, Some(2));
        assert_eq!(
            r.codes[1].operations,
            vec![CheatOperation::Write8 {
                address: 0x012346,
                value: 0x7A
            }]
        );
    }
    #[test]
    fn master_and_conditional_stay_opaque() {
        let r = decode_saturn_action_replay("F6000914 C305\nB6002800 0000\nD6012345 00FF");
        assert_eq!(r.codes[0].opcode, SaturnCheatOpcode::Master);
        assert_eq!(r.codes[2].opcode, SaturnCheatOpcode::Conditional16);
        assert_eq!(r.readiness, SaturnCheatReadiness::PreviewOnly);
        assert!(r.codes[2].operations.is_empty());
    }
    #[test]
    fn malformed_and_non_ascii_fail_closed() {
        assert_eq!(
            decode_saturn_action_replay("16012345 0").readiness,
            SaturnCheatReadiness::Malformed
        );
        assert_eq!(
            decode_saturn_action_replay("１６０１２３４５ ００ＦＦ").readiness,
            SaturnCheatReadiness::Malformed
        );
    }
    #[test]
    fn normalization_is_deterministic() {
        let a = decode_saturn_action_replay("16012345 00ff");
        assert_eq!(a, decode_saturn_action_replay("16012345 00ff"));
        assert_eq!(a.codes[0].normalized, "16012345 00FF");
    }
}
