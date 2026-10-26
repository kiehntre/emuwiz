//! Read-only composition of save migration compatibility with restore
//! planning.
//!
//! [`plan_save_migration_for_target`] answers "can this save safely move from
//! here to there?" by composing the existing
//! [`crate::save_migration_planner::plan_save_migration`] result with the
//! existing restore capabilities in [`crate::save_snapshots`]. It adds
//! restrictions only; it does not duplicate compatibility rules, convert,
//! copy, restore, or write anything.
//!
//! Three truths stay separate and are never derived from one another:
//! migration compatibility, the existence of an Apply writer, and restore
//! safety (backup, quiescence, staging, rollback). A compatible save does not
//! imply a writer, and compatibility never clears a restore refusal.

use crate::launch::process_spawn::{CapturedFileIdentity, open_regular_file_nonblocking};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::save_migration_planner::{
    SaveMigrationCompatibility, SaveMigrationEvidence, SaveMigrationFormat,
    SaveMigrationIdentityConfidence, SaveMigrationPlan, SaveMigrationRepresentation,
    SaveMigrationSource, SaveMigrationTarget, plan_save_migration,
};
use crate::save_snapshots::{
    GenericSingleFileSaveFamily, SaveArtifactType, SaveRestorePreflight, SaveRestoreRefusal,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveMigrationOutcome {
    DirectlyCompatible,
    ConversionRequired,
    KnownIncompatible,
    NeedsEvidence,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveMigrationReason {
    // Known incompatible.
    WrongGame,
    PlatformMismatch,
    // Needs evidence.
    GameIdentityUnknown,
    FilenameOnlyIdentity,
    PlatformEvidenceMissing,
    PspFileSetIncomplete,
    SourceEvidenceStale,
    SourceChangedSinceCapture,
    TargetVersionEvidenceMissing,
    TargetEvidenceMissingOrChanged,
    NoProvenConversionRule,
    // Conversion required.
    RepresentationDiffers,
    ContainerAwareHandlingRequired,
    // Unsupported.
    SaveStateIsNotPersistentSave,
    UnsupportedFormat,
    UnsupportedRepresentation,
}

/// What the object being moved actually is. A card or directory is never
/// flattened to a raw single-save copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveContainerSemantics {
    RawSingleFile,
    /// Memory card / backup memory: container-aware handling required.
    SharedContainer,
    /// Complete directory set (PSP Savedata): every required file moves together.
    DirectorySet {
        required_file_count: Option<u32>,
    },
    Other,
}

/// Which *existing* restore capability could consume this target
/// representation. Says nothing about migration compatibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveRestoreWriterCapability {
    SingleFileTransactional(GenericSingleFileSaveFamily),
    /// The separate directory restore workflow exists, but requires its own
    /// identity/space/quiescence preview and supplies no migration authority.
    DirectoryRestoreWorkflow,
    /// No generic writer for this representation is bound by this adapter.
    PreviewOnly,
    None,
}

impl SaveRestoreWriterCapability {
    pub fn is_transactional(self) -> bool {
        matches!(self, Self::SingleFileTransactional(_))
    }
}

/// Conversion requirement retained from the planner. No converter is
/// invoked; `converter_recorded` only reflects a trusted path already named
/// by the caller/provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveConversionRequirement {
    pub from: SaveMigrationFormat,
    pub to: SaveMigrationFormat,
    pub reason: SaveMigrationReason,
    pub converter_recorded: Option<String>,
    pub evidence: Vec<SaveMigrationEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveTargetEnvironment {
    pub emulator: Option<String>,
    pub emulator_version: Option<String>,
    pub core: Option<String>,
    pub profile: Option<String>,
    /// True only where compatibility genuinely depends on the emulator
    /// version. Otherwise an absent version is not a constraint.
    pub version_matters: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveSourceFreshness {
    Current,
    Stale,
    /// No stamp was supplied, so currency could not be checked.
    Unchecked,
}

/// Opened-file identity and bounded SHA-256 observation of a source save.
/// Directory sets stay unproved; this adapter grants no tree traversal or writer authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveSourceStamp {
    path: PathBuf,
    entries: Option<(CapturedFileIdentity, String)>,
}

