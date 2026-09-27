//! Bounded native Gateway/Citra/Azahar 3DS cheat files.
//!
//! The native file is a title-ID-named text file containing `[name]` sections,
//! optional `*citra_enabled`, and Gateway words `XXXXXXXX YYYYYYYY`.
//! Only Gateway direct writes 0/1/2 are normalized. Control codes remain raw.

use super::cheat_ir::{
    CheatDocument, CheatIssue, CheatOperation, CheatPlatform, CheatSourceFormat,
};
use serde::{Deserialize, Serialize};

pub const THREE_DS_CHEAT_MAX_BYTES: usize = 256 * 1024;
pub const THREE_DS_CHEAT_MAX_ENTRIES: usize = 512;
pub const THREE_DS_CHEAT_MAX_LINES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreeDsCheatState {
    Enabled,
    Disabled,
    RuntimeOnly,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreeDsCheatIssue {
    Malformed(String),
    InvalidTitleId,
    WrongTitleId { expected: String, actual: String },
    UnsupportedOpcode(String),
    VersionMismatch { expected: String, actual: String },
    VersionUnknown,
    Duplicate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreeDsCheatReadiness {
    Ready,
    ReadyWithOpaqueOps,
    WrongTitle,
    WrongVersion,
    PreviewOnly,
    Malformed,
    Ambiguous,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreeDsCheatVersion {
    Exact(String),
    Compatible(String),
    Unknown,
    Mismatch { expected: String, actual: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreeDsCheatCode {
    pub original: String,
    pub normalized: String,
    pub opcode: u8,
    pub address: u32,
    pub value: u32,
    pub width_bytes: Option<u8>,
    pub operation: Option<CheatOperation>,
    pub issue: Option<ThreeDsCheatIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreeDsCheatEntry {
    pub name: String,
    pub enabled: bool,
    pub state: ThreeDsCheatState,
    pub update_version: Option<String>,
    pub codes: Vec<ThreeDsCheatCode>,
    pub comments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreeDsCheatFile {
    pub title_id: String,
    pub entries: Vec<ThreeDsCheatEntry>,
    pub readiness: ThreeDsCheatReadiness,
    pub issues: Vec<ThreeDsCheatIssue>,
    pub provenance: Vec<String>,
}

fn valid_title_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) && id.starts_with("0004")
}
fn parse_word(line: &str) -> Option<(u32, u32)> {
    let mut p = line.split_whitespace();
    let a = p.next()?;
    let v = p.next()?;
    if p.next().is_some()
        || a.len() != 8
        || v.len() != 8
        || !a.bytes().all(|b| b.is_ascii_hexdigit())
        || !v.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some((
        u32::from_str_radix(a, 16).ok()?,
        u32::from_str_radix(v, 16).ok()?,
    ))
}
fn version_from_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'v' && bytes[i + 1].is_ascii_digit() {
            let end = (i + 1..bytes.len())
                .find(|j| !bytes[*j].is_ascii_digit() && bytes[*j] != b'.')
                .unwrap_or(bytes.len());
            return Some(lower[i + 1..end].to_string());
        }
    }
    None
}

pub fn parse_three_ds_cheat_file(title_id: &str, text: &str) -> ThreeDsCheatFile {
    let mut file = ThreeDsCheatFile {
        title_id: title_id.to_ascii_uppercase(),
        entries: Vec::new(),
        readiness: ThreeDsCheatReadiness::Ready,
        issues: Vec::new(),
        provenance: vec!["Local/imported Azahar/Citra Gateway cheat text".into()],
    };
    if !valid_title_id(title_id) {
        file.issues.push(ThreeDsCheatIssue::InvalidTitleId);
        file.readiness = ThreeDsCheatReadiness::Malformed;
        return file;
    }
    if text.len() > THREE_DS_CHEAT_MAX_BYTES {
        file.issues.push(ThreeDsCheatIssue::Malformed(
            "file exceeds bounded size".into(),
        ));
        file.readiness = ThreeDsCheatReadiness::Malformed;
        return file;
    }
    let mut current: Option<ThreeDsCheatEntry> = None;
    let mut lines = 0;
    for raw in text.lines() {
        lines += 1;
        if lines > THREE_DS_CHEAT_MAX_LINES {
            file.issues
                .push(ThreeDsCheatIssue::Malformed("too many lines".into()));
            file.readiness = ThreeDsCheatReadiness::Malformed;
            break;
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            if let Some(entry) = current.take() {
                if file.entries.len() < THREE_DS_CHEAT_MAX_ENTRIES {
                    file.entries.push(entry);
                }
            }
            let name = line[1..line.len() - 1].trim();
            if name.is_empty() {
                file.issues
                    .push(ThreeDsCheatIssue::Malformed("empty cheat name".into()));
                continue;
            }
            current = Some(ThreeDsCheatEntry {
                name: name.to_string(),
                enabled: false,
                state: ThreeDsCheatState::Disabled,
                update_version: version_from_name(name),
                codes: Vec::new(),
                comments: Vec::new(),
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            file.issues
                .push(ThreeDsCheatIssue::Malformed(line.to_string()));
            file.readiness = ThreeDsCheatReadiness::Malformed;
            continue;
        };
        if line.eq_ignore_ascii_case("*citra_enabled") {
            entry.enabled = true;
            entry.state = ThreeDsCheatState::Enabled;
            continue;
        }
        if line.starts_with('*') {
            entry.comments.push(line.to_string());
            continue;
        }
        let Some((word, value)) = parse_word(line) else {
            file.issues
                .push(ThreeDsCheatIssue::Malformed(line.to_string()));
            file.readiness = ThreeDsCheatReadiness::Malformed;
            continue;
        };
        let opcode = (word >> 28) as u8;
        let address = word & 0x0FFF_FFFF;
        let (width, operation, issue) = match opcode {
            0x0 => (
                Some(4),
                Some(CheatOperation::Write32 {
                    address: address as u64,
                    value,
                }),
                None,
            ),
            0x1 => (
                Some(2),
                Some(CheatOperation::Write16 {
                    address: address as u64,
                    value: value as u16,
                }),
                None,
            ),
            0x2 => (
                Some(1),
                Some(CheatOperation::Write8 {
                    address: address as u64,
                    value: value as u8,
                }),
                None,
            ),
            _ => (
                None,
                None,
                Some(ThreeDsCheatIssue::UnsupportedOpcode(format!("{opcode:X}"))),
            ),
        };
        if issue.is_some() {
            file.readiness = ThreeDsCheatReadiness::ReadyWithOpaqueOps;
        }
        entry.codes.push(ThreeDsCheatCode {
            original: line.to_string(),
            normalized: format!("{word:08X} {value:08X}"),
            opcode,
            address,
            value,
            width_bytes: width,
            operation,
            issue,
        });
    }
    if let Some(entry) = current {
        if file.entries.len() < THREE_DS_CHEAT_MAX_ENTRIES {
            file.entries.push(entry);
        }
    }
    if file.entries.is_empty() && file.issues.is_empty() {
        file.issues
            .push(ThreeDsCheatIssue::Malformed("no cheat sections".into()));
        file.readiness = ThreeDsCheatReadiness::Malformed;
    }
    file
}

pub fn render_three_ds_cheat_file(file: &ThreeDsCheatFile) -> String {
    let mut out = String::new();
    for (index, entry) in file.entries.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push('[');
        out.push_str(&entry.name);
        out.push_str("]\n");
        if entry.enabled {
            out.push_str("*citra_enabled\n");
        }
        for comment in &entry.comments {
            out.push_str(comment);
            out.push('\n');
        }
        for code in &entry.codes {
            out.push_str(&code.normalized);
            out.push('\n');
        }
    }
    out
}

pub fn merge_three_ds_cheat_file(
    existing: &ThreeDsCheatFile,
    incoming: &ThreeDsCheatFile,
) -> ThreeDsCheatFile {
    let mut merged = existing.clone();
    for candidate in &incoming.entries {
        if let Some(found) = merged.entries.iter_mut().find(|e| {
            e.name == candidate.name
                && e.codes
                    .iter()
                    .map(|c| &c.normalized)
                    .eq(candidate.codes.iter().map(|c| &c.normalized))
        }) {
            found.enabled = candidate.enabled;
            found.state = candidate.state.clone();
            continue;
        }
        merged.entries.push(candidate.clone());
    }
    merged
}

pub fn assess_three_ds_version(
    actual: Option<&str>,
    expected: Option<&str>,
) -> ThreeDsCheatVersion {
    match (actual, expected) {
        (Some(a), Some(e)) if a == e => ThreeDsCheatVersion::Exact(a.to_string()),
        (Some(a), Some(e)) => ThreeDsCheatVersion::Mismatch {
            expected: e.to_string(),
            actual: a.to_string(),
        },
        (Some(a), None) => ThreeDsCheatVersion::Compatible(a.to_string()),
        _ => ThreeDsCheatVersion::Unknown,
    }
}

pub fn three_ds_cheat_document(
    file: &ThreeDsCheatFile,
    entry: &ThreeDsCheatEntry,
) -> CheatDocument {
    CheatDocument {
        title: entry.name.clone(),
        platform: CheatPlatform::Other("Nintendo 3DS".into()),
        source_format: CheatSourceFormat::Other("Azahar/Citra Gateway".into()),
        operations: entry
            .codes
            .iter()
            .filter_map(|c| c.operation.clone())
            .collect(),
        issues: entry
            .codes
            .iter()
            .filter_map(|c| {
                c.issue
                    .as_ref()
                    .map(|i| CheatIssue::UnsupportedOperation(format!("{i:?}")))
            })
            .collect(),
        provenance: file.provenance.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "0004000000123456";
    #[test]
    fn parses_direct_and_opaque_codes() {
        let f = parse_three_ds_cheat_file(
            ID,
            "[Infinite HP v1.2]\n*citra_enabled\n00123456 000000FF\n10123458 00001234\n2012345A 0000007F\nD0000000 00000000",
        );
        assert_eq!(f.entries[0].state, ThreeDsCheatState::Enabled);
        assert_eq!(f.entries[0].codes[0].width_bytes, Some(4));
        assert_eq!(f.entries[0].codes[1].width_bytes, Some(2));
        assert_eq!(f.entries[0].codes[2].width_bytes, Some(1));
        assert_eq!(f.readiness, ThreeDsCheatReadiness::ReadyWithOpaqueOps);
    }
    #[test]
    fn rejects_wrong_title_and_malformed() {
        assert_eq!(
            parse_three_ds_cheat_file("0000000000000000", "[x]\n00123456 1").readiness,
            ThreeDsCheatReadiness::Malformed
        );
        assert_eq!(
            parse_three_ds_cheat_file(ID, "[x]\nnot code").readiness,
            ThreeDsCheatReadiness::Malformed
        );
    }
    #[test]
    fn render_and_merge_are_deterministic_and_preserve_unrelated() {
        let a = parse_three_ds_cheat_file(
            ID,
            "[A]\n00123456 00000001\n[B]\n*citra_enabled\n20123456 00000002\n",
        );
        let b = parse_three_ds_cheat_file(ID, "[C]\n00123457 00000003\n");
        let m = merge_three_ds_cheat_file(&a, &b);
        assert_eq!(
            render_three_ds_cheat_file(&m),
            render_three_ds_cheat_file(&m)
        );
        assert!(render_three_ds_cheat_file(&m).contains("[B]\n*citra_enabled"));
        assert_eq!(m.entries.len(), 3);
    }
    #[test]
    fn version_is_explicit() {
        assert_eq!(
            assess_three_ds_version(Some("1.2"), Some("1.1")),
            ThreeDsCheatVersion::Mismatch {
                expected: "1.1".into(),
                actual: "1.2".into()
            }
        );
    }
}
