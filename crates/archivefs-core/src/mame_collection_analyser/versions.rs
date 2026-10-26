//! A narrow evidence boundary around the shared MAME version parser.
//! The caller supplies captured MAME evidence; never search unrelated text for
//! its first numeric token. Unrecognised output remains unknown.
use super::*;
use crate::dat::model::DatEcosystem;
use crate::dat::parsers::mame_listxml::MAME_LISTXML_PARSER_SCHEMA_VERSION;
use crate::diagnostics::arcade_dat_version::MameVersion;

pub(super) fn compare(dat: &ParsedDat, installed: Option<&str>) -> MameVersionCompatibility {
    if dat.source.ecosystem != DatEcosystem::MAMEArcade
        || dat.games.is_empty()
        || dat
            .games
            .iter()
            .any(|g| g.mame_listxml_schema_version != Some(MAME_LISTXML_PARSER_SCHEMA_VERSION))
    {
        return MameVersionCompatibility::ExactSourceVersionUnknown;
    }
    match (
        dat.source
            .version
            .as_deref()
            .and_then(parse_version_evidence),
        installed.and_then(parse_version_evidence),
    ) {
        (Some(a), Some(b)) if a == b => MameVersionCompatibility::CatalogueVersionMatch,
        (Some(_), Some(_)) => MameVersionCompatibility::CatalogueVersionMismatch,
        _ => MameVersionCompatibility::ExactSourceVersionUnknown,
    }
}

pub fn parse_version_evidence(text: &str) -> Option<MameVersion> {
    if !text.is_ascii()
        || text
            .bytes()
            .any(|b| b.is_ascii_control() && !b.is_ascii_whitespace())
    {
        return None;
    }
    let mut words = text.split_ascii_whitespace();
    let mut token = words.next()?;
    if token.eq_ignore_ascii_case("mame") {
        token = words.next()?;
        if token.eq_ignore_ascii_case("version") {
            token = words.next()?;
        }
    }
    // Shared parse() accepts delimiters and searches multiple tokens. This
    // boundary gives it exactly one whole alphanumeric/dotted version token.
    if !token
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.')
    {
        return None;
    }
    let parsed = MameVersion::parse(token)?;
    if let Some(build) = words.next() {
        if words.next().is_some() || !valid_build(build, parsed) {
            return None;
        }
    }
    Some(parsed)
}

fn valid_build(build: &str, version: MameVersion) -> bool {
    let Some(tag) = build.strip_prefix('(').and_then(|s| s.strip_suffix(')')) else {
        return false;
    };
    let tag = tag.to_ascii_lowercase();
    let prefix = format!("mame{}", version.display().replace('.', ""));
    let Some(suffix) = tag.strip_prefix(&prefix) else {
        return false;
    };
    if suffix.is_empty() || suffix == "-dirty" {
        return true;
    }
    let Some(suffix) = suffix.strip_prefix('-') else {
        return false;
    };
    let mut parts = suffix.split('-');
    let Some(distance) = parts.next() else {
        return false;
    };
    let Some(hash) = parts.next().and_then(|s| s.strip_prefix('g')) else {
        return false;
    };
    if distance.is_empty()
        || !distance.bytes().all(|b| b.is_ascii_digit())
        || hash.is_empty()
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return false;
    }
    matches!(
        (parts.next(), parts.next()),
        (None, None) | (Some("dirty"), None)
    )
}
