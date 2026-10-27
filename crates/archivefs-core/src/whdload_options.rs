//! Read-only, typed projection of WHDLoad ToolTypes and slave Config metadata.
//!
//! This module deliberately stops at explanation.  It never writes an `.info`
//! file, a ToolType, a startup script, or an emulator configuration.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::identity_source::whdload::SlaveArtifact;

/// Choices read from one Config record; same bound as the trainer path.
const MAX_CONFIG_CHOICES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhdloadOptionKind {
    Pal,
    Ntsc,
    Preload,
    QuitKey,
    Custom(u8),
    Config,
    NoCache,
    NoWriteCache,
    NoWriteInfo,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhdloadOptionValue {
    RegionPal,
    RegionNtsc,
    PreloadDisabled,
    Preload,
    PreloadSmart,
    Flag(bool),
    KeyCode(String),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhdloadOptionApplicability {
    /// A supported ToolType was parsed and has one unambiguous meaning.
    Applicable,
    /// The option is the documented default established by its absence.
    Default,
    /// The key is not in the supported semantic set.
    Unknown,
    /// The record was present but could not be safely parsed.
    Malformed,
    /// More than one incompatible declaration was supplied.
    Conflicting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhdloadOptionEvidenceSource {
    ToolType,
    SlaveConfig,
    DocumentedDefault,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhdloadOptionEvidence {
    pub source: WhdloadOptionEvidenceSource,
    pub raw: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhdloadCustomMetadata {
    pub slot: u8,
    pub option_type: String,
    pub label: Option<String>,
    /// Choice labels in declaration order (`L` and `M` only). The position is
    /// the value WHDLoad expects; no choice is a default.
    pub allowed_values: Vec<String>,
    /// Inclusive bit range from a valid `X` bit number or `M` range.
    pub bit_range: Option<(u8, u8)>,
    /// The type-specific fourth field, verbatim. It is a choice list or bit
    /// number depending on `option_type`, never a default value.
    pub spec: Option<String>,
    pub description: Option<String>,
    pub evidence: WhdloadOptionEvidence,
    pub applicability: WhdloadOptionApplicability,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhdloadOption {
    pub key: String,
    pub kind: WhdloadOptionKind,
    pub value: Option<WhdloadOptionValue>,
    pub applicability: WhdloadOptionApplicability,
    pub evidence: WhdloadOptionEvidence,
    pub custom_metadata: Option<WhdloadCustomMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhdloadOptionProjection {
    pub install_path: PathBuf,
    pub slave_path: PathBuf,
    pub slave_name: String,
    pub slave_sha256: Option<String>,
    pub game_identity: Option<String>,
    pub options: Vec<WhdloadOption>,
    pub config_metadata: Vec<WhdloadCustomMetadata>,
    pub diagnostics: Vec<String>,
}

/// Projects a discovered local slave and its already-read ToolTypes.  The
/// caller supplies the install identity because a filename alone is not game
/// identity.  `tooltypes` are raw `.info` records; no file is opened here.
pub fn project_whdload_options(
    slave: &SlaveArtifact,
    install_path: PathBuf,
    game_identity: Option<String>,
    tooltypes: &[String],
) -> WhdloadOptionProjection {
    let config = slave.parsed.config.as_deref().unwrap_or_default();
    let (options, config_metadata, diagnostics) = project_supplied_options(tooltypes, config);
    WhdloadOptionProjection {
        install_path,
        slave_path: slave.path.clone(),
        slave_name: slave.name.clone(),
        slave_sha256: Some(slave.hashes.sha256.clone()),
        game_identity,
        options,
        config_metadata,
        diagnostics,
    }
}

/// Explain supplied text only; no install/game identity or launch authority.
pub fn project_supplied_options(
    tooltypes: &[String],
    config: &str,
) -> (Vec<WhdloadOption>, Vec<WhdloadCustomMetadata>, Vec<String>) {
    let (config_metadata, mut diagnostics) = parse_config_metadata(config);
    let mut options = Vec::new();

    for raw in tooltypes {
        options.push(parse_tooltype(raw, &config_metadata));
    }

    if !options
        .iter()
        .any(|option| option.kind == WhdloadOptionKind::Preload)
    {
        options.push(WhdloadOption {
            key: "PRELOAD".into(),
            kind: WhdloadOptionKind::Preload,
            value: Some(WhdloadOptionValue::PreloadDisabled),
            applicability: WhdloadOptionApplicability::Default,
            evidence: WhdloadOptionEvidence {
                source: WhdloadOptionEvidenceSource::DocumentedDefault,
                raw: String::new(),
                detail: "Neither PRELOAD nor PRELOADSMART is present".into(),
            },
            custom_metadata: None,
        });
    }

    mark_conflicts(&mut options, &mut diagnostics);
    (options, config_metadata, diagnostics)
}

fn unambiguous_metadata(
    slot: u8,
    metadata: &[WhdloadCustomMetadata],
) -> Option<&WhdloadCustomMetadata> {
    let mut matching = metadata.iter().filter(|item| item.slot == slot);
    let first = matching.next()?;
    (first.applicability == WhdloadOptionApplicability::Applicable && matching.next().is_none())
        .then_some(first)
}

fn parse_tooltype(raw: &str, metadata: &[WhdloadCustomMetadata]) -> WhdloadOption {
    let trimmed = raw.trim();
    let (key_raw, value_raw) = trimmed.split_once('=').unwrap_or((trimmed, ""));
    let key = key_raw.trim().to_ascii_uppercase();
    let value = value_raw.trim();
    let evidence = |detail: &str| WhdloadOptionEvidence {
        source: WhdloadOptionEvidenceSource::ToolType,
        raw: trimmed.into(),
        detail: detail.into(),
    };
    let (kind, parsed, applicability) = match key.as_str() {
        "PAL" => (
            WhdloadOptionKind::Pal,
            Some(WhdloadOptionValue::RegionPal),
            flag_status(value),
        ),
        "NTSC" => (
            WhdloadOptionKind::Ntsc,
            Some(WhdloadOptionValue::RegionNtsc),
            flag_status(value),
        ),
        "PRELOAD" => (
            WhdloadOptionKind::Preload,
            Some(WhdloadOptionValue::Preload),
            flag_status(value),
        ),
        "PRELOADSMART" => (
            WhdloadOptionKind::Preload,
            Some(WhdloadOptionValue::PreloadSmart),
            flag_status(value),
        ),
        "QUITKEY" => parse_text_option(WhdloadOptionKind::QuitKey, value, true),
        "CONFIG" => parse_text_option(WhdloadOptionKind::Config, value, false),
        "NOCACHE" => (
            WhdloadOptionKind::NoCache,
            Some(WhdloadOptionValue::Flag(true)),
            flag_status(value),
        ),
        "NOWRITECACHE" => (
            WhdloadOptionKind::NoWriteCache,
            Some(WhdloadOptionValue::Flag(true)),
            flag_status(value),
        ),
        "NOWRITEINFO" => (
            WhdloadOptionKind::NoWriteInfo,
            Some(WhdloadOptionValue::Flag(true)),
            flag_status(value),
        ),
        _ if custom_slot(&key).is_some() => {
            let slot = custom_slot(&key).unwrap();
            (
                WhdloadOptionKind::Custom(slot),
                Some(WhdloadOptionValue::Text(value.into())),
                custom_status(value, unambiguous_metadata(slot, metadata)),
            )
        }
        _ if key.is_empty() => (
            WhdloadOptionKind::Unknown(key.clone()),
            None,
            WhdloadOptionApplicability::Malformed,
        ),
        _ => (
            WhdloadOptionKind::Unknown(key.clone()),
            Some(WhdloadOptionValue::Text(value.into())),
            WhdloadOptionApplicability::Unknown,
        ),
    };
    let custom_metadata = match kind {
        WhdloadOptionKind::Custom(slot) => unambiguous_metadata(slot, metadata).cloned(),
        _ => None,
    };
    WhdloadOption {
        key: key.clone(),
        kind,
        value: parsed,
        applicability,
        evidence: evidence("supplied ToolType text"),
        custom_metadata,
    }
}

fn parse_text_option(
    kind: WhdloadOptionKind,
    value: &str,
    key_required: bool,
) -> (
    WhdloadOptionKind,
    Option<WhdloadOptionValue>,
    WhdloadOptionApplicability,
) {
    if key_required && value.is_empty() {
        (kind, None, WhdloadOptionApplicability::Malformed)
    } else {
        (
            kind,
            Some(if key_required {
                WhdloadOptionValue::KeyCode(value.into())
            } else {
                WhdloadOptionValue::Text(value.into())
            }),
            if value.is_empty() || (key_required && parse_whdload_number(value).is_none()) {
                WhdloadOptionApplicability::Malformed
            } else {
                WhdloadOptionApplicability::Applicable
            },
        )
    }
}

/// WHDLoad numeric ToolTypes are decimal or `$`-prefixed hexadecimal.
fn parse_whdload_number(value: &str) -> Option<u32> {
    match value.strip_prefix('$') {
        Some(hex) if hex.bytes().all(|b| b.is_ascii_hexdigit()) && !hex.is_empty() => {
            u32::from_str_radix(hex, 16).ok()
        }
        Some(_) => None,
        None if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
            value.parse().ok()
        }
        None => None,
    }
}

/// A supplied CUSTOM value is numeric whatever its declaration says, so bad
/// text is malformed even with no metadata. Meaning requires one valid
/// declaration, and an `L` value must index one of its choices.
fn custom_status(
    value: &str,
    metadata: Option<&WhdloadCustomMetadata>,
) -> WhdloadOptionApplicability {
    let Some(number) = parse_whdload_number(value) else {
        return WhdloadOptionApplicability::Malformed;
    };
    match metadata {
        None => WhdloadOptionApplicability::Unknown,
        Some(item) if item.option_type == "L" && number as usize >= item.allowed_values.len() => {
            WhdloadOptionApplicability::Malformed
        }
        Some(_) => WhdloadOptionApplicability::Applicable,
    }
}

fn flag_status(value: &str) -> WhdloadOptionApplicability {
    if value.is_empty() {
        WhdloadOptionApplicability::Applicable
    } else {
        WhdloadOptionApplicability::Malformed
    }
}

fn custom_slot(key: &str) -> Option<u8> {
    key.strip_prefix("CUSTOM")
        .and_then(|n| n.parse().ok())
        .filter(|n| (1..=5).contains(n))
}

fn mark_conflicts(options: &mut [WhdloadOption], diagnostics: &mut Vec<String>) {
    for index in 0..options.len() {
        for other in index + 1..options.len() {
            let conflict = matches!(
                (&options[index].kind, &options[other].kind),
                (WhdloadOptionKind::Pal, WhdloadOptionKind::Ntsc)
                    | (WhdloadOptionKind::Ntsc, WhdloadOptionKind::Pal)
            ) || options[index].key == options[other].key
                && options[index].value != options[other].value;
            if conflict {
                options[index].applicability = WhdloadOptionApplicability::Conflicting;
                options[other].applicability = WhdloadOptionApplicability::Conflicting;
                diagnostics.push(format!(
                    "conflicting WHDLoad ToolTypes: {} and {}",
                    options[index].evidence.raw, options[other].evidence.raw
                ));
            }
        }
    }
}

/// Parses only the stable `C1:Type:Label:Spec;` shape. Malformed or
/// unsupported records are retained as metadata entries rather than discarded,
/// and only a record whose type-specific spec is valid is `Applicable`.
pub fn parse_config_metadata(config: &str) -> (Vec<WhdloadCustomMetadata>, Vec<String>) {
    let mut records = Vec::new();
    let mut diagnostics = Vec::new();
    for raw in config
        .split(';')
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        let fields: Vec<_> = raw.splitn(4, ':').collect();
        let slot = fields
            .first()
            .and_then(|field| field.strip_prefix('C'))
            .and_then(|n| n.parse().ok())
            .filter(|n| (1..=5).contains(n));
        let evidence = |detail: &str| WhdloadOptionEvidence {
            source: WhdloadOptionEvidenceSource::SlaveConfig,
            raw: raw.into(),
            detail: detail.into(),
        };
        if fields.len() < 3 || slot.is_none() || fields[1].len() != 1 {
            diagnostics.push(format!("malformed WHDLoad Config record: {raw}"));
            records.push(WhdloadCustomMetadata {
                slot: slot.unwrap_or(0),
                option_type: fields.get(1).copied().unwrap_or_default().into(),
                label: None,
                allowed_values: Vec::new(),
                bit_range: None,
                spec: None,
                description: None,
                evidence: evidence("Config syntax is not safely parseable"),
                applicability: WhdloadOptionApplicability::Malformed,
            });
            continue;
        }
        let spec = fields
            .get(3)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        let shape = declaration_shape(fields[1], spec);
        match shape.applicability {
            WhdloadOptionApplicability::Malformed => diagnostics.push(format!(
                "malformed WHDLoad Config record: {raw} ({})",
                shape.detail
            )),
            WhdloadOptionApplicability::Unknown => diagnostics.push(format!(
                "unsupported WHDLoad Config type {}: {raw}",
                fields[1]
            )),
            _ => {}
        }
        let label = (!fields[2].trim().is_empty()).then(|| fields[2].trim().to_string());
        records.push(WhdloadCustomMetadata {
            slot: slot.unwrap(),
            option_type: fields[1].into(),
            label: label.clone(),
            allowed_values: shape.choices,
            bit_range: shape.bit_range,
            spec: spec.map(str::to_string),
            description: label,
            evidence: evidence(shape.detail),
            applicability: shape.applicability,
        });
    }
    (records, diagnostics)
}

struct DeclarationShape {
    applicability: WhdloadOptionApplicability,
    choices: Vec<String>,
    bit_range: Option<(u8, u8)>,
    detail: &'static str,
}

/// Interprets the fourth field by declared type, matching the semantics the
/// trainer path already applies: `X` is a bit number, `L` a choice list, `M`
/// a choice list plus bit range. Nothing here is a default value.
fn declaration_shape(option_type: &str, spec: Option<&str>) -> DeclarationShape {
    let shape = |applicability, choices, bit_range, detail| DeclarationShape {
        applicability,
        choices,
        bit_range,
        detail,
    };
    let malformed = |detail| {
        shape(
            WhdloadOptionApplicability::Malformed,
            Vec::new(),
            None,
            detail,
        )
    };
    let spec_text = spec.unwrap_or_default();
    match option_type {
        "B" => shape(
            WhdloadOptionApplicability::Applicable,
            Vec::new(),
            None,
            "supplied boolean Config metadata",
        ),
        "N" => shape(
            WhdloadOptionApplicability::Applicable,
            Vec::new(),
            None,
            "supplied numeric Config metadata",
        ),
        "X" => match spec_text.parse::<u8>().ok().filter(|bit| *bit <= 31) {
            Some(bit) => shape(
                WhdloadOptionApplicability::Applicable,
                Vec::new(),
                Some((bit, bit)),
                "supplied bit Config metadata",
            ),
            None => malformed("X requires a bit number from 0 to 31"),
        },
        "L" => {
            let choices = parse_choices(spec_text);
            if choices.is_empty() {
                malformed("L requires a comma-separated choice list")
            } else {
                shape(
                    WhdloadOptionApplicability::Applicable,
                    choices,
                    None,
                    "supplied list Config metadata",
                )
            }
        }
        "M" => {
            let (labels, range) = spec_text
                .split_once(':')
                .map_or((spec_text, None), |(a, b)| (a, parse_bit_range(b)));
            let choices = parse_choices(labels);
            match range {
                Some(range) if !choices.is_empty() => shape(
                    WhdloadOptionApplicability::Applicable,
                    choices,
                    Some(range),
                    "supplied multi-bit Config metadata",
                ),
                _ => malformed("M requires choices and a bit range such as 0-1"),
            }
        }
        _ => shape(
            WhdloadOptionApplicability::Unknown,
            Vec::new(),
            None,
            "Config type is not supported; no option meaning is established",
        ),
    }
}

fn parse_choices(spec: &str) -> Vec<String> {
    spec.split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .take(MAX_CONFIG_CHOICES)
        .map(str::to_string)
        .collect()
}

fn parse_bit_range(value: &str) -> Option<(u8, u8)> {
    let (low, high) = value.split_once('-')?;
    let low = low.trim().parse::<u8>().ok()?;
    let high = high.trim().parse::<u8>().ok()?;
    (low <= high && high <= 31 && high - low >= 1).then_some((low, high))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::whdload::{ParsedWHDLoadSlave, SlaveHashes};

    fn slave(config: Option<&str>) -> SlaveArtifact {
        SlaveArtifact {
            path: "/games/Example/Example.slave".into(),
            name: "Example.slave".into(),
            size_bytes: 42,
            parsed: ParsedWHDLoadSlave {
                runtime_version: 20,
                struct_size: 54,
                flags: 0,
                base_mem_size: 0,
                exec_install: 0,
                game_loader: 0,
                current_dir: None,
                dont_cache: None,
                key_debug: None,
                key_exit: None,
                exp_mem: None,
                name: Some("Example".into()),
                copyright: None,
                info: None,
                kick_name: None,
                kick_size: None,
                kick_crc: None,
                config: config.map(str::to_string),
                extension_bytes: Vec::new(),
            },
            hashes: SlaveHashes {
                sha1: "sha1".into(),
                sha256: "sha256".into(),
            },
        }
    }

    fn option<'a>(projection: &'a WhdloadOptionProjection, key: &str) -> &'a WhdloadOption {
        projection
            .options
            .iter()
            .find(|option| option.key == key)
            .unwrap()
    }

    #[test]
    fn projects_region_preload_quitkey_and_identity() {
        let projection = project_whdload_options(
            &slave(None),
            "/games/Example".into(),
            Some("known-id".into()),
            &["PAL".into(), "PRELOAD".into(), "QUITKEY=$59".into()],
        );
        assert_eq!(
            option(&projection, "PAL").value,
            Some(WhdloadOptionValue::RegionPal)
        );
        assert_eq!(
            option(&projection, "PRELOAD").value,
            Some(WhdloadOptionValue::Preload)
        );
        assert_eq!(
            option(&projection, "QUITKEY").value,
            Some(WhdloadOptionValue::KeyCode("$59".into()))
        );
        assert_eq!(projection.game_identity.as_deref(), Some("known-id"));
        assert_eq!(projection.slave_sha256.as_deref(), Some("sha256"));
    }

    #[test]
    fn projects_ntsc_and_preload_smart_without_choosing_region() {
        let projection = project_whdload_options(
            &slave(None),
            "/games/Example".into(),
            None,
            &["NTSC".into(), "PRELOADSMART".into()],
        );
        assert_eq!(
            option(&projection, "NTSC").value,
            Some(WhdloadOptionValue::RegionNtsc)
        );
        assert_eq!(
            option(&projection, "PRELOADSMART").value,
            Some(WhdloadOptionValue::PreloadSmart)
        );
        assert!(
            !projection
                .options
                .iter()
                .any(|item| item.value == Some(WhdloadOptionValue::PreloadDisabled))
        );
    }

    #[test]
    fn custom_label_and_list_choices_are_retained_without_a_default() {
        let projection = project_whdload_options(
            &slave(Some("C1:L:Difficulty:Easy,Hard;")),
            "/games/Example".into(),
            None,
            &["CUSTOM1=1".into()],
        );
        let custom = option(&projection, "CUSTOM1");
        let metadata = custom.custom_metadata.as_ref().unwrap();
        assert_eq!(custom.applicability, WhdloadOptionApplicability::Applicable);
        assert_eq!(metadata.label.as_deref(), Some("Difficulty"));
        assert_eq!(metadata.allowed_values, vec!["Easy", "Hard"]);
        assert_eq!(metadata.spec.as_deref(), Some("Easy,Hard"));
        assert_eq!(metadata.bit_range, None);
    }

    #[test]
    fn unknown_custom_meaning_is_conservative_and_unknown_tooltypes_visible() {
        let projection = project_whdload_options(
            &slave(None),
            "/games/Example".into(),
            None,
            &["CUSTOM1=7".into(), "FUTUREFLAG=on".into()],
        );
        assert_eq!(option(&projection, "CUSTOM1").custom_metadata, None);
        assert_eq!(
            option(&projection, "CUSTOM1").applicability,
            WhdloadOptionApplicability::Unknown
        );
        assert_eq!(
            option(&projection, "FUTUREFLAG").applicability,
            WhdloadOptionApplicability::Unknown
        );
    }

    #[test]
    fn malformed_config_does_not_panic_and_is_visible() {
        let projection = project_whdload_options(
            &slave(Some("C1:X;C2:Q")),
            "/games/Example".into(),
            None,
            &[],
        );
        assert_eq!(
            projection
                .config_metadata
                .iter()
                .filter(|item| item.applicability == WhdloadOptionApplicability::Malformed)
                .count(),
            2
        );
        assert_eq!(projection.diagnostics.len(), 2);
    }

    #[test]
    fn duplicate_and_region_conflicts_are_explicit() {
        let projection = project_whdload_options(
            &slave(None),
            "/games/Example".into(),
            None,
            &["PAL".into(), "NTSC".into(), "PAL".into()],
        );
        assert!(
            projection
                .options
                .iter()
                .filter(|option| option.applicability == WhdloadOptionApplicability::Conflicting)
                .count()
                >= 3
        );
        assert!(!projection.diagnostics.is_empty());
    }

    #[test]
    fn cache_switches_are_supported_conservatively() {
        let projection = project_whdload_options(
            &slave(None),
            "/games/Example".into(),
            None,
            &[
                "NoCache".into(),
                "NoWriteCache".into(),
                "NoWriteInfo".into(),
            ],
        );
        assert_eq!(
            option(&projection, "NOCACHE").kind,
            WhdloadOptionKind::NoCache
        );
        assert_eq!(
            option(&projection, "NOWRITECACHE").kind,
            WhdloadOptionKind::NoWriteCache
        );
        assert_eq!(
            option(&projection, "NOWRITEINFO").kind,
            WhdloadOptionKind::NoWriteInfo
        );
    }
    #[test]
    fn malformed_or_duplicate_custom_definitions_cannot_establish_meaning() {
        for config in ["C1:B", "C1:B:One;C1:B:Two;", "C1:B:One;C1:B"] {
            let (options, _, _) = project_supplied_options(&["CUSTOM1=1".into()], config);
            let option = options.iter().find(|o| o.key == "CUSTOM1").unwrap();
            assert_eq!(option.applicability, WhdloadOptionApplicability::Unknown);
            assert!(option.custom_metadata.is_none());
            assert!(!option.evidence.detail.contains("discovered"));
        }
    }

    fn config_status(config: &str) -> (WhdloadOptionApplicability, usize) {
        let (metadata, diagnostics) = parse_config_metadata(config);
        (metadata[0].applicability.clone(), diagnostics.len())
    }

    fn custom_status_for(config: &str, tooltype: &str) -> WhdloadOptionApplicability {
        let (options, _, _) = project_supplied_options(&[tooltype.into()], config);
        options
            .into_iter()
            .find(|o| matches!(o.kind, WhdloadOptionKind::Custom(_)))
            .unwrap()
            .applicability
    }

    // P2 finding 1: an unsupported or incomplete Config type is not meaning.
    #[test]
    fn unsupported_config_type_is_not_applicable() {
        let (status, diagnostics) = config_status("C1:Q:Unknown;");
        assert_eq!(status, WhdloadOptionApplicability::Unknown);
        assert_eq!(diagnostics, 1);
        assert_eq!(
            custom_status_for("C1:Q:Unknown;", "CUSTOM1=1"),
            WhdloadOptionApplicability::Unknown
        );
        let (options, _, _) = project_supplied_options(&["CUSTOM1=1".into()], "C1:Q:Unknown;");
        assert!(options[0].custom_metadata.is_none());
    }

    #[test]
    fn x_declaration_requires_a_valid_bit_number() {
        for config in [
            "C1:X:Lives;",
            "C1:X:Lives:;",
            "C1:X:Lives:abc;",
            "C1:X:Lives:32;",
            "C1:X:Lives:-1;",
        ] {
            let (status, diagnostics) = config_status(config);
            assert_eq!(status, WhdloadOptionApplicability::Malformed, "{config}");
            assert_eq!(diagnostics, 1, "{config}");
            assert_eq!(
                custom_status_for(config, "CUSTOM1=1"),
                WhdloadOptionApplicability::Unknown,
                "{config}"
            );
        }
    }

    // P2 finding 3: a spec is type-specific, never a default value.
    #[test]
    fn x_spec_is_a_bit_number_and_l_spec_is_a_choice_list() {
        let (metadata, diagnostics) =
            parse_config_metadata("C2:X:Lives:3;C1:L:Difficulty:Easy,Hard;");
        assert!(diagnostics.is_empty());
        let x = metadata.iter().find(|m| m.slot == 2).unwrap();
        assert_eq!(x.applicability, WhdloadOptionApplicability::Applicable);
        assert_eq!(x.bit_range, Some((3, 3)));
        assert!(x.allowed_values.is_empty());
        let l = metadata.iter().find(|m| m.slot == 1).unwrap();
        assert_eq!(l.allowed_values, vec!["Easy", "Hard"]);
        assert_eq!(l.bit_range, None);
    }

    #[test]
    fn m_declaration_needs_choices_and_a_bit_range() {
        let (metadata, _) = parse_config_metadata("C1:M:Mode:A,B,C:0-1;");
        assert_eq!(
            metadata[0].applicability,
            WhdloadOptionApplicability::Applicable
        );
        assert_eq!(metadata[0].allowed_values, vec!["A", "B", "C"]);
        assert_eq!(metadata[0].bit_range, Some((0, 1)));
        for config in [
            "C1:M:Mode:A,B,C;",
            "C1:M:Mode:A,B:5-2;",
            "C1:M:Mode::0-1;",
            "C1:L:Pick;",
        ] {
            assert_eq!(
                config_status(config).0,
                WhdloadOptionApplicability::Malformed,
                "{config}"
            );
        }
    }

    // P2 finding 2: CUSTOM and QUITKEY values must be WHDLoad numbers.
    #[test]
    fn non_numeric_custom_value_is_malformed_even_with_valid_metadata() {
        assert_eq!(
            custom_status_for("C1:B:Practice;", "CUSTOM1=banana"),
            WhdloadOptionApplicability::Malformed
        );
        assert_eq!(
            custom_status_for("", "CUSTOM1=banana"),
            WhdloadOptionApplicability::Malformed
        );
        for bad in [
            "CUSTOM1=$",
            "CUSTOM1=$zz",
            "CUSTOM1=-1",
            "CUSTOM1=0x10",
            "CUSTOM1=1 2",
        ] {
            assert_eq!(
                custom_status_for("C1:B:Practice;", bad),
                WhdloadOptionApplicability::Malformed,
                "{bad}"
            );
        }
        for good in ["CUSTOM1=1", "CUSTOM1=$10"] {
            assert_eq!(
                custom_status_for("C1:B:Practice;", good),
                WhdloadOptionApplicability::Applicable,
                "{good}"
            );
        }
    }

    #[test]
    fn list_value_must_index_a_declared_choice() {
        let config = "C1:L:Difficulty:Easy,Hard;";
        assert_eq!(
            custom_status_for(config, "CUSTOM1=1"),
            WhdloadOptionApplicability::Applicable
        );
        assert_eq!(
            custom_status_for(config, "CUSTOM1=2"),
            WhdloadOptionApplicability::Malformed
        );
        assert_eq!(
            custom_status_for(config, "CUSTOM1=Hard"),
            WhdloadOptionApplicability::Malformed
        );
    }

    #[test]
    fn quitkey_must_be_a_decimal_or_hex_number() {
        for (raw, expected) in [
            ("QUITKEY=banana", WhdloadOptionApplicability::Malformed),
            ("QUITKEY=$", WhdloadOptionApplicability::Malformed),
            ("QUITKEY=-5", WhdloadOptionApplicability::Malformed),
            ("QUITKEY=$59", WhdloadOptionApplicability::Applicable),
            ("QUITKEY=89", WhdloadOptionApplicability::Applicable),
        ] {
            let (options, _, _) = project_supplied_options(&[raw.into()], "");
            assert_eq!(options[0].applicability, expected, "{raw}");
            assert!(
                options[0].value.is_some() || expected == WhdloadOptionApplicability::Malformed
            );
        }
    }
}
