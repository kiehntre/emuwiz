//! Read-only conversion-tool capability inventory and per-item eligibility.

use crate::storage_health::{
    ConversionEligibility, StorageConversionCapability, StorageFormatClass, StorageHealthItem,
    StorageOpportunityKind, StorageRoundTripClass,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolCapabilityStatus {
    Found,
    Missing,
    VersionUnknown,
    VersionSupported,
    VersionTooOld,
    CapabilityUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversionToolRecord {
    pub name: String,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub status: ToolCapabilityStatus,
    pub capabilities: Vec<String>,
    pub capability_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ConversionToolInventory {
    pub tools: Vec<ConversionToolRecord>,
    /// The chdman the verified CUE/BIN converter itself would run. CHD
    /// eligibility follows this, never the PATH lookup in `tools`.
    #[serde(default)]
    pub reviewed_chdman: ChdmanBinding,
}

/// The converter's reviewed chdman binding, judged by the converter's own check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(tag = "state", content = "detail", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChdmanBinding {
    /// The converter would run this file.
    Usable(PathBuf),
    /// Nothing is installed at the reviewed path.
    #[default]
    Absent,
    /// Something is there, but the converter refuses it (for example a symlink
    /// or a file without execute permission).
    Unusable(String),
}

/// Judge `path` exactly as the converter judges its chdman binding. Reads
/// metadata only; nothing is executed.
pub fn chdman_binding_at(path: &Path) -> ChdmanBinding {
    // Mirrors the converter's own lookup: a missing (or dangling) reviewed
    // path means no binding, anything else must pass its executable check.
    if !path.exists() {
        return ChdmanBinding::Absent;
    }
    match crate::repair::optical_conversion::check_chdman_binding(path) {
        Ok(path) => ChdmanBinding::Usable(path),
        Err(error) => ChdmanBinding::Unusable(error.to_string()),
    }
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(PathBuf::from)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}

fn probe(name: &str, args: &[&str], capabilities: &[&str]) -> ConversionToolRecord {
    let path = which(name);
    let Some(path) = path.clone() else {
        return ConversionToolRecord {
            name: name.into(),
            path: None,
            version: None,
            status: ToolCapabilityStatus::Missing,
            capabilities: Vec::new(),
            capability_source: "PATH lookup".into(),
        };
    };
    let output = Command::new("timeout")
        .arg("2")
        .arg(&path)
        .args(args)
        .output();
    let text = output.ok().map(|o| {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    });
    let version = text.as_deref().and_then(|t| {
        t.lines()
            .find(|line| line.to_ascii_lowercase().contains(name))
            .map(str::trim)
            .map(str::to_owned)
    });
    ConversionToolRecord {
        name: name.into(),
        path: Some(path),
        version,
        status: if text.is_some() {
            ToolCapabilityStatus::Found
        } else {
            ToolCapabilityStatus::VersionUnknown
        },
        capabilities: capabilities.iter().map(|s| (*s).into()).collect(),
        capability_source: "bounded --version/help probe".into(),
    }
}

pub fn probe_conversion_tools() -> ConversionToolInventory {
    ConversionToolInventory {
        reviewed_chdman: chdman_binding_at(Path::new(
            crate::repair::optical_conversion::REVIEWED_CHDMAN_PATH,
        )),
        tools: vec![
            probe(
                "chdman",
                &["--version"],
                &["createcd", "createdvd", "extractcd", "extractdvd", "info"],
            ),
            probe(
                "dolphin-tool",
                &["--help"],
                &[
                    "inspect",
                    "convert-to-rvz",
                    "convert-to-iso",
                    "compression-settings",
                ],
            ),
            probe(
                "maxcso",
                &["--help"],
                &["iso-to-cso", "cso-to-iso", "--measure", "cso1"],
            ),
        ],
    }
}

fn tool<'a>(
    inventory: &'a ConversionToolInventory,
    name: &str,
) -> Option<&'a ConversionToolRecord> {
    inventory.tools.iter().find(|t| t.name == name)
}

