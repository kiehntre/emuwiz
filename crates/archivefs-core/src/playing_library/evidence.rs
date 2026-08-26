//! Strict release-evidence tokens read from *provider-published* canonical
//! DAT entry names.
//!
//! No-Intro and friends publish every release's region, revision, languages,
//! and status inside delimited parentheses in each catalogue entry name
//! (`Super Mario (Europe) (En) (Rev 1A)`, `Sonic (USA) (Beta)`). This module
//! reads those tokens with the same discipline
//! [`crate::dat::classification::strict_multidisc_token`] already applies to
//! multi-disc parts:
//!
//! - Only a complete, delimited parenthesized (or comma-listed) whole token
//!   is evidence. A title merely *containing* "demo" or "Sample" is never
//!   evidence.
//! - Regions are recognized only from the fixed provider-region vocabulary;
//!   languages only as ISO-style codes (`En`, `Fr`, `Pt-BR`, ...). This stops
//!   an arbitrary token like `(Beta)` from ever matching a policy region.
//! - Every unrecognized token stays unknown, and unknown is never bad: an
//!   entry with no evidence at all is simply unranked, not excluded.

use super::model::{ReleaseClass, RevisionNumber};

/// Release evidence parsed from one canonical DAT entry name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatReleaseEvidence {
    pub raw_name: String,
    /// Recognized provider-region tokens, e.g. `["Europe"]`.
    pub regions: Vec<String>,
    /// Recognized language codes in declaration order, e.g. `["En", "Ja"]`.
    pub languages: Vec<String>,
    pub revision: Option<RevisionNumber>,
    /// Explicitly declared non-retail classes, e.g. `[ReleaseClass::Beta]`.
    pub release_classes: Vec<ReleaseClass>,
}

/// The closed provider-region vocabulary accepted as trusted region
/// evidence. Extend only when a provider's naming convention genuinely adds
/// a region label.
const PROVIDER_REGION_TOKENS: &[&str] = &[
    "Europe",
    "USA",
    "Japan",
    "World",
    "Asia",
    "Germany",
    "France",
    "Italy",
    "Spain",
    "Netherlands",
    "Portugal",
    "Sweden",
    "Denmark",
    "Finland",
    "Norway",
    "Poland",
    "Russia",
    "Greece",
    "Turkey",
    "Czech",
    "Slovakia",
    "Hungary",
    "Romania",
    "Ukraine",
    "Croatia",
    "Serbia",
    "Bulgaria",
    "Australia",
    "New Zealand",
    "Canada",
    "Mexico",
    "Brazil",
    "Argentina",
    "Chile",
    "China",
    "Taiwan",
    "Hong Kong",
    "Korea",
    "India",
    "Thailand",
    "Vietnam",
    "Indonesia",
    "Philippines",
    "Malaysia",
    "Singapore",
];

fn recognized_region(token: &str) -> Option<&'static str> {
    PROVIDER_REGION_TOKENS
        .iter()
        .find(|region| region.eq_ignore_ascii_case(token))
        .copied()
}

/// ISO 639-style base codes accepted as trusted language evidence,
/// complementing [`PROVIDER_REGION_TOKENS`]. Closed on purpose: an open
/// alphabetic-shape rule would read `(Beta)` / `(Sample)` / `(Promo)` as a
/// "language".
const LANGUAGE_BASE_CODES: &[&str] = &[
    "en", "ja", "fr", "de", "es", "it", "nl", "pt", "sv", "no", "da", "fi", "is", "pl", "cs", "sk",
    "hu", "ro", "bg", "el", "tr", "ru", "uk", "sr", "hr", "sl", "lt", "lv", "et", "zh", "ko", "he",
    "ar", "th", "vi", "id", "ms", "hi", "bn", "ta",
];

/// ISO-style language tags only: a base code from [`LANGUAGE_BASE_CODES`],
/// optionally followed by one script/region suffix (`Pt-BR`, `Zh-Hans`,
/// `Fr-CA`). Nothing else is a language.
fn recognized_language(token: &str) -> Option<String> {
    let mut parts = token.split('-');
    let base = parts.next()?;
    let Some(code) = LANGUAGE_BASE_CODES
        .iter()
        .find(|code| code.eq_ignore_ascii_case(base))
    else {
        return None;
    };
    let mut language = String::new();
    // Preserve the provider's own capitalisation (`Pt-BR` stays `Pt-BR`).
    language.push_str(&base[..code.len()]);
    if let Some(suffix) = parts.next() {
        if parts.next().is_some() || suffix.is_empty() || suffix.len() > 4 {
            return None;
        }
        let mut chars = suffix.chars();
        let Some(head) = chars.next() else {
            return None;
        };
        if !head.is_ascii_alphabetic() || !chars.all(|rest| rest.is_ascii_alphanumeric()) {
            return None;
        }
        language.push('-');
        language.push_str(suffix);
    }
    Some(language)
}

