//! Pure, read-only Sega CD / Mega-CD boot-signature evidence extraction.
//!
//! # Deliberately narrow scope
//!
//! This module recognises the `SEGADISCSYSTEM` boot identifier and the
//! documented Disc ID product field. Sega's own official Mega-CD Disc Format
//! Specification PDF (referenced from `segaretro.org`) blocks automated
//! fetches. The Sega specification places the product/version field at `$180`,
//! so this module reads that fixed field only after validating the Sega CD
//! system identifier. See the public transcription of the Mega-CD Disc Format
//! Specification, Fig. A-3:
//! <https://gist.github.com/akiyan/a90d1e7d41ce89c532f29cafc356ccc2>.
//!
//! - `https://www.retrodev.com/segacd.html`
//! - the SpritesMind.Net Mega-CD development forum
//!   (`http://gendev.spritesmind.net/forum/viewtopic.php?t=2996`)
//! - Clownacy's `clownmdemu` development blog on booting Sonic CD
//!
//! all of which independently describe the same fact: the Mega-CD volume
//! header begins at offset 0 with the ASCII string `"SEGADISCSYSTEM"`,
//! immediately followed by `"CDBOOTLOADR"`.
//!
//! # Collision safety
//!
//! `SEGADISCSYSTEM` is a Sega-specific boot marker, not a generic optical
//! disc convention - but "Mega-CD" and "Sega CD" are the same hardware
//! under two regional names, and this signature alone says nothing about
//! which regional branding applies; that distinction (where it matters at
//! all) belongs to a resolver, never this module.

use crate::content_detector::{ContentDetectionOutcome, ContentDetector};
use crate::content_evidence::{ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind};

pub const SEGA_CD_BOOT_SIGNATURE: &[u8; 14] = b"SEGADISCSYSTEM";
pub const SEGA_CD_PRODUCT_FIELD_OFFSET: usize = 0x180;
pub const SEGA_CD_PRODUCT_FIELD_BYTES: usize = 14;
pub const SEGA_CD_DISC_ID_BYTES: usize = 0x200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegaCdProductFact {
    pub raw_product_code: String,
    pub product_code: String,
}

pub fn looks_like_sega_cd_boot_sector(bytes: &[u8]) -> bool {
    bytes.len() >= SEGA_CD_BOOT_SIGNATURE.len()
        && &bytes[..SEGA_CD_BOOT_SIGNATURE.len()] == SEGA_CD_BOOT_SIGNATURE.as_slice()
}

/// `Strong` `BootStructure` evidence when `bytes` begins with
/// [`SEGA_CD_BOOT_SIGNATURE`], otherwise no evidence at all.
pub fn observe_segacd_evidence(bytes: &[u8]) -> Vec<ContentEvidence> {
    if !looks_like_sega_cd_boot_sector(bytes) {
        return Vec::new();
    }
    vec![ContentEvidence::new(
        ContentEvidenceKind::BootStructure,
        "SEGADISCSYSTEM",
        ContentEvidenceConfidence::Strong,
        "Sega CD/Mega-CD volume header boot identifier present at offset 0",
    )]
}

/// Parses the fixed Disc ID product/version field. The documented example is
/// `GM T-12345-00`; padding between product number and version is normalised
/// to the stable form `GM T-12345-00`. The raw field is retained for
/// diagnostics, while the validated normalized value is suitable as identity.
pub fn parse_segacd_product_code(bytes: &[u8]) -> Option<SegaCdProductFact> {
    if !looks_like_sega_cd_boot_sector(bytes) || bytes.len() < SEGA_CD_DISC_ID_BYTES {
        return None;
    }
    let raw = bytes.get(
        SEGA_CD_PRODUCT_FIELD_OFFSET..SEGA_CD_PRODUCT_FIELD_OFFSET + SEGA_CD_PRODUCT_FIELD_BYTES,
    )?;
    if !raw
        .iter()
        .all(|byte| *byte == b' ' || byte.is_ascii_graphic())
    {
        return None;
    }
    let raw_product_code = String::from_utf8(raw.to_vec()).ok()?;
    let fields: Vec<&str> = raw_product_code.split_whitespace().collect();
    if fields.len() < 2 || !matches!(fields[0], "GM" | "AI") {
        return None;
    }
    let code = fields[1..].concat();
    if code.is_empty()
        || !code.contains('-')
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }
    let product_code = format!("{} {code}", fields[0]);
    Some(SegaCdProductFact {
        raw_product_code,
        product_code,
    })
}

