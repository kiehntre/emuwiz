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

fn tool(inventory: &ConversionToolInventory, name: &str) -> Option<&ConversionToolRecord> {
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
            savings_measurement: "Not measured.".into(),
        };
    }
    let (name, mode, options, round_trip, verification) = match (item.format, target) {
        (StorageFormatClass::BinCue, Some(StorageFormatClass::Chd)) => (
            "chdman",
            "createcd",
            vec!["--compression cdlz,cdzl,cdfl".into(), "--hunksize 8".into()],
            StorageRoundTripClass::ContentEquivalent,
            "Track hashes, normalized CUE/topology, and canonical optical fingerprint.",
        ),
        (StorageFormatClass::Gdi, Some(StorageFormatClass::Chd)) => (
            "chdman",
            "createcd",
            vec!["--compression cdlz,cdzl,cdfl".into(), "--hunksize 8".into()],
            StorageRoundTripClass::ContentEquivalent,
            "Track hashes, normalized GDI topology, and GD-ROM fingerprint.",
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
        (StorageFormatClass::Iso, Some(StorageFormatClass::Chd)) => (
            "chdman",
            "createdvd",
            vec![
                "--compression lzma,zlib,huff,flac".into(),
                "--hunksize 2 sectors".into(),
            ],
            StorageRoundTripClass::ByteIdenticalExpected,
            "Proven DVD media classification and SHA-256 of reconstructed ISO.",
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
                savings_measurement: "Qualitative only.".into(),
            };
        }
    };
    let Some(record) = tool(inventory, name) else {
        unreachable!()
    };
    let eligibility = if record.status == ToolCapabilityStatus::Missing {
        ConversionEligibility::ToolMissing
    } else if item.opportunity.topology_sensitive
        && item.opportunity.round_trip == StorageRoundTripClass::NotEstablished
    {
        ConversionEligibility::TopologyIncomplete
    } else if record.status == ToolCapabilityStatus::VersionUnknown {
        ConversionEligibility::ReviewRequired
    } else {
        ConversionEligibility::ConversionReady
    };
    StorageConversionCapability {
        eligibility,
        tool: Some(name.into()),
        tool_path: record.path.clone(),
        tool_version: record.version.clone(),
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
    }
}