pub fn capability_for_item(
    item: &StorageHealthItem,
    inventory: &ConversionToolInventory,
) -> StorageConversionCapability {
    let target = item.opportunity.target_format;
    if matches!(
        item.opportunity.kind,
        StorageOpportunityKind::AlreadyEfficient
    ) {
        return StorageConversionCapability {
            eligibility: ConversionEligibility::FormatAlreadyEfficient,
            tool: None,
            tool_path: None,
            tool_version: None,
            target_format: None,
            mode: None,
            options: Vec::new(),
            round_trip: item.opportunity.round_trip,
            verification_required: "No conversion recommended.".into(),
            eligibility_detail: None,
            savings_measurement: "Not measured.".into(),
        };
    }
    let (name, mode, options, round_trip, verification) = match (item.format, target) {
        // The only CHD route: storage health sets this target solely for a
        // sheet the verified converter admits. GDI and ISO have no verified
        // converter, and a CD image must never be offered `createdvd`.
        (StorageFormatClass::BinCue, Some(StorageFormatClass::Chd)) => (
            "chdman",
            "createcd",
            vec!["--compression cdlz,cdzl,cdfl".into(), "--hunksize 8".into()],
            StorageRoundTripClass::ContentEquivalent,
            "Canonical optical fingerprint of the source and the CHD, plus the CHD's single-track layout metadata and hunk map.",
        ),
        (StorageFormatClass::Iso, Some(StorageFormatClass::Rvz)) => (
            "dolphin-tool",
            "convert-to-rvz",
            vec!["zstd".into(), "128 KiB blocks".into(), "level 5".into()],
            StorageRoundTripClass::PlayableNotOriginalReconstructable,
            "Dolphin structural verification plus reconstructed ISO hash.",
        ),
        (StorageFormatClass::Iso, Some(StorageFormatClass::Cso)) => (
            "maxcso",
            "iso-to-cso",
            vec!["CSO1".into()],
            StorageRoundTripClass::PlayableNotOriginalReconstructable,
            "CSO header/block validation and reconstructed ISO hash.",
        ),
        _ => {
            return StorageConversionCapability {
                eligibility: ConversionEligibility::Unsupported,
                tool: None,
                tool_path: None,
                tool_version: None,
                target_format: target,
                mode: None,
                options: Vec::new(),
                round_trip: StorageRoundTripClass::NotEstablished,
                verification_required: "No evidence-backed conversion proof is established.".into(),
                eligibility_detail: None,
                savings_measurement: "Qualitative only.".into(),
            };
        }
    };
    let Some(record) = tool(inventory, name) else {
        unreachable!()
    };
    let topology_unproven = item.opportunity.topology_sensitive
        && item.opportunity.round_trip == StorageRoundTripClass::NotEstablished;
    let mut eligibility_detail = None;
    let mut tool_path = record.path.clone();
    let mut tool_version = record.version.clone();
    let eligibility = if name == "chdman" {
        // The converter runs only its reviewed binding, so that binding, not
        // whichever chdman PATH finds first, decides readiness.
        let reviewed = crate::repair::optical_conversion::REVIEWED_CHDMAN_PATH;
        let elsewhere = record
            .path
            .as_deref()
            .filter(|path| *path != Path::new(reviewed))
            .map(|path| format!(" The chdman at {} is not used.", path.display()));
        if record.path.as_deref() != Some(Path::new(reviewed)) {
            tool_version = None;
        }
        match &inventory.reviewed_chdman {
            ChdmanBinding::Absent => {
                tool_path = None;
                eligibility_detail = Some(format!(
                    "The converter runs chdman only from {reviewed}, and nothing is installed there.{}",
                    elsewhere.unwrap_or_default()
                ));
                ConversionEligibility::ToolMissing
            }
            ChdmanBinding::Unusable(reason) => {
                tool_path = Some(PathBuf::from(reviewed));
                eligibility_detail = Some(format!(
                    "The converter cannot use {reviewed}: {reason}.{}",
                    elsewhere.unwrap_or_default()
                ));
                ConversionEligibility::ReviewRequired
            }
            ChdmanBinding::Usable(path) => {
                tool_path = Some(path.clone());
                if topology_unproven {
                    ConversionEligibility::TopologyIncomplete
                } else {
                    ConversionEligibility::ConversionReady
                }
            }
        }
    } else if record.status == ToolCapabilityStatus::Missing {
        ConversionEligibility::ToolMissing
    } else if topology_unproven {
        ConversionEligibility::TopologyIncomplete
    } else if record.status == ToolCapabilityStatus::VersionUnknown {
        ConversionEligibility::ReviewRequired
    } else {
        ConversionEligibility::ConversionReady
    };
    StorageConversionCapability {
        eligibility,
        tool: Some(name.into()),
        tool_path,
        tool_version,
        target_format: Some(target.unwrap()),
        mode: Some(mode.into()),
        options,
        round_trip,
        verification_required: verification.into(),
        savings_measurement: if name == "maxcso" {
            "Use bounded maxcso --measure; no output is created.".into()
        } else {
            "Qualitative/range estimate only; no safe read-only measurement mode.".into()
        },
        eligibility_detail,
    }
}