pub struct SegaCdBootDetector;

impl ContentDetector for SegaCdBootDetector {
    fn id(&self) -> &'static str {
        "segacd_boot_signature"
    }

    fn detect(&self, data: &[u8]) -> ContentDetectionOutcome {
        if !looks_like_sega_cd_boot_sector(data) {
            return ContentDetectionOutcome::NotRecognized;
        }
        ContentDetectionOutcome::Recognized {
            evidence: observe_segacd_evidence(data),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_signature_is_detected() {
        let mut data = SEGA_CD_BOOT_SIGNATURE.to_vec();
        data.extend_from_slice(b"CDBOOTLOADR");
        assert!(looks_like_sega_cd_boot_sector(&data));
        assert!(SegaCdBootDetector.detect(&data).is_recognized());
    }

    #[test]
    fn non_matching_bytes_are_not_recognized() {
        assert!(!looks_like_sega_cd_boot_sector(b"not a sega cd disc"));
        assert_eq!(
            SegaCdBootDetector.detect(b"not a sega cd disc"),
            ContentDetectionOutcome::NotRecognized
        );
    }

    #[test]
    fn truncated_signature_fails_closed() {
        assert!(!looks_like_sega_cd_boot_sector(b"SEGADISC"));
    }

    #[test]
    fn evidence_is_strong_boot_structure() {
        let evidence = observe_segacd_evidence(SEGA_CD_BOOT_SIGNATURE.as_slice());
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].kind, ContentEvidenceKind::BootStructure);
        assert_eq!(evidence[0].confidence, ContentEvidenceConfidence::Strong);
    }

    #[test]
    fn no_signature_yields_no_evidence() {
        assert!(observe_segacd_evidence(b"random bytes here").is_empty());
    }

    #[test]
    fn evidence_never_assigns_a_platform() {
        for item in observe_segacd_evidence(SEGA_CD_BOOT_SIGNATURE.as_slice()) {
            assert_eq!(item.kind, ContentEvidenceKind::BootStructure);
        }
    }

    #[test]
    fn product_code_is_read_from_the_documented_disc_id_field() {
        let mut data = vec![b' '; SEGA_CD_DISC_ID_BYTES];
        data[..SEGA_CD_BOOT_SIGNATURE.len()].copy_from_slice(SEGA_CD_BOOT_SIGNATURE);
        data[SEGA_CD_PRODUCT_FIELD_OFFSET..SEGA_CD_PRODUCT_FIELD_OFFSET + 14]
            .copy_from_slice(b"GM T-12345 -00");
        let fact = parse_segacd_product_code(&data).unwrap();
        assert_eq!(fact.product_code, "GM T-12345-00");
        assert_eq!(fact.raw_product_code, "GM T-12345 -00");
    }

    #[test]
    fn missing_or_malformed_product_code_fails_closed() {
        let mut data = vec![b' '; SEGA_CD_DISC_ID_BYTES];
        data[..SEGA_CD_BOOT_SIGNATURE.len()].copy_from_slice(SEGA_CD_BOOT_SIGNATURE);
        assert_eq!(parse_segacd_product_code(&data), None);
        data[SEGA_CD_PRODUCT_FIELD_OFFSET..SEGA_CD_PRODUCT_FIELD_OFFSET + 14]
            .copy_from_slice(b"NO PRODUCT    ");
        assert_eq!(parse_segacd_product_code(&data), None);
    }
}
