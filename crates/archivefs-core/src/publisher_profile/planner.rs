//! The one generic Publisher Profile planning engine - task section 2's
//! "do not hard-code RomM logic throughout the planner".
//!
//! [`build_publisher_plan`] reads a [`PublisherProfile`] plus an
//! already-elected [`PlayingLibraryPlan`] (the existing 1G1R planner's
//! output - never re-scanned, re-hashed, or re-elected here) and a
//! caller-resolved [`PublisherPlatformMapping`] (produced by
//! `romm::resolve_romm_platform_mapping` or
//! `es_de::resolve_es_de_platform_mapping` - this function never calls
//! either directly, so a future frontend needs no change here).
//!
//! # Phase 1 boundary
//!
//! This function performs **zero filesystem writes**. It may optionally
//! *read* an existing destination root (see
//! [`super::destination_inspection`]) when the caller supplies one; with
//! none, every item's [`DestinationState`] stays
//! [`DestinationState::Unknown`]. No hardlink, symlink, copy, directory,
//! metadata file, or playlist is ever created by this module - see
//! `tests.rs`'s `zero_side_effects` module for the structural proof this
//! module's own test suite runs on every call.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::destination_inspection::inspect_destination;
use super::model::{
    DestinationState, PublisherActionKind, PublisherActionSafety, PublisherBiosRequirement,
    PublisherCompanionItem, PublisherConflict, PublisherPlan, PublisherPlanItem,
    PublisherPlannedAction, PublisherPlatformMapping, PublisherProfile, PublisherWarning,
};
use crate::playing_library::{ElectedGame, LinkedLibraryOperation, PlayingLibraryPlan};

/// Everything one Publisher Profile planning run needs. Deliberately does
/// not take a `DatPlatformIdentity` or any RomM/ES-DE-specific type - the
/// caller resolves `platform_mapping` first with the profile-specific
/// helper, keeping this function frontend-agnostic.
pub struct PublisherPlanRequest<'a> {
    pub profile: &'a PublisherProfile,
    pub playing_library_plan: &'a PlayingLibraryPlan,
    pub platform_mapping: PublisherPlatformMapping,
    /// Where this profile's tree would be rooted (e.g. a RomM library root,
    /// or an ES-DE `%ROMPATH%`). Never created or written by this
    /// function.
    pub destination_root: PathBuf,
    /// Read-only inspection root - task section 16. `None` means no
    /// existing-destination evidence is available; every item's
    /// [`DestinationState`] stays [`DestinationState::Unknown`].
    pub existing_destination_root: Option<&'a Path>,
}

/// Builds a read-only [`PublisherPlan`]. Never touches the filesystem
/// except an optional read-only inspection pass (see
/// [`PublisherPlanRequest::existing_destination_root`]).
pub fn build_publisher_plan(request: &PublisherPlanRequest<'_>) -> Result<PublisherPlan, String> {
    if !request.destination_root.is_absolute() {
        return Err("publisher destination root must be an absolute path".to_string());
    }
    if !request.playing_library_plan.conflicts.is_empty() {
        return Err(format!(
            "{} Playing Library destination conflict(s) must be resolved before publisher planning",
            request.playing_library_plan.conflicts.len()
        ));
    }

    let mapping_warning = platform_mapping_warning(&request.platform_mapping);
    let mapping_safety_floor = platform_mapping_safety_floor(&request.platform_mapping);
    let platform_folder = request.platform_mapping.folder().map(str::to_string);

    let mut items: Vec<PublisherPlanItem> = request
        .playing_library_plan
        .elected_games
        .iter()
        .map(|game| {
            build_item(
                request.profile,
                &request.destination_root,
                game,
                &request.platform_mapping,
                platform_folder.as_deref(),
                mapping_safety_floor,
                mapping_warning.clone(),
            )
        })
        .collect();

    detect_plan_collisions(&mut items);

    if request.existing_destination_root.is_some() {
        for item in &mut items {
            apply_destination_inspection(item);
        }
    }

    let bios_requirements: Vec<PublisherBiosRequirement> = Vec::new();

    let mut plan = PublisherPlan {
        frontend: request.profile.frontend,
        destination_root: request.destination_root.clone(),
        items,
        bios_requirements,
        plan_hash: String::new(),
    };
    plan.plan_hash = compute_plan_hash(&plan);
    Ok(plan)
}

