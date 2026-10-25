//! Validation of the local file boundary; existing evidence/policy models stay intact.

use serde_json::Value;

use super::{LocalCatalogueError, LocalHomebrewRecord};
use crate::homebrew_artifact::{
    ArtifactChecksumAlgorithm, HomebrewProjectId, HomebrewProviderId, PlatformClaimEvidence,
    RightsEvidence, RightsFact, SourceAvailability,
};
use crate::mod_catalogue::{MAX_CATALOGUE_TEXT_BYTES, MAX_CATALOGUE_URL_BYTES};

fn invalid(field: &str, reason: &str) -> LocalCatalogueError {
    LocalCatalogueError::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

// Compare the original keys to the existing models' serialized field shape.
// This rejects nested typos without adding parallel wire versions of every
// Homebrew model or changing their existing deserialization contracts.
pub(super) fn known_fields(
    raw: &Value,
    known: &Value,
    field: &str,
) -> Result<(), LocalCatalogueError> {
    match (raw, known) {
        (Value::Object(raw), Value::Object(known)) => {
            for (key, value) in raw {
                let field = format!("{field}.{key}");
                let expected = known
                    .get(key)
                    .ok_or_else(|| invalid(&field, "unknown field"))?;
                known_fields(value, expected, &field)?;
            }
        }
        (Value::Array(raw), Value::Array(known)) => {
            for (index, (value, expected)) in raw.iter().zip(known).enumerate() {
                known_fields(value, expected, &format!("{field}[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn bounded_strings(value: &Value, field: &str) -> Result<(), LocalCatalogueError> {
    match value {
        Value::String(text) => {
            if text.len() > MAX_CATALOGUE_URL_BYTES || text.contains('\0') {
                return Err(invalid(field, "text contains NUL or exceeds 8192 bytes"));
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                bounded_strings(value, &format!("{field}[{index}]"))?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                bounded_strings(value, &format!("{field}.{key}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn text(value: &str, field: &str, required: bool) -> Result<(), LocalCatalogueError> {
    if value.len() > MAX_CATALOGUE_TEXT_BYTES
        || value.contains('\0')
        || required && value.trim().is_empty()
    {
        return Err(invalid(
            field,
            "required text is empty, contains NUL or exceeds 4096 bytes",
        ));
    }
    Ok(())
}

/// Characters that are control codes or invisible and can spoof, hide or
/// reformat text when a title is rendered. Zero-width joiner/non-joiner and
/// variation selectors are deliberately allowed: emoji sequences and Persian,
/// Indic and other scripts need them.
fn disallowed_display_char(c: char) -> Option<&'static str> {
    if c.is_control() {
        // C0, DEL and C1: escapes, CR/LF, tabs, NEL, CSI and the rest.
        return Some("control character");
    }
    match c {
        '\u{200B}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}' => Some("invisible character"),
        '\u{2028}' | '\u{2029}' => Some("line or paragraph separator"),
        '\u{061C}'
        | '\u{200E}'
        | '\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' => Some("bidirectional formatting character"),
        _ => None,
    }
}

/// Titles and summaries are shown to people, so they get the ordinary text
/// bounds plus a refusal of control and spoofing characters. The error names
/// the field and the first offending code point.
pub(super) fn display_text(
    value: &str,
    field: &str,
    required: bool,
) -> Result<(), LocalCatalogueError> {
    if let Some((index, c, kind)) = value
        .chars()
        .enumerate()
        .find_map(|(i, c)| disallowed_display_char(c).map(|kind| (i, c, kind)))
    {
        return Err(invalid(
            field,
            &format!(
                "contains a {kind} (U+{:04X}) at character {index}; titles and summaries must be plain visible text",
                c as u32
            ),
        ));
    }
    text(value, field, required)
}

// Validate titles/summaries before catalogue-wide NUL checks can hide the
// first forbidden character. Other fields keep their existing validation.
pub(super) fn display_fields(records: &[LocalHomebrewRecord]) -> Result<(), LocalCatalogueError> {
    for (index, record) in records.iter().enumerate() {
        let field = format!("records[{index}].project");
        display_text(&record.project.title, &format!("{field}.title"), true)?;
        if let Some(summary) = &record.project.summary {
            display_text(summary, &format!("{field}.summary"), false)?;
        }
    }
    Ok(())
}

fn url(value: &str, field: &str) -> Result<(), LocalCatalogueError> {
    let parsed =
        url::Url::parse(value).map_err(|_| invalid(field, "malformed HTTP(S) source URL"))?;
    if value.len() > MAX_CATALOGUE_URL_BYTES
        || value.chars().any(char::is_control)
        || value.trim() != value
        || !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(invalid(
            field,
            "expected an HTTP(S) source URL without credentials or control characters",
        ));
    }
    Ok(())
}

pub(super) fn provider_id(
    provider: &HomebrewProviderId,
    field: &str,
) -> Result<(), LocalCatalogueError> {
    HomebrewProviderId::new(provider.as_str())
        .map_err(|_| invalid(field, "invalid provider namespace"))?;
    text(provider.as_str(), field, true)
}

fn project_id(id: &HomebrewProjectId, field: &str) -> Result<(), LocalCatalogueError> {
    provider_id(&id.provider, &format!("{field}.provider"))?;
    text(&id.provider_id, &format!("{field}.provider_id"), true)
}

fn fact<T>(fact: &RightsFact<T>, field: &str) -> Result<(), LocalCatalogueError> {
    if let RightsFact::Known { evidence, .. } = fact {
        text(evidence, &format!("{field}.evidence"), true)?;
    }
    Ok(())
}

fn rights(rights: &RightsEvidence, field: &str) -> Result<(), LocalCatalogueError> {
    fact(
        &rights.software_licence,
        &format!("{field}.software_licence"),
    )?;
    fact(&rights.asset_licence, &format!("{field}.asset_licence"))?;
    fact(&rights.redistribution, &format!("{field}.redistribution"))?;
    fact(
        &rights.commercial_status,
        &format!("{field}.commercial_status"),
    )?;
    fact(
        &rights.source_availability,
        &format!("{field}.source_availability"),
    )?;
    for (name, fact) in [
        ("software_licence", &rights.software_licence),
        ("asset_licence", &rights.asset_licence),
    ] {
        if let RightsFact::Known { value, .. } = fact {
            text(value, &format!("{field}.{name}.value"), true)?;
        }
    }
    if let RightsFact::Known {
        value: SourceAvailability::Available {
            source_url: Some(value),
        },
        ..
    } = &rights.source_availability
    {
        url(value, &format!("{field}.source_availability.source_url"))?;
    }
    Ok(())
}

fn date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit())
    {
        return false;
    }
    let year = value[..4].parse::<u32>().unwrap_or(0);
    let month = value[5..7].parse::<usize>().unwrap_or(0);
    let day = value[8..].parse::<u32>().unwrap_or(0);
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    day >= 1 && day <= days[month - 1]
}

pub(super) fn record(
    record: &LocalHomebrewRecord,
    provider: &HomebrewProviderId,
    field: &str,
) -> Result<(), LocalCatalogueError> {
    let project = &record.project;
    project_id(&project.id, &format!("{field}.project.id"))?;
    if project.id.provider != *provider {
        return Err(invalid(
            &format!("{field}.project.id.provider"),
            "does not match catalogue namespace",
        ));
    }
    url(
        &project.source_page_url,
        &format!("{field}.project.source_page_url"),
    )?;
    rights(&project.rights, &format!("{field}.project.rights"))?;
    for (index, link) in project.links.iter().enumerate() {
        project_id(
            &link.project,
            &format!("{field}.project.links[{index}].project"),
        )?;
        text(
            &link.evidence,
            &format!("{field}.project.links[{index}].evidence"),
            true,
        )?;
    }
    if record.observations.is_empty() {
        return Err(invalid(
            &format!("{field}.observations"),
            "at least one dated source observation is required",
        ));
    }
    for (index, observation) in record.observations.iter().enumerate() {
        let field = format!("{field}.observations[{index}]");
        url(&observation.source_url, &format!("{field}.source_url"))?;
        if !date(&observation.observed_on) {
            return Err(invalid(
                &format!("{field}.observed_on"),
                "expected a calendar date YYYY-MM-DD",
            ));
        }
        text(&observation.note, &format!("{field}.note"), true)?;
    }
    for (index, listing) in record.listings.iter().enumerate() {
        let field = format!("{field}.listings[{index}]");
        if listing.project != project.id {
            return Err(invalid(
                &format!("{field}.project"),
                "does not match the containing project identity",
            ));
        }
        if let Some(release) = &listing.release {
            let release_id = release.provider_release_id.as_deref().ok_or_else(|| {
                invalid(
                    &format!("{field}.release.provider_release_id"),
                    "explicit release id required; use null release for an unversioned batch",
                )
            })?;
            text(
                release_id,
                &format!("{field}.release.provider_release_id"),
                true,
            )?;
            for (name, value) in [("version", &release.version), ("tag", &release.tag)] {
                if let Some(value) = value {
                    text(value, &format!("{field}.release.{name}"), true)?;
                }
            }
        }
        for (index, artifact) in listing.artifacts.iter().enumerate() {
            let field = format!("{field}.artifacts[{index}]");
            text(
                &artifact.provider_asset_id,
                &format!("{field}.provider_asset_id"),
                true,
            )?;
            text(&artifact.filename, &format!("{field}.filename"), true)?;
            url(
                &artifact.acquisition.source_page_url,
                &format!("{field}.acquisition.source_page_url"),
            )?;
            if let Some(link) = &artifact.acquisition.link_url {
                url(link, &format!("{field}.acquisition.link_url"))?;
            }
            rights(&artifact.rights, &format!("{field}.rights"))?;
            if let Some(digest) = &artifact.provider_digest {
                let length = match &digest.algorithm {
                    ArtifactChecksumAlgorithm::Sha256 => Some(64),
                    ArtifactChecksumAlgorithm::Sha1 => Some(40),
                    ArtifactChecksumAlgorithm::Md5 => Some(32),
                    ArtifactChecksumAlgorithm::Unknown(name) => {
                        text(name, &format!("{field}.provider_digest.algorithm"), true)?;
                        None
                    }
                };
                text(
                    &digest.value,
                    &format!("{field}.provider_digest.value"),
                    true,
                )?;
                if length.is_some_and(|len| {
                    digest.value.len() != len
                        || !digest.value.bytes().all(|b| b.is_ascii_hexdigit())
                }) {
                    return Err(invalid(
                        &format!("{field}.provider_digest"),
                        "digest must have the algorithm's exact hexadecimal length",
                    ));
                }
            }
            for (index, claim) in artifact.platform_claims.iter().enumerate() {
                let field = format!("{field}.platform_claims[{index}]");
                text(&claim.platform, &format!("{field}.platform"), true)?;
                // An unregistered name may stay as evidence, but a case or
                // whitespace variant of a registered ID would look like that
                // platform to a reader and silently fail every filter.
                if crate::platform::platform_by_id(&claim.platform).is_none() {
                    let folded = claim.platform.trim().to_lowercase();
                    if let Some(registered) = crate::platform::PLATFORMS
                        .iter()
                        .find(|platform| platform.id.to_lowercase() == folded)
                    {
                        return Err(invalid(
                            &format!("{field}.platform"),
                            &format!(
                                "{:?} is a case or whitespace variant of registered platform id {:?}; use the exact id",
                                claim.platform, registered.id
                            ),
                        ));
                    }
                }
                let evidence = match &claim.evidence {
                    PlatformClaimEvidence::Extension(value)
                    | PlatformClaimEvidence::Filename(value)
                    | PlatformClaimEvidence::ReleaseMetadata(value)
                    | PlatformClaimEvidence::StructuralInspection(value) => value,
                };
                text(evidence, &format!("{field}.evidence"), true)?;
            }
        }
    }
    Ok(())
}
