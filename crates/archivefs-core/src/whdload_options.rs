//! Read-only, typed projection of WHDLoad ToolTypes and slave Config metadata.
//!
//! This module deliberately stops at explanation.  It never writes an `.info`
//! file, a ToolType, a startup script, or an emulator configuration.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::identity_source::whdload::SlaveArtifact;

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
    pub allowed_values: Vec<String>,
    pub range: Option<(i64, i64)>,
    pub default_value: Option<String>,
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
            let metadata_present = unambiguous_metadata(slot, metadata).is_some();
            (
                WhdloadOptionKind::Custom(slot),
                Some(WhdloadOptionValue::Text(value.into())),
                if value.is_empty() {
                    WhdloadOptionApplicability::Malformed
                } else if metadata_present {
                    WhdloadOptionApplicability::Applicable
                } else {
                    WhdloadOptionApplicability::Unknown
                },
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
            if value.is_empty() {
                WhdloadOptionApplicability::Malformed
            } else {
                WhdloadOptionApplicability::Applicable
            },
        )
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

/// Parses only the stable `C1:Type:Label:Spec;` shape. Malformed records are
/// retained as metadata entries rather than discarded.
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
        if fields.len() < 3 || slot.is_none() || fields[1].len() != 1 {
            diagnostics.push(format!("malformed WHDLoad Config record: {raw}"));
            records.push(WhdloadCustomMetadata {
                slot: slot.unwrap_or(0),
                option_type: fields.get(1).copied().unwrap_or_default().into(),
                label: None,
                allowed_values: Vec::new(),
                range: None,
                default_value: None,
                description: None,
                evidence: WhdloadOptionEvidence {
                    source: WhdloadOptionEvidenceSource::SlaveConfig,
                    raw: raw.into(),
                    detail: "Config syntax is not safely parseable".into(),
                },
                applicability: WhdloadOptionApplicability::Malformed,
            });
            continue;
        }
        let spec = fields
            .get(3)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        let (allowed_values, range, default_value) = parse_explicit_spec(spec);
        let label = (!fields[2].trim().is_empty()).then(|| fields[2].trim().to_string());
        records.push(WhdloadCustomMetadata {
            slot: slot.unwrap(),
            option_type: fields[1].into(),
            label: label.clone(),
            allowed_values,
            range,
            default_value,
            description: label,
            evidence: WhdloadOptionEvidence {
                source: WhdloadOptionEvidenceSource::SlaveConfig,
                raw: raw.into(),
                detail: "supplied Config metadata".into(),
            },
            applicability: WhdloadOptionApplicability::Applicable,
        });
    }
    (records, diagnostics)
}

fn parse_explicit_spec(spec: Option<&str>) -> (Vec<String>, Option<(i64, i64)>, Option<String>) {
    let Some(spec) = spec else {
        return (Vec::new(), None, None);
    };
    let default = Some(spec.to_string());
    if spec.starts_with('[') && spec.ends_with(']') {
        return (
            spec[1..spec.len() - 1]
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .collect(),
            None,
            default,
        );
    }
    if let Some((low, high)) = spec
        .split_once("..=")
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
    {
        return (Vec::new(), Some((low, high)), default);
    }
    (Vec::new(), None, default)
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
    fn custom_label_and_explicit_values_are_retained() {
        let projection = project_whdload_options(
            &slave(Some("C1:X:Difficulty:[Easy,Hard];")),
            "/games/Example".into(),
            None,
            &["CUSTOM1=Hard".into()],
        );
        let custom = option(&projection, "CUSTOM1");
        assert_eq!(
            custom.custom_metadata.as_ref().unwrap().label.as_deref(),
            Some("Difficulty")
        );
        assert_eq!(
            custom.custom_metadata.as_ref().unwrap().allowed_values,
            vec!["Easy", "Hard"]
        );
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
}