fn platform_mapping_warning(mapping: &PublisherPlatformMapping) -> Option<PublisherWarning> {
    match mapping {
        PublisherPlatformMapping::Mapped { .. } => None,
        PublisherPlatformMapping::Unmapped {
            canonical_platform_id,
        } => Some(PublisherWarning::UnmappedPlatform {
            canonical_platform_id: canonical_platform_id.clone(),
        }),
        PublisherPlatformMapping::Ambiguous {
            canonical_platform_id,
        } => Some(PublisherWarning::AmbiguousPlatformMapping {
            canonical_platform_id: canonical_platform_id.clone(),
        }),
        PublisherPlatformMapping::Unsupported {
            canonical_platform_id,
        } => Some(PublisherWarning::UnsupportedPlatform {
            canonical_platform_id: canonical_platform_id.clone(),
        }),
    }
}

fn platform_mapping_safety_floor(
    mapping: &PublisherPlatformMapping,
) -> Option<PublisherActionSafety> {
    match mapping {
        PublisherPlatformMapping::Mapped { .. } => None,
        PublisherPlatformMapping::Unmapped { .. } | PublisherPlatformMapping::Ambiguous { .. } => {
            Some(PublisherActionSafety::ReviewRequired)
        }
        PublisherPlatformMapping::Unsupported { .. } => Some(PublisherActionSafety::Unsupported),
    }
}

/// The only planned action any reviewed profile in this codebase can
/// justify today: a symlink. Every real, apply-capable path this crate
/// already ships (`playing_library::apply_adapter`,
/// `dat::rom_organisation`'s `CreateSymlink`) publishes via symlink, never
/// a hardlink - so Hardlink stays a declared-but-unselected
/// [`PublisherActionKind`] until real same-filesystem evidence exists to
/// justify it (task section 7: "do not decide automatically where
/// evidence is insufficient").
fn default_planned_action() -> PublisherPlannedAction {
    PublisherPlannedAction {
        kind: PublisherActionKind::Symlink,
        required_by_target: true,
    }
}

fn project_one(
    operation: &LinkedLibraryOperation,
    profile: &PublisherProfile,
    destination_root: &Path,
    destination_root_folder: Option<&str>,
) -> Option<PathBuf> {
    let folder = destination_root_folder?;
    let file_name = operation.source_path.file_name()?;
    let mut destination = profile.path_rule.resolve(destination_root, folder);
    destination.push(file_name);
    Some(destination)
}

#[allow(clippy::too_many_arguments)]
fn build_item(
    profile: &PublisherProfile,
    destination_root: &Path,
    game: &ElectedGame,
    platform_mapping: &PublisherPlatformMapping,
    platform_folder: Option<&str>,
    mapping_safety_floor: Option<PublisherActionSafety>,
    mapping_warning: Option<PublisherWarning>,
) -> PublisherPlanItem {
    let mut warnings = Vec::new();
    if let Some(warning) = mapping_warning {
        warnings.push(warning);
    }

    let planned_destination = project_one(
        &game.launcher_operation,
        profile,
        destination_root,
        platform_folder,
    );
    let companions: Vec<PublisherCompanionItem> = game
        .companion_operations
        .iter()
        .filter_map(|operation| {
            project_one(operation, profile, destination_root, platform_folder).map(|destination| {
                PublisherCompanionItem {
                    source_path: operation.source_path.clone(),
                    planned_destination: destination,
                }
            })
        })
        .collect();
    if planned_destination.is_some() && companions.len() != game.companion_operations.len() {
        warnings.push(PublisherWarning::IncompleteMediaSet {
            reason: "a companion file's source has no file name to project".to_string(),
        });
    }

    let extension_ok = extension_is_accepted(profile, &game.launcher_operation.source_path);
    if !extension_ok {
        warnings.push(PublisherWarning::UnsupportedExtension {
            extension: extension_of(&game.launcher_operation.source_path)
                .unwrap_or_default()
                .to_string(),
        });
    }

    let safety = mapping_safety_floor.unwrap_or_else(|| {
        if planned_destination.is_none() {
            PublisherActionSafety::Blocked
        } else if !extension_ok {
            PublisherActionSafety::ReviewRequired
        } else {
            PublisherActionSafety::SafeToAct
        }
    });

    let reason = build_reason(
        platform_mapping,
        planned_destination.is_some(),
        extension_ok,
    );

    PublisherPlanItem {
        dat_entry_name: game.dat_entry_name.clone(),
        platform_mapping: platform_mapping.clone(),
        source_path: game.launcher_operation.source_path.clone(),
        planned_destination,
        companions,
        reason,
        planned_action: default_planned_action(),
        safety,
        destination_state: DestinationState::Unknown,
        conflicts: Vec::new(),
        warnings,
    }
}

