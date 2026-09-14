//! Read-only PS1/PS2 memory-card inventory.
//!
//! The card image is always the shared preservation unit. Entries below are
//! observations only; this module never extracts, rewrites, repairs, or
//! attributes a complete card to one game.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

pub const PS1_CARD_BYTES: usize = 128 * 1024;
pub const PS1_SLOT_COUNT: usize = 15;
pub const PS1_HEADER_BYTES: usize = 128;
pub const PS1_BLOCK_BYTES: usize = 8192;
pub const PS2_MAX_CARD_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_TITLE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardFormat {
    Ps1Raw,
    Ps2,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardFormatConfidence {
    ConfirmedFormat,
    LikelyFormat,
    UnknownFormat,
    Malformed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardHealth {
    Healthy,
    StructuralWarning,
    Malformed,
    Truncated,
    UnsupportedVariant,
    CorruptionSuspected,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardIdentityConfidence {
    ConfirmedIdentity,
    SupportingEvidence,
    HeuristicOnly,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCardEntryIdentity {
    pub product_code: Option<String>,
    pub display_title: Option<String>,
    pub confidence: MemoryCardIdentityConfidence,
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCardEntry {
    pub slot: Option<u32>,
    pub name: String,
    pub size_bytes: u64,
    pub used_blocks: Option<u32>,
    pub identity: MemoryCardEntryIdentity,
    pub deleted: bool,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCardInventory {
    pub path: PathBuf,
    pub format: MemoryCardFormat,
    pub format_confidence: MemoryCardFormatConfidence,
    pub health: MemoryCardHealth,
    pub card_size_bytes: u64,
    pub entries: Vec<MemoryCardEntry>,
    pub used_blocks: Option<u32>,
    pub free_blocks: Option<u32>,
    pub warnings: Vec<String>,
    pub shared_container: bool,
}

fn text(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let value = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
    (!value.is_empty()).then_some(value)
}
fn ps1_identity(header: &[u8], data: &[u8]) -> MemoryCardEntryIdentity {
    let product = text(header.get(12..22).unwrap_or_default());
    let title = text(data.get(4..4 + MAX_TITLE_BYTES).unwrap_or_default());
    let confidence = if product.as_deref().is_some_and(|v| v.contains('-')) {
        MemoryCardIdentityConfidence::ConfirmedIdentity
    } else if title.is_some() {
        MemoryCardIdentityConfidence::SupportingEvidence
    } else {
        MemoryCardIdentityConfidence::Unknown
    };
    let mut evidence = Vec::new();
    if product.is_some() {
        evidence.push("validated PS1 card product-code field".into());
    }
    if title.is_some() {
        evidence.push("PS1 save title field".into());
    }
    MemoryCardEntryIdentity {
        product_code: product,
        display_title: title,
        confidence,
        evidence,
    }
}

fn inspect_ps1(path: &Path, bytes: &[u8]) -> MemoryCardInventory {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut used = 0u32;
    let mut damaged = false;
    for slot in 0..PS1_SLOT_COUNT {
        let offset = slot * PS1_BLOCK_BYTES;
        let header = &bytes[offset..offset + PS1_HEADER_BYTES];
        let kind = header[0];
        let valid = matches!(kind, 0xA0 | 0x51 | 0x52 | 0x53 | 0xA1 | 0xA2 | 0xA3);
        if !valid {
            if kind != 0 {
                damaged = true;
                warnings.push(format!("slot {slot} has unknown block marker 0x{kind:02x}"));
            }
            continue;
        }
        if matches!(kind, 0xA0 | 0x51) {
            used += 1;
            let data = &bytes[offset + PS1_HEADER_BYTES..offset + PS1_BLOCK_BYTES];
            let checksum = header[..127].iter().fold(0u8, |sum, value| sum ^ value);
            let mut entry_warnings = Vec::new();
            if checksum != header[127] {
                entry_warnings
                    .push("header XOR checksum does not match; bytes were not repaired".into());
                damaged = true;
            }
            entries.push(MemoryCardEntry {
                slot: Some(slot as u32),
                name: text(&header[10..30]).unwrap_or_else(|| format!("slot-{slot}")),
                size_bytes: PS1_BLOCK_BYTES as u64,
                used_blocks: Some(1),
                identity: ps1_identity(header, data),
                deleted: false,
                warnings: entry_warnings,
            });
        } else if matches!(kind, 0xA1 | 0xA2 | 0xA3) {
            entries.push(MemoryCardEntry {
                slot: Some(slot as u32),
                name: format!("deleted-slot-{slot}"),
                size_bytes: 0,
                used_blocks: Some(1),
                identity: MemoryCardEntryIdentity {
                    product_code: None,
                    display_title: None,
                    confidence: MemoryCardIdentityConfidence::Unknown,
                    evidence: Vec::new(),
                },
                deleted: true,
                warnings: vec![
                    "Deleted slot state is reported only; deleted data is not recovered.".into(),
                ],
            });
        }
    }
    MemoryCardInventory {
        path: path.into(),
        format: MemoryCardFormat::Ps1Raw,
        format_confidence: MemoryCardFormatConfidence::ConfirmedFormat,
        health: if damaged {
            MemoryCardHealth::CorruptionSuspected
        } else {
            MemoryCardHealth::Healthy
        },
        card_size_bytes: bytes.len() as u64,
        entries,
        used_blocks: Some(used),
        free_blocks: Some((PS1_SLOT_COUNT as u32).saturating_sub(used)),
        warnings,
        shared_container: true,
    }
}

fn looks_like_ps2(bytes: &[u8]) -> bool {
    bytes.len() >= 32
        && bytes.get(..30).is_some_and(|head| {
            String::from_utf8_lossy(head).starts_with("Sony PS2 Memory Card Format")
        })
}

pub fn inspect_memory_card(path: &Path) -> Result<MemoryCardInventory, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("memory card must be a regular non-symlink file".into());
    }
    if metadata.len() > PS2_MAX_CARD_BYTES as u64 {
        return Err("memory card exceeds inspection bound".into());
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() == PS1_CARD_BYTES {
        return Ok(inspect_ps1(path, &bytes));
    }
    if looks_like_ps2(&bytes) {
        return Ok(MemoryCardInventory { path: path.into(), format: MemoryCardFormat::Ps2, format_confidence: MemoryCardFormatConfidence::ConfirmedFormat, health: MemoryCardHealth::UnsupportedVariant, card_size_bytes: bytes.len() as u64, entries: Vec::new(), used_blocks: None, free_blocks: None, warnings: vec!["PS2 card header is recognized, but filesystem allocation/directory semantics are not yet independently proven; no entries were guessed.".into()], shared_container: true });
    }
    Ok(MemoryCardInventory {
        path: path.into(),
        format: MemoryCardFormat::Unknown,
        format_confidence: if bytes.len() < PS1_CARD_BYTES {
            MemoryCardFormatConfidence::Malformed
        } else {
            MemoryCardFormatConfidence::UnknownFormat
        },
        health: if bytes.len() < PS1_CARD_BYTES {
            MemoryCardHealth::Truncated
        } else {
            MemoryCardHealth::Unknown
        },
        card_size_bytes: bytes.len() as u64,
        entries: Vec::new(),
        used_blocks: None,
        free_blocks: None,
        warnings: vec!["Size/structure did not prove a supported PS1 or PS2 card format.".into()],
        shared_container: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture() -> Vec<u8> {
        let mut card = vec![0u8; PS1_CARD_BYTES];
        card[0] = 0xA0;
        card[10..26].copy_from_slice(b"BASLUS-00067SAVE");
        card[12..22].copy_from_slice(b"BASLUS-000");
        card[128 + 4..128 + 4 + 10].copy_from_slice(b"Test Save\0");
        card[127] = card[..127].iter().fold(0u8, |sum, value| sum ^ value);
        card
    }
    #[test]
    fn ps1_card_is_shared_and_identity_is_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.mcr");
        let before = fixture();
        fs::write(&path, &before).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert!(inv.shared_container);
        assert_eq!(inv.entries.len(), 1);
        assert!(inv.entries[0].identity.product_code.is_some());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    #[test]
    fn truncated_and_unknown_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.bin");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"bad").unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.health, MemoryCardHealth::Truncated);
        assert!(inv.entries.is_empty());
    }
    #[test]
    fn ps2_header_is_not_falsely_enumerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.ps2");
        let mut bytes = vec![0u8; 8 * 1024 * 1024];
        bytes[..27].copy_from_slice(b"Sony PS2 Memory Card Format");
        fs::write(&path, &bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.format, MemoryCardFormat::Ps2);
        assert_eq!(inv.health, MemoryCardHealth::UnsupportedVariant);
        assert!(inv.entries.is_empty());
    }
}
