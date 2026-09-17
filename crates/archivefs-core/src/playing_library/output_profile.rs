//! Shared, read-only output-profile projections for an elected Playing Library.
//!
//! This module deliberately sits after the 1G1R planner. It changes only the
//! proposed destination paths; it never scans, hashes, elects, writes, or
//! persists. Apply continues to use the existing journaled linked-library
//! transaction builder.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::dat::identity::{DatPlatformConfidence, DatPlatformIdentity};

use super::{DestinationConflict, ElectedGame, LinkedLibraryOperation, PlayingLibraryPlan};

/// The four output profiles exposed by Library Organisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryOutputProfile {
    Generic,
    Romm,
    EsDe,
    RetroDeck,
}

impl LibraryOutputProfile {
    pub fn label(self) -> &'static str {
        match self {
            Self::Generic => "Generic",
            Self::Romm => "RomM",
            Self::EsDe => "ES-DE",
            Self::RetroDeck => "RetroDECK",
        }
    }
}

/// One common, presentation-only view of any output projection.
///
/// `already_correct` and `stale_owned_entries` remain zero/empty when the
/// caller has not supplied a read-only ownership scan. They are fields rather
/// than inferred values so the GUI cannot accidentally claim filesystem state
/// that was never inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryOutputProjection {
    pub profile: LibraryOutputProfile,
    pub destination_root: PathBuf,
    pub planned_directories: Vec<PathBuf>,
    pub planned_links: Vec<LinkedLibraryOperation>,
    pub already_correct: usize,
    pub conflicts: Vec<DestinationConflict>,
    pub unresolved_mappings: Vec<String>,
    pub stale_owned_entries: Vec<PathBuf>,
    pub sample_paths: Vec<PathBuf>,
    pub operation_count: usize,
}

impl LibraryOutputProjection {
    pub fn from_plan(
        profile: LibraryOutputProfile,
        destination_root: PathBuf,
        plan: &PlayingLibraryPlan,
        unresolved_mappings: Vec<String>,
    ) -> Self {
        let mut directories = BTreeSet::new();
        for operation in &plan.operations {
            if let Some(parent) = operation.destination_path.parent() {
                directories.insert(parent.to_path_buf());
            }
        }
        let planned_links = plan.operations.clone();
        let sample_paths = planned_links
            .iter()
            .take(5)
            .map(|operation| operation.destination_path.clone())
            .collect();
        Self {
            profile,
            destination_root,
            planned_directories: directories.into_iter().collect(),
            operation_count: planned_links.len(),
            planned_links,
            already_correct: 0,
            conflicts: plan.conflicts.clone(),
            unresolved_mappings,
            stale_owned_entries: Vec::new(),
            sample_paths,
        }
    }
}

/// Projects an already-elected plan into the deterministic Generic layout:
/// `<destination>/<canonical-platform>/<filename>`.
///
/// Platform identity is authoritative input. Unknown, ambiguous, or weak
/// identity uses the explicit `unknown` folder; it is never promoted to a
/// platform-specific compatibility claim. Empty or path-shaped values fail.
pub fn project_generic_playing_library(
    plan: &PlayingLibraryPlan,
    identity: &DatPlatformIdentity,
    destination_root: PathBuf,
) -> Result<PlayingLibraryPlan, String> {
    if !destination_root.is_absolute() {
        return Err("the Generic destination root must be an absolute path".to_string());
    }
    let platform = match identity {
        DatPlatformIdentity::Resolved {
            platform,
            confidence: DatPlatformConfidence::Strong,
            ..
        } => platform,
        DatPlatformIdentity::Resolved { .. }
        | DatPlatformIdentity::Unknown
        | DatPlatformIdentity::Ambiguous { .. } => "unknown",
    };
    let platform_path = safe_platform_component(platform)?;
    project_plan(plan, destination_root.join(platform_path))
}

fn safe_platform_component(platform: &str) -> Result<PathBuf, String> {
    let path = Path::new(platform.trim());
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(format!(
            "platform {platform:?} is not a safe single Generic folder name"
        ));
    }
    Ok(path.to_path_buf())
}