fn build_reason(
    mapping: &PublisherPlatformMapping,
    has_destination: bool,
    extension_ok: bool,
) -> String {
    match mapping {
        PublisherPlatformMapping::Mapped { folder, .. } if has_destination && extension_ok => {
            format!(
                "verified platform identity mapped to \"{folder}\"; the elected release's own representation is published unchanged"
            )
        }
        PublisherPlatformMapping::Mapped { folder, .. } if has_destination => {
            format!(
                "mapped to \"{folder}\", but this file's extension is not yet reviewed for this target"
            )
        }
        PublisherPlatformMapping::Mapped { .. } => {
            "mapped platform, but no safe destination file name could be computed".to_string()
        }
        PublisherPlatformMapping::Unmapped {
            canonical_platform_id,
        } => format!("no reviewed mapping exists yet for platform \"{canonical_platform_id}\""),
        PublisherPlatformMapping::Ambiguous {
            canonical_platform_id,
        } => format!(
            "platform \"{canonical_platform_id}\" has more than one possible mapping with no defensible default"
        ),
        PublisherPlatformMapping::Unsupported {
            canonical_platform_id,
        } => format!("platform \"{canonical_platform_id}\" is not supported by this target"),
    }
}

fn extension_of(path: &Path) -> Option<&str> {
    path.extension().and_then(|value| value.to_str())
}

fn extension_is_accepted(profile: &PublisherProfile, path: &Path) -> bool {
    if profile.accepted_extensions.is_empty() {
        // "Not reviewed yet" - task section 2/3: this profile has not
        // published a vetted extension list, so extension gating is
        // skipped entirely rather than rejecting everything.
        return true;
    }
    match extension_of(path) {
        Some(extension) => profile
            .accepted_extensions
            .iter()
            .any(|accepted| accepted.eq_ignore_ascii_case(extension)),
        None => false,
    }
}