impl SaveSourceStamp {
    pub fn capture(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            entries: read_entries(path),
        }
    }

    /// Bind a content observation to the path, size and digest supplied by a
    /// trusted inventory. A filename alone can never establish this binding.
    pub fn matches_inventory(&self, path: &Path, size: u64, digest: Option<&str>) -> bool {
        self.path == path
            && self.entries.as_ref().is_some_and(|(identity, hash)| {
                identity.size == size && digest == Some(hash.as_str())
            })
    }

    /// Read-only content and file identity recheck.
    pub fn freshness(&self) -> SaveSourceFreshness {
        match (&self.entries, read_entries(&self.path)) {
            (Some(before), Some(now)) if *before == now => SaveSourceFreshness::Current,
            _ => SaveSourceFreshness::Stale,
        }
    }
}

/// Bounded regular-file observation; directories remain unproved rather than
/// walking arbitrary save sets or treating a partial set as complete.
fn read_entries(path: &Path) -> Option<(CapturedFileIdentity, String)> {
    const MAX_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;
    let mut file = open_regular_file_nonblocking(path).ok()?;
    let before = CapturedFileIdentity::capture(&file.metadata().ok()?);
    if before.size > MAX_PREVIEW_BYTES {
        return None;
    }
    let mut digest = Sha256::new();
    let mut total = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_PREVIEW_BYTES {
            return None;
        }
        digest.update(&buffer[..count]);
    }
    let after = CapturedFileIdentity::capture(&file.metadata().ok()?);
    let path_meta = std::fs::symlink_metadata(path).ok()?;
    if !path_meta.is_file()
        || before != after
        || total != before.size
        || CapturedFileIdentity::capture(&path_meta) != before
    {
        return None;
    }
    Some((
        before,
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}

pub struct SaveMigrationRequest {
    pub source: SaveMigrationSource,
    pub target: SaveMigrationTarget,
    pub target_environment: SaveTargetEnvironment,
    /// Where the source save lives, for provenance.
    pub source_path: Option<PathBuf>,
    pub source_stamp: Option<SaveSourceStamp>,
    /// Refusals from the existing restore preflight for this target, when one
    /// has been built. `None` means no restore preflight was supplied.
    pub restore_refusals: Option<Vec<SaveRestoreRefusal>>,
}

pub fn refusals_from_single_file_preflight(
    preflight: &SaveRestorePreflight,
) -> Vec<SaveRestoreRefusal> {
    preflight.refusals.clone()
}

/// Restore capability, kept apart from migration compatibility.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveRestoreComposition {
    pub writer: SaveRestoreWriterCapability,
    /// Existing restore refusals, carried through untouched.
    pub restore_refusals: Option<Vec<SaveRestoreRefusal>>,
    /// Restore planning may go on to its normal preview/readiness path.
    pub restore_planning_may_proceed: bool,
    /// A transactional writer exists AND its own preflight was supplied and
    /// is clean. Never true because migration is compatible.
    pub existing_restore_apply_ready: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMigrationReadiness {
    pub outcome: SaveMigrationOutcome,
    pub reasons: Vec<SaveMigrationReason>,
    pub container: SaveContainerSemantics,
    pub conversion: Option<SaveConversionRequirement>,
    /// The existing planner's own result, retained verbatim when it ran.
    pub planner_plan: Option<SaveMigrationPlan>,
    pub restore: SaveRestoreComposition,
    /// Always false: this job adds no migration Apply path.
    pub automatic_migration_available: bool,
    pub target_environment: SaveTargetEnvironment,
    pub source_path: Option<PathBuf>,
    pub source_freshness: SaveSourceFreshness,
    pub read_only: bool,
}

fn container_semantics(source: &SaveMigrationSource) -> SaveContainerSemantics {
    match (&source.format, &source.representation) {
        (SaveMigrationFormat::Ps1MemoryCard | SaveMigrationFormat::SaturnBackupMemory, _)
        | (
            _,
            SaveMigrationRepresentation::MemoryCardImage { .. }
            | SaveMigrationRepresentation::OpaqueContainer { .. },
        ) => SaveContainerSemantics::SharedContainer,
        (_, SaveMigrationRepresentation::Directory { file_count }) => {
            SaveContainerSemantics::DirectorySet {
                required_file_count: *file_count,
            }
        }
        (_, SaveMigrationRepresentation::RawFile { .. }) => SaveContainerSemantics::RawSingleFile,
    }
}

fn writer_for(target: &SaveMigrationTarget) -> SaveRestoreWriterCapability {
    if matches!(
        target.representation,
        SaveMigrationRepresentation::Directory { .. }
    ) && matches!(
        target.save_type,
        SaveArtifactType::PspSavedata | SaveArtifactType::SaveDirectory
    ) {
        return SaveRestoreWriterCapability::DirectoryRestoreWorkflow;
    }
    let raw = matches!(
        target.representation,
        SaveMigrationRepresentation::RawFile { .. }
    );
    let family = match target.save_type {
        SaveArtifactType::Sram => Some(GenericSingleFileSaveFamily::Sram),
        SaveArtifactType::Eeprom => Some(GenericSingleFileSaveFamily::Eeprom),
        SaveArtifactType::FlashSave => Some(GenericSingleFileSaveFamily::FlashSave),
        SaveArtifactType::Nvram => Some(GenericSingleFileSaveFamily::Nvram),
        _ => None,
    };
    if let (true, Some(family)) = (raw, family) {
        return SaveRestoreWriterCapability::SingleFileTransactional(family);
    }
    match target.save_type {
        SaveArtifactType::MemoryCard
        | SaveArtifactType::PsMemoryCard
        | SaveArtifactType::DolphinMemoryCard
        | SaveArtifactType::Vmu
        | SaveArtifactType::SaveDirectory
        | SaveArtifactType::PspSavedata => SaveRestoreWriterCapability::PreviewOnly,
        _ => SaveRestoreWriterCapability::None,
    }
}

fn is_raw_format(format: &SaveMigrationFormat) -> bool {
    matches!(
        format,
        SaveMigrationFormat::RawSram
            | SaveMigrationFormat::GbaRawSave
            | SaveMigrationFormat::SnesSram
            | SaveMigrationFormat::GenesisSram
            | SaveMigrationFormat::MisterRawSave
    )
}

/// Canonical, read-only preflight: source save + target environment =
/// migration assessment. Rechecks a bounded file digest, never traverses directories.
pub fn plan_save_migration_for_target(request: SaveMigrationRequest) -> SaveMigrationReadiness {
    let SaveMigrationRequest {
        mut source,
        target,
        target_environment,
        source_path,
        source_stamp,
        restore_refusals,
    } = request;

    let source_freshness = source_stamp
        .as_ref()
        .filter(|stamp| source_path.as_ref() == Some(&stamp.path))
        .map(SaveSourceStamp::freshness)
        .unwrap_or(SaveSourceFreshness::Unchecked);
    let container = container_semantics(&source);
    let writer = writer_for(&target);

    let build = |outcome: SaveMigrationOutcome,
                 reasons: Vec<SaveMigrationReason>,
                 conversion: Option<SaveConversionRequirement>,
                 planner_plan: Option<SaveMigrationPlan>,
                 source_freshness: SaveSourceFreshness| {
        let direct = outcome == SaveMigrationOutcome::DirectlyCompatible;
        let restore_planning_may_proceed = direct && writer != SaveRestoreWriterCapability::None;
        // This composition supplies no target-bound writer authorization.
        // Even a supplied empty refusal list cannot authorize a migration.
        let existing_restore_apply_ready = false;
        SaveMigrationReadiness {
            outcome,
            reasons,
            container,
            conversion,
            planner_plan,
            restore: SaveRestoreComposition {
                writer,
                restore_refusals: restore_refusals.clone(),
                restore_planning_may_proceed,
                existing_restore_apply_ready,
            },
            automatic_migration_available: false,
            target_environment: target_environment.clone(),
            source_path: source_path.clone(),
            source_freshness,
            read_only: true,
        }
    };

    // Save states have their own compatibility model.
    if source.save_type.is_save_state() || target.save_type.is_save_state() {
        return build(
            SaveMigrationOutcome::Unsupported,
            vec![SaveMigrationReason::SaveStateIsNotPersistentSave],
            None,
            None,
            source_freshness,
        );
    }
    if matches!(source.format, SaveMigrationFormat::Unknown(_))
        || matches!(target.format, SaveMigrationFormat::Unknown(_))
    {
        return build(
            SaveMigrationOutcome::Unsupported,
            vec![SaveMigrationReason::UnsupportedFormat],
            None,
            None,
            source_freshness,
        );
    }
    // Source changed after evidence capture: the old conclusion is not current.
    if source_freshness == SaveSourceFreshness::Stale {
        source.evidence_stale = true;
    }

    let plan = plan_save_migration(source.clone(), target.clone());
    let mut reasons = Vec::new();
    let mut conversion = None;
    let mut outcome = match plan.compatibility {
        SaveMigrationCompatibility::DirectlyCompatible => SaveMigrationOutcome::DirectlyCompatible,
        SaveMigrationCompatibility::ConversionRequired => {
            reasons.push(SaveMigrationReason::RepresentationDiffers);
            SaveMigrationOutcome::ConversionRequired
        }
        SaveMigrationCompatibility::ConversionAvailable => {
            reasons.push(SaveMigrationReason::RepresentationDiffers);
            SaveMigrationOutcome::ConversionRequired
        }
        SaveMigrationCompatibility::ConversionUnknown => {
            reasons.push(SaveMigrationReason::NoProvenConversionRule);
            SaveMigrationOutcome::NeedsEvidence
        }
        SaveMigrationCompatibility::Ambiguous => {
            reasons.push(
                if source.identity_confidence == SaveMigrationIdentityConfidence::FilenameOnly {
                    SaveMigrationReason::FilenameOnlyIdentity
                } else {
                    SaveMigrationReason::GameIdentityUnknown
                },
            );
            SaveMigrationOutcome::NeedsEvidence
        }
        SaveMigrationCompatibility::StaleEvidence => {
            reasons.push(if source_freshness == SaveSourceFreshness::Stale {
                SaveMigrationReason::SourceChangedSinceCapture
            } else {
                SaveMigrationReason::SourceEvidenceStale
            });
            SaveMigrationOutcome::NeedsEvidence
        }
        SaveMigrationCompatibility::Unsupported => {
            let games_differ = matches!(
                (&source.game_identity, &target.game_identity),
                (Some(a), Some(b)) if a != b
            );
            let platforms_differ = matches!(
                (&source.platform, &target.platform),
                (Some(a), Some(b)) if a != b
            );
            if games_differ {
                reasons.push(SaveMigrationReason::WrongGame);
                SaveMigrationOutcome::KnownIncompatible
            } else if platforms_differ {
                reasons.push(SaveMigrationReason::PlatformMismatch);
                SaveMigrationOutcome::KnownIncompatible
            } else {
                reasons.push(SaveMigrationReason::UnsupportedRepresentation);
                SaveMigrationOutcome::Unsupported
            }
        }
    };

    // Restrictions layered on top of the planner; each can only make the
    // outcome more conservative.
    if outcome == SaveMigrationOutcome::DirectlyCompatible {
        if source_freshness != SaveSourceFreshness::Current {
            reasons.push(SaveMigrationReason::SourceEvidenceStale);
            outcome = SaveMigrationOutcome::NeedsEvidence;
        } else if (is_raw_format(&source.format) || is_raw_format(&target.format))
            && (source.platform.is_none() || target.platform.is_none())
        {
            reasons.push(SaveMigrationReason::PlatformEvidenceMissing);
            outcome = SaveMigrationOutcome::NeedsEvidence;
        } else if source.format == SaveMigrationFormat::PspSavedata
            && !matches!(
                source.representation,
                SaveMigrationRepresentation::Directory {
                    file_count: Some(_)
                }
            )
        {
            reasons.push(SaveMigrationReason::PspFileSetIncomplete);
            outcome = SaveMigrationOutcome::NeedsEvidence;
        } else if container == SaveContainerSemantics::SharedContainer {
            reasons.push(SaveMigrationReason::ContainerAwareHandlingRequired);
            outcome = SaveMigrationOutcome::ConversionRequired;
        } else if target_environment.version_matters
            && target_environment.emulator_version.is_none()
        {
            reasons.push(SaveMigrationReason::TargetVersionEvidenceMissing);
            outcome = SaveMigrationOutcome::NeedsEvidence;
        }
    }
    if source.format == SaveMigrationFormat::PspSavedata
        && !matches!(
            source.representation,
            SaveMigrationRepresentation::Directory {
                file_count: Some(_)
            }
        )
        && !reasons.contains(&SaveMigrationReason::PspFileSetIncomplete)
    {
        reasons.push(SaveMigrationReason::PspFileSetIncomplete);
    }
    if outcome == SaveMigrationOutcome::ConversionRequired
        && container == SaveContainerSemantics::SharedContainer
        && !reasons.contains(&SaveMigrationReason::ContainerAwareHandlingRequired)
    {
        reasons.push(SaveMigrationReason::ContainerAwareHandlingRequired);
    }
    if outcome == SaveMigrationOutcome::ConversionRequired {
        let converter_recorded = plan.evidence.iter().find_map(|item| match item {
            SaveMigrationEvidence::KnownConversionPath(path) => Some(path.clone()),
            _ => None,
        });
        conversion = Some(SaveConversionRequirement {
            from: source.format.clone(),
            to: target.format.clone(),
            reason: reasons[0].clone(),
            converter_recorded,
            evidence: plan.evidence.clone(),
        });
    }
    build(outcome, reasons, conversion, Some(plan), source_freshness)
}

#[cfg(test)]
mod tests;