fn project_plan(
    plan: &PlayingLibraryPlan,
    destination_root: PathBuf,
) -> Result<PlayingLibraryPlan, String> {
    let operations = plan
        .operations
        .iter()
        .map(|operation| project_operation(operation, &plan.destination_root, &destination_root))
        .collect::<Result<Vec<_>, _>>()?;
    let elected_games = plan
        .elected_games
        .iter()
        .map(|game| {
            Ok(ElectedGame {
                dat_entry_name: game.dat_entry_name.clone(),
                family_root_name: game.family_root_name.clone(),
                explanation: game.explanation.clone(),
                launcher_operation: project_operation(
                    &game.launcher_operation,
                    &plan.destination_root,
                    &destination_root,
                )?,
                companion_operations: game
                    .companion_operations
                    .iter()
                    .map(|operation| {
                        project_operation(operation, &plan.destination_root, &destination_root)
                    })
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut projected = plan.clone();
    projected.destination_root = destination_root;
    projected.operations = operations;
    projected.elected_games = elected_games;
    Ok(projected)
}

fn project_operation(
    operation: &LinkedLibraryOperation,
    source_root: &Path,
    destination_root: &Path,
) -> Result<LinkedLibraryOperation, String> {
    let relative = operation
        .destination_path
        .strip_prefix(source_root)
        .map_err(|_| "Playing Library destination escaped its configured root".to_string())?;
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Playing Library destination is not a safe relative path".to_string());
    }
    Ok(LinkedLibraryOperation {
        source_path: operation.source_path.clone(),
        destination_path: destination_root.join(relative),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::identity::DatPlatformConfidence;
    use crate::playing_library::{
        CandidateEvidenceSummary, ElectionExplanation, PlayingLibraryPolicy,
    };

    fn identity(platform: &str) -> DatPlatformIdentity {
        DatPlatformIdentity::Resolved {
            platform: platform.into(),
            machine_key: None,
            confidence: DatPlatformConfidence::Strong,
            evidence: vec![],
        }
    }

    fn plan(root: &Path) -> PlayingLibraryPlan {
        let source = root.join("Game With Spaces.zip");
        let operation = LinkedLibraryOperation {
            source_path: source,
            destination_path: root.join("Game With Spaces.zip"),
        };
        PlayingLibraryPlan {
            destination_root: root.to_path_buf(),
            policy: PlayingLibraryPolicy::default(),
            archives_examined: 1,
            families_examined: 1,
            elected_games: vec![ElectedGame {
                dat_entry_name: "Game".into(),
                family_root_name: "Game".into(),
                explanation: ElectionExplanation {
                    steps: vec![],
                    rejected: vec![],
                    winner_evidence: CandidateEvidenceSummary::unknown(),
                },
                launcher_operation: operation.clone(),
                companion_operations: vec![],
            }],
            unresolved_groups: vec![],
            exclusions: vec![],
            singleton_families: 1,
            conflicts: vec![],
            operations: vec![operation],
            rejected_launchers: vec![],
        }
    }

    #[test]
    fn generic_projection_uses_one_platform_folder_and_preserves_one_token_paths() {
        let root = Path::new("/tmp/playing library");
        let projected = project_generic_playing_library(
            &plan(root),
            &identity("Game Boy Advance"),
            root.to_path_buf(),
        )
        .unwrap();
        assert_eq!(
            projected.operations[0].destination_path,
            root.join("Game Boy Advance/Game With Spaces.zip")
        );
        assert_eq!(
            projected.operations[0].source_path,
            root.join("Game With Spaces.zip")
        );
    }

    #[test]
    fn generic_projection_uses_an_explicit_unknown_folder_and_refuses_path_shaped_platforms() {
        let root = Path::new("/tmp/playing");
        let projected = project_generic_playing_library(
            &plan(root),
            &DatPlatformIdentity::Unknown,
            root.to_path_buf(),
        )
        .unwrap();
        assert_eq!(
            projected.operations[0].destination_path,
            root.join("unknown/Game With Spaces.zip")
        );
        assert!(project_generic_playing_library(
            &plan(root),
            &identity("../escape"),
            root.to_path_buf()
        )
        .is_err());
    }

    #[test]
    fn every_profile_uses_the_same_common_projection_shape() {
        let root = Path::new("/tmp/playing");
        let plan = plan(root);
        for profile in [
            LibraryOutputProfile::Generic,
            LibraryOutputProfile::Romm,
            LibraryOutputProfile::EsDe,
            LibraryOutputProfile::RetroDeck,
        ] {
            let projection =
                LibraryOutputProjection::from_plan(profile, root.to_path_buf(), &plan, Vec::new());
            assert_eq!(projection.profile, profile);
            assert_eq!(projection.operation_count, plan.operations.len());
            assert_eq!(projection.planned_links, plan.operations);
            assert_eq!(projection.sample_paths.len(), 1);
        }
    }
}