fn parse_revision_token(token: &str) -> Option<RevisionNumber> {
    // Exactly `Rev <digits>[.<digits>][<letter>]`. Anything else ("Reversi"
    // in a title) is not evidence.
    let rest = token.strip_prefix("Rev ")?;
    if rest.is_empty() {
        return None;
    }
    let digits_end = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len());
    if digits_end == 0 || digits_end > 5 {
        return None;
    }
    let mut number = RevisionNumber {
        major: rest[..digits_end].parse().ok()?,
        minor: 0,
        letter: '\0',
    };
    let mut remainder = &rest[digits_end..];
    if let Some(after_dot) = remainder.strip_prefix('.') {
        let minor_end = after_dot
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(after_dot.len());
        if minor_end == 0 || minor_end > 5 {
            return None;
        }
        number.minor = after_dot[..minor_end].parse().ok()?;
        remainder = &after_dot[minor_end..];
    }
    if remainder.is_empty() {
        return Some(number);
    }
    letter_suffix(remainder, number)
}

fn letter_suffix(remainder: &str, mut number: RevisionNumber) -> Option<RevisionNumber> {
    let mut chars = remainder.chars();
    let letter = chars.next()?;
    if !(letter.is_ascii_uppercase() || letter.is_ascii_lowercase()) || chars.next().is_some() {
        return None;
    }
    number.letter = letter.to_ascii_uppercase();
    Some(number)
}

/// Reads all strictly-delimited parenthesized tokens of a canonical entry
/// name, in left-to-right declaration order. Comma lists such as `(En,Ja)`
/// contribute one token per member, in list order.
///
/// The scan itself walks right-to-left exactly like
/// `crate::dat::classification::strict_multidisc_token` does (each iteration
/// peels off the rightmost remaining `(...)` group), but each group's tokens
/// are collected separately and the group order is reversed before
/// flattening - otherwise a name with more than one trailing group, such as
/// `(En) (Ja)`, would come back as `["Ja", "En"]` instead of the declared
/// `["En", "Ja"]`. A single group's own member order (`(En,Ja)`) was never
/// affected either way, since `split(',')` alone preserves it.
fn name_tokens(name: &str) -> Vec<String> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut rest = name;
    while let Some(close) = rest.rfind(')') {
        let before_close = &rest[..close];
        let Some(open) = before_close.rfind('(') else {
            break;
        };
        let inner = &before_close[open + 1..];
        let group: Vec<String> = inner
            .split(',')
            .map(str::trim)
            .filter(|member| !member.is_empty())
            .map(str::to_string)
            .collect();
        groups.push(group);
        rest = &before_close[..open];
    }
    groups.reverse();
    groups.into_iter().flatten().collect()
}

/// Parses release evidence out of one *provider-published* canonical DAT
/// entry name. Never call this on a local archive filename - a local file
/// name is presentation, not metadata.
pub fn dat_release_evidence(canonical_entry_name: &str) -> DatReleaseEvidence {
    let mut evidence = DatReleaseEvidence {
        raw_name: canonical_entry_name.to_string(),
        regions: Vec::new(),
        languages: Vec::new(),
        revision: None,
        release_classes: Vec::new(),
    };
    for token in name_tokens(canonical_entry_name) {
        if let Some(region) = recognized_region(&token) {
            if !evidence.regions.iter().any(|seen| seen == region) {
                evidence.regions.push(region.to_string());
            }
            continue;
        }
        if let Some(revision) = parse_revision_token(&token) {
            // The first strict revision token wins; real catalogues declare
            // at most one per entry.
            if evidence.revision.is_none() {
                evidence.revision = Some(revision);
            }
            continue;
        }
        let normalized = token.to_ascii_lowercase();
        if let Some(class) = ReleaseClass::all()
            .iter()
            .copied()
            .find(|class| normalized == class.token())
        {
            if !evidence.release_classes.contains(&class) {
                evidence.release_classes.push(class);
            }
            continue;
        }
        if let Some(language) = recognized_language(&token)
            && !evidence.languages.contains(&language)
        {
            evidence.languages.push(language);
            continue;
        }
    }
    evidence
}