#[cfg(test)]
mod tests {
    //! Regression fixtures from the 2026-10-09 OpenROM feature-delta audit.
    //!
    //! Unsafe behaviour prevented: offering a CHD "audio codec" choice that
    //! emits a compressor chdman does not have (`cdav`, sold as lossy audio).
    //!
    //! Scope: these tests cover *descriptive text only* - the option strings
    //! this module shows for a route, and the codec list recorded in the
    //! CUE/BIN policy. Neither is an executed command. The command chdman is
    //! actually given is `repair::optical_conversion::chdman_createcd_args`,
    //! which names no compressor; it is covered by the tests beside it.

    use super::*;
    use crate::storage_health::{
        StorageConfidence, StorageEstimate, StorageEstimateKind, StorageObjectKind,
        StorageOpportunity,
    };

    /// chdman's CD codecs (`createcd`). All lossless.
    const CD_CODECS: [&str; 4] = ["cdlz", "cdzl", "cdzs", "cdfl"];
    /// chdman's general codecs (`createdvd`, `createhd`, `createraw`). All lossless.
    const GENERAL_CODECS: [&str; 5] = ["zlib", "zstd", "lzma", "huff", "flac"];
    /// Identifiers that must never be generated: not chdman codecs, or lossy.
    const FORBIDDEN: [&str; 8] = [
        "cdav", "avhu", "vorbis", "ogg", "mp3", "opus", "aac", "lossy",
    ];

    fn assert_chdman_codecs(mode: &str, list: &str, context: &str) {
        let allowed: &[&str] = match mode {
            "createcd" => &CD_CODECS,
            "createdvd" => &GENERAL_CODECS,
            other => panic!("{context}: unexpected chdman mode {other:?}"),
        };
        let codecs: Vec<&str> = list.split(',').collect();
        assert!(
            (1..=4).contains(&codecs.len()),
            "{context}: chdman takes one to four codecs, got {list:?}"
        );
        for codec in codecs {
            assert!(
                !FORBIDDEN.contains(&codec),
                "{context}: {codec:?} is not a lossless chdman codec"
            );
            assert!(
                allowed.contains(&codec),
                "{context}: {codec:?} is not a chdman {mode} codec"
            );
        }
    }

    fn item(format: StorageFormatClass, target: StorageFormatClass) -> StorageHealthItem {
        StorageHealthItem {
            path: PathBuf::from("/synthetic/never-opened"),
            platform: None,
            format,
            object_kind: StorageObjectKind::OrdinaryFile,
            logical_size_bytes: Some(1),
            allocated_size_bytes: Some(1),
            filesystem_device: None,
            filesystem_inode: None,
            opportunity: StorageOpportunity {
                kind: StorageOpportunityKind::Compressible,
                target_format: Some(target),
                round_trip: StorageRoundTripClass::NotEstablished,
                confidence: StorageConfidence::Unknown,
                estimate: StorageEstimate {
                    kind: StorageEstimateKind::Unknown,
                    minimum_savings_bytes: None,
                    maximum_savings_bytes: None,
                    explanation: String::new(),
                },
                verification_required: true,
                topology_sensitive: false,
                cleanup_eligibility: String::new(),
                warning: None,
            },
            warnings: Vec::new(),
        }
    }

