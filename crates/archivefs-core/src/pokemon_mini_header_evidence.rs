//! Bounded, read-only Pokémon Mini cartridge header evidence.
//!
//! The header lives at cartridge offset `0x2100`.  This module establishes
//! platform-compatible cartridge structure and exposes the header's code/title
//! as corroborating evidence; it deliberately does not resolve a commercial
//! release.  Exact release identity remains DAT/hash-led.

use crate::content_detector::{ContentDetectionOutcome, ContentDetector};
use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};

pub const POKEMON_MINI_MAX_ROM_BYTES: usize = 2 * 1024 * 1024;
pub const POKEMON_MINI_HEADER_OFFSET: usize = 0x2100;
pub const POKEMON_MINI_HEADER_BYTES: usize = 0xD0;
const NINTENDO_OFFSET: usize = POKEMON_MINI_HEADER_OFFSET + 0xA4;
const GAME_CODE_OFFSET: usize = POKEMON_MINI_HEADER_OFFSET + 0xAC;
const TITLE_OFFSET: usize = POKEMON_MINI_HEADER_OFFSET + 0xB0;
const PM_MARKER_OFFSET: usize = POKEMON_MINI_HEADER_OFFSET;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PokemonMiniHeaderFact {
    pub pm_marker_present: bool,
    pub game_code: String,
    pub title: String,
}

fn bounded_ascii_field(bytes: &[u8], offset: usize, length: usize) -> Option<String> {
    let field = bytes.get(offset..offset.checked_add(length)?)?;
    if field
        .iter()
        .any(|byte| *byte != 0 && !byte.is_ascii_graphic() && *byte != b' ')
    {
        return None;
    }
    let end = field.iter().position(|byte| *byte == 0).unwrap_or(length);
    Some(
        String::from_utf8_lossy(&field[..end])
            .trim_end()
            .to_string(),
    )
}

/// Parse the fixed, documented header fields. Every offset is bounds checked;
/// no cartridge-controlled count or allocation is used.
pub fn parse_pokemon_mini_header(bytes: &[u8]) -> Option<PokemonMiniHeaderFact> {
    let required = POKEMON_MINI_HEADER_OFFSET.checked_add(POKEMON_MINI_HEADER_BYTES)?;
    if bytes.len() < required || bytes.len() > POKEMON_MINI_MAX_ROM_BYTES {
        return None;
    }
    if bytes.get(NINTENDO_OFFSET..NINTENDO_OFFSET + 8)? != b"NINTENDO" {
        return None;
    }
    let game_code = bounded_ascii_field(bytes, GAME_CODE_OFFSET, 4)?;
    if game_code.is_empty() {
        return None;
    }
    let title = bounded_ascii_field(bytes, TITLE_OFFSET, 12)?;
    Some(PokemonMiniHeaderFact {
        pm_marker_present: bytes.get(PM_MARKER_OFFSET..PM_MARKER_OFFSET + 2) == Some(b"PM"),
        game_code,
        title,
    })
}

pub fn observe_pokemon_mini_evidence(fact: &PokemonMiniHeaderFact) -> Vec<ContentEvidence> {
    let mut evidence = vec![ContentEvidence::new(
        ContentEvidenceKind::BootStructure,
        "Pokemon Mini cartridge header",
        ContentEvidenceConfidence::Strong,
        "bounded cartridge header contains the documented NINTENDO watermark and valid game-code field",
    )];
    if fact.pm_marker_present {
        evidence.push(ContentEvidence::new(
            ContentEvidenceKind::ContentSignature,
            "PM",
            ContentEvidenceConfidence::Corroborated,
            "optional Pokémon Mini cartridge marker at the documented header offset",
        ));
    }
    evidence.push(ContentEvidence::new(
        ContentEvidenceKind::ProductCode,
        fact.game_code.clone(),
        ContentEvidenceConfidence::Corroborated,
        "candidate game code from the Pokémon Mini header; not release authority",
    ));
    if !fact.title.is_empty() {
        evidence.push(ContentEvidence::new(
            ContentEvidenceKind::ProductCode,
            fact.title.clone(),
            ContentEvidenceConfidence::Corroborated,
            "candidate title from the Pokémon Mini header; not release authority",
        ));
    }
    evidence
}

pub struct PokemonMiniHeaderDetector;

impl ContentDetector for PokemonMiniHeaderDetector {
    fn id(&self) -> &'static str {
        "pokemon_mini_cartridge_header"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        parse_pokemon_mini_header(data)
            .map(|fact| ContentDetectionOutcome::Recognized {
                evidence: observe_pokemon_mini_evidence(&fact),
            })
            .unwrap_or(ContentDetectionOutcome::NotRecognized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(marker: bool, code: &[u8], title: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; POKEMON_MINI_HEADER_OFFSET + POKEMON_MINI_HEADER_BYTES];
        if marker {
            bytes[PM_MARKER_OFFSET..PM_MARKER_OFFSET + 2].copy_from_slice(b"PM");
        }
        bytes[NINTENDO_OFFSET..NINTENDO_OFFSET + 8].copy_from_slice(b"NINTENDO");
        bytes[GAME_CODE_OFFSET..GAME_CODE_OFFSET + code.len().min(4)]
            .copy_from_slice(&code[..code.len().min(4)]);
        bytes[TITLE_OFFSET..TITLE_OFFSET + title.len().min(12)]
            .copy_from_slice(&title[..title.len().min(12)]);
        bytes
    }

    #[test]
    fn valid_header_is_recognized_without_requiring_optional_marker() {
        let fact = parse_pokemon_mini_header(&synthetic(false, b"ABCD", b"HOME")).unwrap();
        assert!(!fact.pm_marker_present);
        assert_eq!(fact.game_code, "ABCD");
    }

    #[test]
    fn malformed_and_truncated_headers_fail_closed() {
        assert!(parse_pokemon_mini_header(&[]).is_none());
        let mut bytes = synthetic(true, b"ABCD", b"HOME");
        bytes[NINTENDO_OFFSET] = b'X';
        assert!(parse_pokemon_mini_header(&bytes).is_none());
        assert!(parse_pokemon_mini_header(&bytes[..POKEMON_MINI_HEADER_OFFSET]).is_none());
    }

    #[test]
    fn oversized_rom_is_rejected() {
        let bytes = vec![0u8; POKEMON_MINI_MAX_ROM_BYTES + 1];
        assert!(parse_pokemon_mini_header(&bytes).is_none());
    }

    #[test]
    fn evidence_keeps_platform_and_release_claims_separate() {
        let fact = parse_pokemon_mini_header(&synthetic(true, b"ABCD", b"HOME")).unwrap();
        let evidence = observe_pokemon_mini_evidence(&fact);
        assert!(
            evidence
                .iter()
                .any(|item| item.kind == ContentEvidenceKind::BootStructure
                    && item.confidence == ContentEvidenceConfidence::Strong)
        );
        assert!(
            evidence
                .iter()
                .any(|item| item.kind == ContentEvidenceKind::ProductCode)
        );
    }
}
