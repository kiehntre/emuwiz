//! Read-only compatibility planning for moving saves between representations.
//!
//! This module deliberately stops at an evidence-backed plan.  It does not
//! inspect or rewrite files, invoke conversion tools, download anything, or
//! publish into emulator directories.  Callers should build the observations
//! from the existing save and persistent-state inventories.

use crate::save_snapshots::SaveArtifactType;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveMigrationFormat {
    RawSram,
    GbaRawSave,
    SnesSram,
    GenesisSram,
    Ps1MemoryCard,
    SaturnBackupMemory,
    PspSavedata,
    MisterRawSave,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveMigrationRepresentation {
    RawFile { size_bytes: u64 },
    Directory { file_count: Option<u32> },
    MemoryCardImage { size_bytes: u64, container: String },
    OpaqueContainer { size_bytes: u64, kind: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveMigrationIdentityConfidence {
    Exact,
    Strong,
    FilenameOnly,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMigrationSource {
    pub platform: Option<String>,
    pub game_identity: Option<String>,
    pub region: Option<String>,
    pub emulator: Option<String>,
    pub core: Option<String>,
    pub save_type: SaveArtifactType,
    pub format: SaveMigrationFormat,
    pub representation: SaveMigrationRepresentation,
    pub identity_confidence: SaveMigrationIdentityConfidence,
    pub filename: Option<String>,
    pub container_context: Option<String>,
    pub evidence_stale: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMigrationConversionPath {
    pub from: SaveMigrationFormat,
    pub to: SaveMigrationFormat,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMigrationTarget {
    pub platform: Option<String>,
    pub game_identity: Option<String>,
    pub region: Option<String>,
    pub emulator: Option<String>,
    pub core: Option<String>,
    pub save_type: SaveArtifactType,
    pub format: SaveMigrationFormat,
    pub representation: SaveMigrationRepresentation,
    pub container_context: Option<String>,
    /// Paths asserted by a trusted provider or format adapter.  An empty list
    /// means that no conversion engine is currently proven.
    pub proven_conversion_paths: Vec<SaveMigrationConversionPath>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveMigrationEvidence {
    ExactGameIdentity,
    ExactPlatform,
    ExactRegion,
    ExactFormat,
    ExactRepresentation,
    ExactSize,
    KnownConversionPath(String),
    FilenameOnly,
    MissingGameIdentity,
    WrapperMismatch,
    SizeMismatch,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveMigrationCompatibility {
    DirectlyCompatible,
    ConversionRequired,
    ConversionAvailable,
    ConversionUnknown,
    Unsupported,
    Ambiguous,
    StaleEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMigrationPlan {
    pub compatibility: SaveMigrationCompatibility,
    pub source: SaveMigrationSource,
    pub target: SaveMigrationTarget,
    pub evidence: Vec<SaveMigrationEvidence>,
    pub explanation: String,
    pub read_only: bool,
}

fn same_opt(left: &Option<String>, right: &Option<String>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn same_format(left: &SaveMigrationFormat, right: &SaveMigrationFormat) -> bool {
    left == right
}

fn same_representation(
    left: &SaveMigrationRepresentation,
    right: &SaveMigrationRepresentation,
) -> (bool, bool) {
    match (left, right) {
        (
            SaveMigrationRepresentation::RawFile { size_bytes: left },
            SaveMigrationRepresentation::RawFile { size_bytes: right },
        ) => (left == right, left != right),
        (
            SaveMigrationRepresentation::Directory { file_count: left },
            SaveMigrationRepresentation::Directory { file_count: right },
        ) => (left == right || left.is_none() || right.is_none(), false),
        (
            SaveMigrationRepresentation::MemoryCardImage {
                size_bytes: left_size,
                container: left_container,
            },
            SaveMigrationRepresentation::MemoryCardImage {
                size_bytes: right_size,
                container: right_container,
            },
        ) => (
            left_size == right_size && left_container == right_container,
            left_size != right_size,
        ),
        (
            SaveMigrationRepresentation::OpaqueContainer {
                size_bytes: left_size,
                kind: left_kind,
            },
            SaveMigrationRepresentation::OpaqueContainer {
                size_bytes: right_size,
                kind: right_kind,
            },
        ) => (
            left_size == right_size && left_kind == right_kind,
            left_size != right_size,
        ),
        _ => (false, false),
    }
}

fn proven_conversion(source: &SaveMigrationSource, target: &SaveMigrationTarget) -> Option<String> {
    target
        .proven_conversion_paths
        .iter()
        .find(|path| path.from == source.format && path.to == target.format)
        .map(|path| path.evidence.clone())
}

/// Build a read-only save migration compatibility plan.
pub fn plan_save_migration(
    source: SaveMigrationSource,
    target: SaveMigrationTarget,
) -> SaveMigrationPlan {
    let mut evidence = Vec::new();
    if source.evidence_stale {
        evidence.push(SaveMigrationEvidence::Stale);
        return plan(
            SaveMigrationCompatibility::StaleEvidence,
            source,
            target,
            evidence,
            "source evidence is stale and must be refreshed before compatibility is assessed",
        );
    }
    if !same_opt(&source.game_identity, &target.game_identity) {
        return plan(
            SaveMigrationCompatibility::Unsupported,
            source,
            target,
            evidence,
            "source and target identify different games",
        );
    }
    if let (Some(source_platform), Some(target_platform)) = (&source.platform, &target.platform) {
        if source_platform != target_platform {
            return plan(
                SaveMigrationCompatibility::Unsupported,
                source,
                target,
                evidence,
                "source and target platforms differ",
            );
        }
        evidence.push(SaveMigrationEvidence::ExactPlatform);
    }
    if source.game_identity.is_some() && source.game_identity == target.game_identity {
        evidence.push(SaveMigrationEvidence::ExactGameIdentity);
    } else {
        evidence.push(SaveMigrationEvidence::MissingGameIdentity);
    }
    if source.region.is_some() && source.region == target.region {
        evidence.push(SaveMigrationEvidence::ExactRegion);
    }
    if source.identity_confidence == SaveMigrationIdentityConfidence::FilenameOnly {
        evidence.push(SaveMigrationEvidence::FilenameOnly);
        return plan(
            SaveMigrationCompatibility::Ambiguous,
            source,
            target,
            evidence,
            "filename-only evidence cannot prove save compatibility",
        );
    }
    if source.game_identity.is_none() || target.game_identity.is_none() {
        return plan(
            SaveMigrationCompatibility::Ambiguous,
            source,
            target,
            evidence,
            "game identity is required before direct compatibility can be claimed",
        );
    }

    if matches!(source.format, SaveMigrationFormat::PspSavedata)
        && !matches!(
            source.representation,
            SaveMigrationRepresentation::Directory { .. }
        )
    {
        return plan(
            SaveMigrationCompatibility::Unsupported,
            source,
            target,
            evidence,
            "PSP Savedata is a directory context, not a raw single-file save",
        );
    }
    if matches!(target.format, SaveMigrationFormat::PspSavedata)
        && !matches!(
            target.representation,
            SaveMigrationRepresentation::Directory { .. }
        )
    {
        return plan(
            SaveMigrationCompatibility::Unsupported,
            source,
            target,
            evidence,
            "PSP Savedata targets must remain directory representations",
        );
    }

    let (representation_match, size_mismatch) =
        same_representation(&source.representation, &target.representation);
    if same_format(&source.format, &target.format) {
        evidence.push(SaveMigrationEvidence::ExactFormat);
        if representation_match {
            evidence.push(SaveMigrationEvidence::ExactRepresentation);
            if source.container_context == target.container_context {
                if matches!(
                    source.representation,
                    SaveMigrationRepresentation::RawFile { .. }
                        | SaveMigrationRepresentation::MemoryCardImage { .. }
                ) {
                    evidence.push(SaveMigrationEvidence::ExactSize);
                }
                return plan(
                    SaveMigrationCompatibility::DirectlyCompatible,
                    source,
                    target,
                    evidence,
                    "format, game identity, and representation are proven equivalent",
                );
            }
        }
        if size_mismatch {
            evidence.push(SaveMigrationEvidence::SizeMismatch);
        }
        evidence.push(SaveMigrationEvidence::WrapperMismatch);
        if let Some(path) = proven_conversion(&source, &target) {
            evidence.push(SaveMigrationEvidence::KnownConversionPath(path));
            return plan(
                SaveMigrationCompatibility::ConversionAvailable,
                source,
                target,
                evidence,
                "a trusted conversion path is recorded for the incompatible representation",
            );
        }
        return plan(
            SaveMigrationCompatibility::ConversionRequired,
            source,
            target,
            evidence,
            "the save format is known but its wrapper, layout, or size differs",
        );
    }

    if let Some(path) = proven_conversion(&source, &target) {
        evidence.push(SaveMigrationEvidence::KnownConversionPath(path));
        return plan(
            SaveMigrationCompatibility::ConversionAvailable,
            source,
            target,
            evidence,
            "a trusted conversion path is explicitly recorded for these formats",
        );
    }
    if source.platform == target.platform && representation_match {
        return plan(
            SaveMigrationCompatibility::ConversionUnknown,
            source,
            target,
            evidence,
            "the platform and representation are similar, but no conversion rule is proven",
        );
    }
    plan(
        SaveMigrationCompatibility::ConversionUnknown,
        source,
        target,
        evidence,
        "no exact format or trusted conversion rule is known",
    )
}

fn plan(
    compatibility: SaveMigrationCompatibility,
    source: SaveMigrationSource,
    target: SaveMigrationTarget,
    evidence: Vec<SaveMigrationEvidence>,
    explanation: &str,
) -> SaveMigrationPlan {
    SaveMigrationPlan {
        compatibility,
        source,
        target,
        evidence,
        explanation: explanation.into(),
        read_only: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(
        format: SaveMigrationFormat,
        representation: SaveMigrationRepresentation,
    ) -> SaveMigrationSource {
        SaveMigrationSource {
            platform: Some("SNES".into()),
            game_identity: Some("game-1".into()),
            region: Some("US".into()),
            emulator: Some("RetroArch".into()),
            core: Some("snes9x".into()),
            save_type: SaveArtifactType::Sram,
            format,
            representation,
            identity_confidence: SaveMigrationIdentityConfidence::Exact,
            filename: Some("game.srm".into()),
            container_context: None,
            evidence_stale: false,
        }
    }

    fn target(
        format: SaveMigrationFormat,
        representation: SaveMigrationRepresentation,
    ) -> SaveMigrationTarget {
        SaveMigrationTarget {
            platform: Some("SNES".into()),
            game_identity: Some("game-1".into()),
            region: Some("US".into()),
            emulator: Some("RetroArch".into()),
            core: Some("snes9x".into()),
            save_type: SaveArtifactType::Sram,
            format,
            representation,
            container_context: None,
            proven_conversion_paths: Vec::new(),
        }
    }

    fn raw(size: u64) -> SaveMigrationRepresentation {
        SaveMigrationRepresentation::RawFile { size_bytes: size }
    }

    #[test]
    fn exact_raw_format_is_directly_compatible() {
        let plan = plan_save_migration(
            source(SaveMigrationFormat::SnesSram, raw(32 * 1024)),
            target(SaveMigrationFormat::SnesSram, raw(32 * 1024)),
        );
        assert_eq!(
            plan.compatibility,
            SaveMigrationCompatibility::DirectlyCompatible
        );
        assert!(plan.read_only);
    }

    #[test]
    fn incompatible_wrapper_requires_conversion() {
        let mut destination = target(
            SaveMigrationFormat::Ps1MemoryCard,
            SaveMigrationRepresentation::MemoryCardImage {
                size_bytes: 128 * 1024,
                container: "duckstation-card".into(),
            },
        );
        destination.platform = Some("PS1".into());
        destination.container_context = Some("duckstation".into());
        let mut origin = source(
            SaveMigrationFormat::Ps1MemoryCard,
            SaveMigrationRepresentation::MemoryCardImage {
                size_bytes: 128 * 1024,
                container: "raw-card".into(),
            },
        );
        origin.platform = Some("PS1".into());
        origin.container_context = Some("raw".into());
        let plan = plan_save_migration(origin, destination);
        assert_eq!(
            plan.compatibility,
            SaveMigrationCompatibility::ConversionRequired
        );
    }

    #[test]
    fn explicit_proven_conversion_is_available() {
        let mut destination = target(SaveMigrationFormat::GbaRawSave, raw(32 * 1024));
        destination.platform = Some("GBA".into());
        destination
            .proven_conversion_paths
            .push(SaveMigrationConversionPath {
                from: SaveMigrationFormat::SnesSram,
                to: SaveMigrationFormat::GbaRawSave,
                evidence: "provider-verified game-specific adapter".into(),
            });
        let mut origin = source(SaveMigrationFormat::SnesSram, raw(32 * 1024));
        origin.platform = Some("GBA".into());
        let plan = plan_save_migration(origin, destination);
        assert_eq!(
            plan.compatibility,
            SaveMigrationCompatibility::ConversionAvailable
        );
    }

    #[test]
    fn unknown_emulator_format_is_conversion_unknown() {
        let mut destination = target(
            SaveMigrationFormat::Unknown("vendor-x".into()),
            raw(32 * 1024),
        );
        destination.emulator = Some("UnknownEmulator".into());
        let plan = plan_save_migration(
            source(SaveMigrationFormat::SnesSram, raw(32 * 1024)),
            destination,
        );
        assert_eq!(
            plan.compatibility,
            SaveMigrationCompatibility::ConversionUnknown
        );
    }

    #[test]
    fn filename_only_evidence_is_ambiguous() {
        let mut origin = source(SaveMigrationFormat::RawSram, raw(32 * 1024));
        origin.game_identity = None;
        origin.identity_confidence = SaveMigrationIdentityConfidence::FilenameOnly;
        let plan =
            plan_save_migration(origin, target(SaveMigrationFormat::RawSram, raw(32 * 1024)));
        assert_eq!(plan.compatibility, SaveMigrationCompatibility::Ambiguous);
    }

    #[test]
    fn wrong_game_identity_is_unsupported() {
        let mut destination = target(SaveMigrationFormat::SnesSram, raw(32 * 1024));
        destination.game_identity = Some("other-game".into());
        let plan = plan_save_migration(
            source(SaveMigrationFormat::SnesSram, raw(32 * 1024)),
            destination,
        );
        assert_eq!(plan.compatibility, SaveMigrationCompatibility::Unsupported);
    }

    #[test]
    fn stale_evidence_is_reported_before_compatibility() {
        let mut origin = source(SaveMigrationFormat::SnesSram, raw(32 * 1024));
        origin.evidence_stale = true;
        let plan = plan_save_migration(
            origin,
            target(SaveMigrationFormat::SnesSram, raw(32 * 1024)),
        );
        assert_eq!(
            plan.compatibility,
            SaveMigrationCompatibility::StaleEvidence
        );
    }

    #[test]
    fn ps1_card_container_mismatch_is_not_direct() {
        let mut origin = source(
            SaveMigrationFormat::Ps1MemoryCard,
            SaveMigrationRepresentation::MemoryCardImage {
                size_bytes: 128 * 1024,
                container: "raw-card".into(),
            },
        );
        origin.platform = Some("PS1".into());
        let mut destination = target(
            SaveMigrationFormat::Ps1MemoryCard,
            SaveMigrationRepresentation::MemoryCardImage {
                size_bytes: 128 * 1024,
                container: "duckstation-card".into(),
            },
        );
        destination.platform = Some("PS1".into());
        assert_ne!(
            plan_save_migration(origin, destination).compatibility,
            SaveMigrationCompatibility::DirectlyCompatible
        );
    }

    #[test]
    fn saturn_backup_memory_distinguishes_containers() {
        let mut origin = source(
            SaveMigrationFormat::SaturnBackupMemory,
            SaveMigrationRepresentation::MemoryCardImage {
                size_bytes: 32 * 1024,
                container: "raw-backup-memory".into(),
            },
        );
        origin.platform = Some("Saturn".into());
        let mut destination = target(
            SaveMigrationFormat::SaturnBackupMemory,
            SaveMigrationRepresentation::MemoryCardImage {
                size_bytes: 32 * 1024,
                container: "emulator-backup-memory".into(),
            },
        );
        destination.platform = Some("Saturn".into());
        assert_eq!(
            plan_save_migration(origin, destination).compatibility,
            SaveMigrationCompatibility::ConversionRequired
        );
    }

    #[test]
    fn psp_savedata_requires_directory_context() {
        let mut origin = source(SaveMigrationFormat::PspSavedata, raw(64 * 1024));
        origin.platform = Some("PSP".into());
        let mut destination = target(
            SaveMigrationFormat::PspSavedata,
            SaveMigrationRepresentation::Directory { file_count: None },
        );
        destination.platform = Some("PSP".into());
        assert_eq!(
            plan_save_migration(origin, destination).compatibility,
            SaveMigrationCompatibility::Unsupported
        );
    }

    #[test]
    fn mister_target_requires_proven_representation() {
        let mut origin = source(SaveMigrationFormat::SnesSram, raw(32 * 1024));
        origin.platform = Some("SNES".into());
        let mut destination = target(SaveMigrationFormat::MisterRawSave, raw(32 * 1024));
        destination.emulator = Some("MiSTer".into());
        assert_eq!(
            plan_save_migration(origin, destination).compatibility,
            SaveMigrationCompatibility::ConversionUnknown
        );
    }
}