    /// A synthetic inventory: no tool is probed or executed by these tests.
    fn inventory() -> ConversionToolInventory {
        ConversionToolInventory {
            tools: ["chdman", "dolphin-tool", "maxcso"]
                .into_iter()
                .map(|name| ConversionToolRecord {
                    name: name.into(),
                    path: None,
                    version: None,
                    status: ToolCapabilityStatus::Missing,
                    capabilities: Vec::new(),
                    capability_source: "synthetic".into(),
                })
                .collect(),
            reviewed_chdman: ChdmanBinding::Absent,
        }
    }

    const FORMATS: [StorageFormatClass; 15] = [
        StorageFormatClass::Chd,
        StorageFormatClass::Rvz,
        StorageFormatClass::Wia,
        StorageFormatClass::Gcz,
        StorageFormatClass::Wbfs,
        StorageFormatClass::Cso,
        StorageFormatClass::Zso,
        StorageFormatClass::Iso,
        StorageFormatClass::BinCue,
        StorageFormatClass::Gdi,
        StorageFormatClass::Cdi,
        StorageFormatClass::Pbp,
        StorageFormatClass::Archive,
        StorageFormatClass::RawImage,
        StorageFormatClass::Unknown,
    ];

    #[test]
    fn regression_every_described_chdman_route_names_only_real_lossless_codecs() {
        let inventory = inventory();
        let mut chdman_routes = 0;
        for format in FORMATS {
            for target in FORMATS {
                let capability = capability_for_item(&item(format, target), &inventory);
                let context = format!("{format} -> {target}");
                for option in &capability.options {
                    let lower = option.to_ascii_lowercase();
                    for forbidden in FORBIDDEN {
                        assert!(
                            !lower.contains(forbidden),
                            "{context}: option {option:?} names {forbidden:?}"
                        );
                    }
                }
                if capability.tool.as_deref() != Some("chdman") {
                    continue;
                }
                chdman_routes += 1;
                let mode = capability.mode.as_deref().expect("a chdman mode");
                let lists: Vec<&str> = capability
                    .options
                    .iter()
                    .filter_map(|option| option.strip_prefix("--compression "))
                    .collect();
                assert_eq!(lists.len(), 1, "{context}: one explicit codec list");
                assert_chdman_codecs(mode, lists[0], &context);
            }
        }
        // Was 3 (CUE/BIN, GDI and ISO). GDI and ISO to CHD were withdrawn on
        // 2026-11-03: no verified converter exists for either, and the ISO
        // route named chdman's DVD mode for CD media. Only the CUE/BIN lane
        // that `repair::optical_conversion` verifies remains.
        assert_eq!(
            chdman_routes, 1,
            "CUE/BIN to CHD is the only advertised chdman route, because it is the only \
             one a verified converter backs; a new one must be added here deliberately"
        );
    }

    /// The policy's codec list is recorded text, not an argument: the command
    /// does not pass it. This only keeps that text to real lossless CD codecs.
    #[test]
    fn regression_the_recorded_chd_policy_text_names_only_lossless_cd_codecs() {
        let policy = &crate::repair::optical_conversion::SAFE_CUE_BIN_TO_CHD_POLICY;
        assert_chdman_codecs(policy.mode, policy.compression, "recorded CUE/BIN policy");
    }

    #[test]
    fn regression_the_codec_guard_itself_rejects_cdav() {
        for bad in [
            "cdlz,cdav",
            "cdav",
            "cdlz,vorbis",
            "cdlz,cdzl,cdfl,cdzs,cdlz",
        ] {
            assert!(
                std::panic::catch_unwind(|| assert_chdman_codecs("createcd", bad, "guard"))
                    .is_err(),
                "{bad:?} must be rejected"
            );
        }
        // General codecs are not CD codecs, and the reverse.
        assert!(
            std::panic::catch_unwind(|| assert_chdman_codecs("createcd", "flac", "guard")).is_err()
        );
        assert!(
            std::panic::catch_unwind(|| assert_chdman_codecs("createdvd", "cdfl", "guard"))
                .is_err()
        );
    }
}

#[cfg(test)]
mod eligibility_tests;
