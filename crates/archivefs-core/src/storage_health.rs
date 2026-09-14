//! Read-only storage health analysis over the existing library catalogue.
//!
//! This module deliberately does not convert, rewrite, move, delete, or hash
//! user content. It combines persisted catalogue facts with bounded metadata
//! reads and only offers conservative future opportunities.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::ingestion::cue_bin::resolve_cue_layout;
use crate::ingestion::gdi::resolve_gdi_all_tracks;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StorageFormatClass {
    Chd,
    Rvz,
    Wia,
    Gcz,
    Wbfs,
    Cso,
    Zso,
    Iso,
    BinCue,
    Gdi,
    Cdi,
    Pbp,
    Archive,
    RawImage,
    Unknown,
}

impl fmt::Display for StorageFormatClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Chd => "CHD",
            Self::Rvz => "RVZ",
            Self::Wia => "WIA",
            Self::Gcz => "GCZ",
            Self::Wbfs => "WBFS",
            Self::Cso => "CSO",
            Self::Zso => "ZSO",
            Self::Iso => "ISO",
            Self::BinCue => "BIN/CUE",
            Self::Gdi => "GDI",
            Self::Cdi => "CDI",
            Self::Pbp => "PBP",
            Self::Archive => "Archive",
            Self::RawImage => "Raw image",
            Self::Unknown => "Unknown",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StorageOpportunityKind {
    AlreadyEfficient,
    Compressible,
    PossibleDuplicateContent,
    TopologySensitive,
    UnsupportedFormat,
    DoNotConvert,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StorageRoundTripClass {
    ByteIdenticalExpected,
    ContentEquivalent,
    PlayableNotOriginalReconstructable,
    LossyOrUnsafe,
    NotEstablished,
}

impl fmt::Display for StorageRoundTripClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ByteIdenticalExpected => "byte-identical expected",
            Self::ContentEquivalent => "content-equivalent",
            Self::PlayableNotOriginalReconstructable => "playable, original not reconstructable",
            Self::LossyOrUnsafe => "lossy or unsafe",
            Self::NotEstablished => "not established",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StorageEstimateKind {
    ExactMeasured,
    EstimatedRange,
    RoughOpportunity,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StorageConfidence {
    High,
    Medium,
    Low,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StorageObjectKind {
    OrdinaryFile,
    Hardlink,
    Symlink,
    Missing,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageEstimate {
    pub kind: StorageEstimateKind,
    pub minimum_savings_bytes: Option<u64>,
    pub maximum_savings_bytes: Option<u64>,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageWarning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageOpportunity {
    pub kind: StorageOpportunityKind,
    pub target_format: Option<StorageFormatClass>,
    pub round_trip: StorageRoundTripClass,
    pub confidence: StorageConfidence,
    pub estimate: StorageEstimate,
    pub verification_required: bool,
    pub topology_sensitive: bool,
    pub cleanup_eligibility: String,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageHealthItem {
    pub path: PathBuf,
    pub platform: Option<String>,
    pub format: StorageFormatClass,
    pub object_kind: StorageObjectKind,
    pub logical_size_bytes: Option<u64>,
    pub allocated_size_bytes: Option<u64>,
    pub filesystem_device: Option<u64>,
    pub filesystem_inode: Option<u64>,
    pub opportunity: StorageOpportunity,
    pub warnings: Vec<StorageWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageHealthReport {
    pub items: Vec<StorageHealthItem>,
    pub total_logical_size_bytes: u64,
    pub total_allocated_size_bytes: Option<u64>,
    pub already_efficient_count: usize,
    pub candidate_count: usize,
    pub topology_sensitive_count: usize,
    pub duplicate_candidate_count: usize,
    pub unsupported_or_unknown_count: usize,
    pub warnings: Vec<StorageWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageHealthInput {
    pub path: PathBuf,
    pub platform: Option<String>,
    pub logical_size_bytes: Option<u64>,
    pub format_hint: Option<String>,
    pub content_hash: Option<String>,
}

fn platform_is(platform: Option<&str>, values: &[&str]) -> bool {
    let Some(platform) = platform else {
        return false;
    };
    let platform = platform.to_ascii_lowercase();
    values
        .iter()
        .any(|value| platform == *value || platform.contains(value))
}

fn format_for(input: &StorageHealthInput) -> StorageFormatClass {
    if let Some(hint) = input.format_hint.as_deref() {
        match hint.to_ascii_lowercase().as_str() {
            "zip" | "sevenzip" | "rar" => return StorageFormatClass::Archive,
            "direct_game_image" => {}
            _ => {}
        }
    }
    match input
        .path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "chd" => StorageFormatClass::Chd,
        "rvz" => StorageFormatClass::Rvz,
        "wia" => StorageFormatClass::Wia,
        "gcz" => StorageFormatClass::Gcz,
        "wbfs" => StorageFormatClass::Wbfs,
        "cso" => StorageFormatClass::Cso,
        "zso" => StorageFormatClass::Zso,
        "iso" => StorageFormatClass::Iso,
        "cue" | "bin" => StorageFormatClass::BinCue,
        "gdi" => StorageFormatClass::Gdi,
        "cdi" => StorageFormatClass::Cdi,
        "pbp" => StorageFormatClass::Pbp,
        "zip" | "7z" | "rar" => StorageFormatClass::Archive,
        "nes" | "smc" | "sfc" | "gb" | "gbc" | "gba" | "n64" | "z64" | "v64" => {
            StorageFormatClass::RawImage
        }
        _ => StorageFormatClass::Unknown,
    }
}

fn rough_estimate(logical: u64, explanation: &str) -> StorageEstimate {
    let _ = logical;
    StorageEstimate {
        kind: StorageEstimateKind::RoughOpportunity,
        minimum_savings_bytes: None,
        maximum_savings_bytes: None,
        explanation: explanation.to_string(),
    }
}

fn unknown_estimate(explanation: &str) -> StorageEstimate {
    StorageEstimate {
        kind: StorageEstimateKind::Unknown,
        minimum_savings_bytes: None,
        maximum_savings_bytes: None,
        explanation: explanation.to_string(),
    }
}

fn opportunity_for(
    format: StorageFormatClass,
    platform: Option<&str>,
    path: &Path,
    logical: u64,
) -> (StorageOpportunity, Vec<StorageWarning>) {
    let efficient = |target: Option<StorageFormatClass>| StorageOpportunity {
        kind: StorageOpportunityKind::AlreadyEfficient,
        target_format: target,
        round_trip: StorageRoundTripClass::NotEstablished,
        confidence: StorageConfidence::High,
        estimate: unknown_estimate(
            "Already compressed or containerised; no recompression estimate is shown.",
        ),
        verification_required: false,
        topology_sensitive: false,
        cleanup_eligibility: "NEVER_RECOMMEND".into(),
        warning: None,
    };
    match format {
        StorageFormatClass::Chd | StorageFormatClass::Rvz | StorageFormatClass::Wia
        | StorageFormatClass::Gcz | StorageFormatClass::Cso | StorageFormatClass::Zso
        | StorageFormatClass::Pbp | StorageFormatClass::Archive => return (efficient(None), Vec::new()),
        StorageFormatClass::Wbfs => return (
            StorageOpportunity {
                kind: StorageOpportunityKind::DoNotConvert,
                target_format: None,
                round_trip: StorageRoundTripClass::LossyOrUnsafe,
                confidence: StorageConfidence::Medium,
                estimate: unknown_estimate("WBFS is not treated as a preservation-grade conversion source."),
                verification_required: true,
                topology_sensitive: true,
                cleanup_eligibility: "NEVER_RECOMMEND".into(),
                warning: Some("Do not infer a safe generic conversion from the WBFS extension alone.".into()),
            },
            vec![StorageWarning { code: "WBFS_PRESERVATION_REVIEW".into(), message: "WBFS handling is topology-sensitive and preservation-hostile without verified Wii evidence.".into() }],
        ),
        StorageFormatClass::Cdi => return (
            StorageOpportunity {
                kind: StorageOpportunityKind::UnsupportedFormat,
                target_format: None,
                round_trip: StorageRoundTripClass::NotEstablished,
                confidence: StorageConfidence::Low,
                estimate: unknown_estimate("No safe generic CDI conversion is established."),
                verification_required: true,
                topology_sensitive: true,
                cleanup_eligibility: "NEVER_RECOMMEND".into(),
                warning: Some("CDI may encode platform-specific or non-standard disc details.".into()),
            },
            Vec::new(),
        ),
        StorageFormatClass::BinCue => {
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bin")) {
                return (
                    StorageOpportunity {
                        kind: StorageOpportunityKind::TopologySensitive,
                        target_format: None,
                        round_trip: StorageRoundTripClass::NotEstablished,
                        confidence: StorageConfidence::Unknown,
                        estimate: unknown_estimate("The BIN is a CUE-owned companion; assess the complete sheet."),
                        verification_required: true,
                        topology_sensitive: true,
                        cleanup_eligibility: "REQUIRES_CONVERSION".into(),
                        warning: Some("Do not assess or remove a BIN independently of its CUE topology.".into()),
                    },
                    Vec::new(),
                );
            }
            let layout = resolve_cue_layout(path);
            let track_count = layout.as_ref().map(|layout| layout.tracks.len()).unwrap_or(0);
            let warning = if track_count > 1 { Some("Multi-track topology must be preserved and independently verified.".into()) } else { None };
            let mut warnings = Vec::new();
            if layout.is_err() { warnings.push(StorageWarning { code: "CUE_REVIEW_REQUIRED".into(), message: "CUE topology could not be verified from the current source.".into() }); }
            (
                StorageOpportunity {
                    kind: if layout.is_ok() { StorageOpportunityKind::Compressible } else { StorageOpportunityKind::TopologySensitive },
                    target_format: layout.is_ok().then_some(StorageFormatClass::Chd),
                    round_trip: if layout.is_ok() { StorageRoundTripClass::ContentEquivalent } else { StorageRoundTripClass::NotEstablished },
                    confidence: if layout.is_ok() { StorageConfidence::High } else { StorageConfidence::Low },
                    estimate: if layout.is_ok() { rough_estimate(logical, "Qualitative opportunity only; exact CHD savings require a bounded dry-run or conversion measurement.") } else { unknown_estimate("CUE topology could not be verified.") },
                    verification_required: true,
                    topology_sensitive: true,
                    cleanup_eligibility: "REQUIRES_VERIFICATION".into(),
                    warning,
                },
                warnings,
            )
        }
        StorageFormatClass::Gdi => {
            let valid = resolve_gdi_all_tracks(path).is_ok();
            (
                StorageOpportunity {
                    kind: if valid { StorageOpportunityKind::Compressible } else { StorageOpportunityKind::TopologySensitive },
                    target_format: valid.then_some(StorageFormatClass::Chd),
                    round_trip: if valid { StorageRoundTripClass::ContentEquivalent } else { StorageRoundTripClass::NotEstablished },
                    confidence: if valid && platform_is(platform, &["dreamcast"]) { StorageConfidence::Medium } else { StorageConfidence::Low },
                    estimate: if valid { rough_estimate(logical, "Range only; GD-ROM CHD savings depend on track layout and compression." ) } else { unknown_estimate("GDI topology is incomplete or invalid.") },
                    verification_required: true,
                    topology_sensitive: true,
                    cleanup_eligibility: "REQUIRES_VERIFICATION".into(),
                    warning: Some("Dreamcast/GD-ROM topology must be verified before any future conversion.".into()),
                },
                Vec::new(),
            )
        }
        StorageFormatClass::Iso => {
            let (target, round_trip, explanation) = if platform_is(platform, &["gamecube", "wii"]) {
                (Some(StorageFormatClass::Rvz), StorageRoundTripClass::PlayableNotOriginalReconstructable, "RVZ is a storage candidate, but original reconstruction is not assumed byte-identical.")
            } else if platform_is(platform, &["psp"]) {
                (Some(StorageFormatClass::Cso), StorageRoundTripClass::PlayableNotOriginalReconstructable, "CSO is a PSP compatibility candidate; exact savings and compatibility require verification.")
            } else if platform_is(platform, &["playstation", "ps2", "saturn", "dreamcast", "sega cd", "pc engine cd", "neogeo cd", "3do"]) {
                (Some(StorageFormatClass::Chd), StorageRoundTripClass::ContentEquivalent, "Optical CHD candidacy requires verified media classification; generic ISO to CHD is not assumed safe.")
            } else {
                (None, StorageRoundTripClass::NotEstablished, "Media platform is not sufficient to propose a safe target format.")
            };
            let kind = target.map_or(StorageOpportunityKind::Unknown, |_| StorageOpportunityKind::Compressible);
            (
                StorageOpportunity {
                    kind,
                    target_format: target,
                    round_trip,
                    confidence: if target.is_some() { StorageConfidence::Medium } else { StorageConfidence::Unknown },
                    estimate: if target.is_some() { rough_estimate(logical, explanation) } else { unknown_estimate(explanation) },
                    verification_required: true,
                    topology_sensitive: target == Some(StorageFormatClass::Chd),
                    cleanup_eligibility: "REQUIRES_VERIFICATION".into(),
                    warning: (target == Some(StorageFormatClass::Chd)).then_some("Disc type and track topology must be proven before a CHD mode is selected.".into()),
                },
                Vec::new(),
            )
        }
        StorageFormatClass::RawImage | StorageFormatClass::Unknown => (
            StorageOpportunity {
                kind: StorageOpportunityKind::Unknown,
                target_format: None,
                round_trip: StorageRoundTripClass::NotEstablished,
                confidence: StorageConfidence::Unknown,
                estimate: unknown_estimate("No evidence-backed conversion candidate is available."),
                verification_required: true,
                topology_sensitive: false,
                cleanup_eligibility: "NEVER_RECOMMEND".into(),
                warning: None,
            },
            Vec::new(),
        ),
    }
}

fn metadata_for(
    path: &Path,
) -> (
    StorageObjectKind,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return (StorageObjectKind::Missing, None, None, None, None);
    };
    if metadata.file_type().is_symlink() {
        return (
            StorageObjectKind::Symlink,
            Some(metadata.len()),
            None,
            None,
            None,
        );
    }
    if !metadata.is_file() {
        return (
            StorageObjectKind::Unreadable,
            Some(metadata.len()),
            None,
            None,
            None,
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let allocated = metadata.blocks().checked_mul(512);
        let kind = if metadata.nlink() > 1 {
            StorageObjectKind::Hardlink
        } else {
            StorageObjectKind::OrdinaryFile
        };
        return (
            kind,
            Some(metadata.len()),
            allocated,
            Some(metadata.dev()),
            Some(metadata.ino()),
        );
    }
    #[cfg(not(unix))]
    (
        StorageObjectKind::OrdinaryFile,
        Some(metadata.len()),
        None,
        None,
        None,
    )
}

pub fn analyze_storage_health(inputs: &[StorageHealthInput]) -> StorageHealthReport {
    let mut sorted = inputs.to_vec();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut items = Vec::with_capacity(sorted.len());
    for input in &sorted {
        let format = format_for(&input);
        let (object_kind, observed_size, allocated, device, inode) = metadata_for(&input.path);
        let logical = input.logical_size_bytes.or(observed_size);
        let (mut opportunity, mut warnings) = opportunity_for(
            format,
            input.platform.as_deref(),
            &input.path,
            logical.unwrap_or(0),
        );
        if matches!(
            object_kind,
            StorageObjectKind::Missing | StorageObjectKind::Symlink | StorageObjectKind::Unreadable
        ) {
            warnings.push(StorageWarning {
                code: "FILESYSTEM_EVIDENCE_INCOMPLETE".into(),
                message: "Filesystem metadata is incomplete; no safe savings claim is made.".into(),
            });
            opportunity.kind = StorageOpportunityKind::Unknown;
            opportunity.target_format = None;
            opportunity.confidence = StorageConfidence::Unknown;
            opportunity.estimate = unknown_estimate("Filesystem evidence is incomplete.");
            opportunity.cleanup_eligibility = "NOT_ELIGIBLE".into();
        }
        items.push(StorageHealthItem {
            path: input.path.clone(),
            platform: input.platform.clone(),
            format,
            object_kind,
            logical_size_bytes: logical,
            allocated_size_bytes: allocated,
            filesystem_device: device,
            filesystem_inode: inode,
            opportunity,
            warnings,
        });
    }

    let mut hashes: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut inodes: BTreeMap<(u64, u64), Vec<usize>> = BTreeMap::new();
    for (index, input) in sorted.iter().enumerate() {
        if let Some(hash) = input.content_hash.as_ref().filter(|hash| !hash.is_empty()) {
            hashes.entry(hash.clone()).or_default().push(index);
        }
        if let (Some(device), Some(inode)) = (
            items[index].filesystem_device,
            items[index].filesystem_inode,
        ) {
            inodes.entry((device, inode)).or_default().push(index);
        }
    }
    let mut duplicate_indices = BTreeSet::new();
    for indices in hashes.values().chain(inodes.values()) {
        if indices.len() < 2 {
            continue;
        }
        for index in indices {
            if !matches!(
                items[*index].opportunity.kind,
                StorageOpportunityKind::AlreadyEfficient
            ) {
                items[*index].opportunity.kind = StorageOpportunityKind::PossibleDuplicateContent;
                items[*index].opportunity.warning = Some("Duplicate content or shared inode is evidenced; physical savings are not counted without a user decision.".into());
                items[*index].opportunity.estimate = unknown_estimate(
                    "Shared or duplicate storage must be proven at filesystem level before savings are measured.",
                );
                items[*index].opportunity.cleanup_eligibility = "VERIFIED_BUT_USER_DECISION".into();
            }
            duplicate_indices.insert(*index);
        }
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));
    let total_logical_size_bytes = items
        .iter()
        .filter_map(|item| item.logical_size_bytes)
        .sum();
    let total_allocated_size_bytes = if items.iter().all(|item| item.allocated_size_bytes.is_some())
    {
        let mut seen = BTreeSet::new();
        Some(
            items
                .iter()
                .filter_map(|item| {
                    let allocated = item.allocated_size_bytes?;
                    let identity = item.filesystem_device.zip(item.filesystem_inode);
                    if let Some(identity) = identity
                        && !seen.insert(identity)
                    {
                        return Some(0);
                    }
                    Some(allocated)
                })
                .sum(),
        )
    } else {
        None
    };
    let already_efficient_count = items
        .iter()
        .filter(|i| matches!(i.opportunity.kind, StorageOpportunityKind::AlreadyEfficient))
        .count();
    let candidate_count = items
        .iter()
        .filter(|i| matches!(i.opportunity.kind, StorageOpportunityKind::Compressible))
        .count();
    let topology_sensitive_count = items
        .iter()
        .filter(|i| i.opportunity.topology_sensitive)
        .count();
    let unsupported_or_unknown_count = items
        .iter()
        .filter(|i| {
            matches!(
                i.opportunity.kind,
                StorageOpportunityKind::UnsupportedFormat
                    | StorageOpportunityKind::Unknown
                    | StorageOpportunityKind::DoNotConvert
            )
        })
        .count();
    StorageHealthReport {
        items,
        total_logical_size_bytes,
        total_allocated_size_bytes,
        already_efficient_count,
        candidate_count,
        topology_sensitive_count,
        duplicate_candidate_count: duplicate_indices.len(),
        unsupported_or_unknown_count,
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn input(path: &Path, platform: &str) -> StorageHealthInput {
        StorageHealthInput {
            path: path.to_path_buf(),
            platform: Some(platform.into()),
            logical_size_bytes: None,
            format_hint: None,
            content_hash: None,
        }
    }

    #[test]
    fn raw_iso_is_classified_without_fake_exact_savings() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("game.iso");
        fs::write(&path, vec![0_u8; 1000]).unwrap();
        let report = analyze_storage_health(&[input(&path, "PlayStation 2")]);
        assert_eq!(report.items[0].format, StorageFormatClass::Iso);
        assert_eq!(
            report.items[0].opportunity.target_format,
            Some(StorageFormatClass::Chd)
        );
        assert_eq!(
            report.items[0].opportunity.estimate.kind,
            StorageEstimateKind::RoughOpportunity
        );
        assert_ne!(
            report.items[0].opportunity.estimate.kind,
            StorageEstimateKind::ExactMeasured
        );
    }

    #[test]
    fn cue_multitrack_is_topology_sensitive() {
        let dir = tempdir().unwrap();
        let cue = dir.path().join("disc.cue");
        fs::write(&dir.path().join("a.bin"), [0_u8; 4]).unwrap();
        fs::write(&dir.path().join("b.bin"), [0_u8; 4]).unwrap();
        fs::write(&cue, "FILE \"a.bin\" BINARY\n TRACK 01 MODE1/2352\n INDEX 01 00:00:00\n FILE \"b.bin\" BINARY\n TRACK 02 AUDIO\n INDEX 01 00:00:00\n").unwrap();
        let report = analyze_storage_health(&[input(&cue, "PlayStation")]);
        assert!(report.items[0].opportunity.topology_sensitive);
        assert_eq!(
            report.items[0].opportunity.target_format,
            Some(StorageFormatClass::Chd)
        );
        assert!(report.items[0].opportunity.warning.is_some());
    }

    #[test]
    fn already_compressed_formats_are_not_recompression_candidates() {
        let dir = tempdir().unwrap();
        let chd = dir.path().join("game.chd");
        let rvz = dir.path().join("game.rvz");
        fs::write(&chd, [0_u8; 4]).unwrap();
        fs::write(&rvz, [0_u8; 4]).unwrap();
        let report = analyze_storage_health(&[input(&chd, "PlayStation"), input(&rvz, "GameCube")]);
        assert!(
            report
                .items
                .iter()
                .all(|i| matches!(i.opportunity.kind, StorageOpportunityKind::AlreadyEfficient))
        );
    }

    #[test]
    fn hardlinks_are_counted_as_shared_storage_not_two_physical_copies() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("a.iso");
        let second = dir.path().join("b.iso");
        fs::write(&first, [1_u8; 32]).unwrap();
        fs::hard_link(&first, &second).unwrap();
        let report = analyze_storage_health(&[input(&first, "Unknown"), input(&second, "Unknown")]);
        assert!(
            report
                .items
                .iter()
                .all(|i| matches!(i.object_kind, StorageObjectKind::Hardlink))
        );
        assert_eq!(report.duplicate_candidate_count, 2);
        assert_eq!(
            report.total_allocated_size_bytes,
            Some(report.items[0].allocated_size_bytes.unwrap())
        );
    }

    #[test]
    fn duplicate_hashes_are_reported_deterministically() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.nes");
        let b = dir.path().join("b.nes");
        fs::write(&a, [1_u8; 4]).unwrap();
        fs::write(&b, [2_u8; 4]).unwrap();
        let mut left = input(&a, "NES");
        let mut right = input(&b, "NES");
        left.content_hash = Some("same".into());
        right.content_hash = Some("same".into());
        let report = analyze_storage_health(&[right, left]);
        assert_eq!(report.items[0].path, a);
        assert_eq!(report.duplicate_candidate_count, 2);
        assert!(report.items.iter().all(|i| matches!(
            i.opportunity.kind,
            StorageOpportunityKind::PossibleDuplicateContent
        )));
    }
}