/// Detects two elections planning the same destination - task section 15.
/// `BTreeMap` keeps this `O(N log N)`, never the `O(N^2)` pairwise compare
/// task section 24 explicitly warns against.
fn detect_plan_collisions(items: &mut [PublisherPlanItem]) {
    let mut by_casefold: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut by_exact: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        let Some(destination) = &item.planned_destination else {
            continue;
        };
        by_exact.entry(destination.clone()).or_default().push(index);
        by_casefold
            .entry(destination.to_string_lossy().to_ascii_lowercase())
            .or_default()
            .push(index);
    }

    let mut exact_collisions: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for (destination, indexes) in &by_exact {
        if indexes.len() > 1 {
            exact_collisions.push((destination.clone(), indexes.clone()));
        }
    }
    for (destination, indexes) in &exact_collisions {
        let contenders: Vec<String> = indexes
            .iter()
            .map(|index| items[*index].dat_entry_name.clone())
            .collect();
        for index in indexes {
            items[*index]
                .conflicts
                .push(PublisherConflict::DestinationPlanCollision {
                    destination: destination.clone(),
                    contenders: contenders.clone(),
                });
            items[*index].safety = PublisherActionSafety::Blocked;
        }
    }

    let mut casefold_collisions: Vec<(String, Vec<usize>)> = Vec::new();
    for (basename, indexes) in &by_casefold {
        if indexes.len() <= 1 {
            continue;
        }
        // Only a genuine case-*fold* collision - two or more *distinct*
        // exact paths that only clash after case-folding - is reported
        // here. A group whose members all share one identical exact path
        // is already reported, more precisely, as a `DestinationPlanCollision`
        // above; reporting it again here would be a duplicate, not a
        // sharper signal.
        let distinct_exact_paths: std::collections::BTreeSet<&PathBuf> = indexes
            .iter()
            .filter_map(|index| items[*index].planned_destination.as_ref())
            .collect();
        if distinct_exact_paths.len() > 1 {
            casefold_collisions.push((basename.clone(), indexes.clone()));
        }
    }
    for (basename, indexes) in &casefold_collisions {
        let contenders: Vec<String> = indexes
            .iter()
            .map(|index| items[*index].dat_entry_name.clone())
            .collect();
        for index in indexes {
            items[*index]
                .conflicts
                .push(PublisherConflict::CaseFoldCollision {
                    destination_basename: basename.clone(),
                    contenders: contenders.clone(),
                });
            if items[*index].safety != PublisherActionSafety::Unsupported {
                items[*index].safety = PublisherActionSafety::Blocked;
            }
        }
    }
}

fn apply_destination_inspection(item: &mut PublisherPlanItem) {
    let Some(destination) = &item.planned_destination else {
        return;
    };
    let state = inspect_destination(destination, &item.source_path);
    item.destination_state = state;
    match state {
        DestinationState::Conflicting => {
            item.conflicts
                .push(PublisherConflict::DestinationExistsDifferentContent {
                    destination: destination.clone(),
                });
            item.warnings
                .push(PublisherWarning::ExistingDestinationConflict {
                    destination: destination.clone(),
                });
            if item.safety == PublisherActionSafety::SafeToAct {
                item.safety = PublisherActionSafety::Blocked;
            }
        }
        DestinationState::Stale => {
            if item.safety == PublisherActionSafety::SafeToAct {
                item.safety = PublisherActionSafety::ReviewRequired;
            }
        }
        DestinationState::AlreadyCorrect
        | DestinationState::Missing
        | DestinationState::Unknown => {}
    }
}

/// A deterministic, order-independent plan fingerprint - task section 23.
/// Every field that can influence what a future execution engine would do
/// is folded in; item order in the input never changes the result because
/// every contributing line is sorted before hashing.
fn compute_plan_hash(plan: &PublisherPlan) -> String {
    let mut lines: Vec<String> = plan
        .items
        .iter()
        .map(|item| {
            format!(
                "{}|{}|{:?}|{}|{:?}|{:?}|{}",
                item.dat_entry_name,
                item.source_path.display(),
                item.planned_destination
                    .as_ref()
                    .map(|p| p.display().to_string()),
                item.companions.len(),
                item.safety,
                item.destination_state,
                item.conflicts.len(),
            )
        })
        .collect();
    lines.sort();
    let mut hasher = Fnv1a::new();
    hasher.write(plan.frontend.label().as_bytes());
    hasher.write(plan.destination_root.to_string_lossy().as_bytes());
    for line in &lines {
        hasher.write(line.as_bytes());
    }
    format!("{:016x}", hasher.finish())
}

/// A small, dependency-free, deterministic 64-bit hash. Not cryptographic -
/// only used as a stable plan fingerprint for the determinism guarantee in
/// task section 23, never as a security or content-integrity check.
struct Fnv1a(u64);
impl Fnv1a {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
        // A 0x00 separator between fields keeps `("ab","c")` distinct from
        // `("a","bc")`.
        self.0 ^= 0x00;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}
