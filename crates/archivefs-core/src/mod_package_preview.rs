//! Read-only projection for ordinary local mod package previews.
//!
//! Archive parsing and destination inspection remain in `archive_mod_package`.
//! This module only turns those facts into a novice-facing, stable model and
//! compares independently inspected packages. It never executes package
//! content and never writes a package or destination.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::archive_mod_package::{ArchiveModFileAction, ArchiveModPackagePlan};
use crate::mod_package::{ModPlanBlockerKind, ProposedFileState};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPackageConflictKind {
    ModVsOriginal,
    ModVsModSameContent,
    ModVsModDifferentContent,
    CaseCollision,
    DestinationAlreadyModified,
    UnknownDestination,
    ExecutableInstaller,
    ArchiveUnsafePath,
    AmbiguousTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPackageReadiness {
    PreviewSafe,
    ReadyToApply,
    NeedsReview,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPackageRisk {
    OriginalReplacement,
    ExistingModification,
    MultipleModCollision,
    InstallerPresent,
    UnsafePackage,
    TargetUnproven,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModPackagePreviewEntry {
    pub package_id: String,
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub operation: ArchiveModFileAction,
    pub destination_state: ProposedFileState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModPackageConflict {
    pub kind: ModPackageConflictKind,
    pub destination_path: Option<PathBuf>,
    pub packages: Vec<String>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModInstallationProjection {
    pub package_id: String,
    pub package_path: PathBuf,
    pub target_platform: String,
    pub target_proven: bool,
    pub entries: Vec<ModPackagePreviewEntry>,
    pub conflicts: Vec<ModPackageConflict>,
    pub risks: Vec<ModPackageRisk>,
    pub warnings: Vec<String>,
    pub readiness: ModPackageReadiness,
    pub source_unchanged: bool,
}

/// Projects one existing archive/folder inspection. The plan remains the
/// source of truth for hashes, path confinement, and transaction eligibility.
pub fn project_archive_mod_package(
    package_id: impl Into<String>,
    plan: &ArchiveModPackagePlan,
) -> ModInstallationProjection {
    let package_id = package_id.into();
    let entries = plan
        .files
        .iter()
        .filter(|file| !file.metadata)
        .map(|file| ModPackagePreviewEntry {
            package_id: package_id.clone(),
            source_path: file.package_path.clone(),
            destination_path: file.destination_relative_path.clone(),
            size: file.size,
            sha256: file.sha256.clone(),
            operation: file.action,
            destination_state: file.destination_state,
        })
        .collect::<Vec<_>>();

    let mut conflicts = plan
        .inspection
        .conflicts
        .iter()
        .map(|conflict| ModPackageConflict {
            kind: ModPackageConflictKind::UnknownDestination,
            destination_path: Some(conflict.destination_path.clone()),
            packages: vec![package_id.clone()],
            detail: conflict.detail.clone(),
        })
        .collect::<Vec<_>>();
    let mut risks = Vec::new();
    for entry in &entries {
        if entry.operation == ArchiveModFileAction::Replace {
            conflicts.push(ModPackageConflict {
                kind: ModPackageConflictKind::ModVsOriginal,
                destination_path: Some(entry.destination_path.clone()),
                packages: vec![package_id.clone()],
                detail: "the package would replace an existing file; the original must be backed up by the shared transaction".into(),
            });
            risks.push(ModPackageRisk::OriginalReplacement);
        }
        if is_installer_or_script(&entry.destination_path) {
            conflicts.push(ModPackageConflict {
                kind: ModPackageConflictKind::ExecutableInstaller,
                destination_path: Some(entry.destination_path.clone()),
                packages: vec![package_id.clone()],
                detail: "package content is displayed only; EmuWiz will never execute it".into(),
            });
            risks.push(ModPackageRisk::InstallerPresent);
        }
    }
    for blocker in &plan.inspection.blockers {
        let kind = match blocker.kind {
            ModPlanBlockerKind::PackagePathUnsafe
            | ModPlanBlockerKind::UnsafeSymlink
            | ModPlanBlockerKind::UnsafePackageEntry
            | ModPlanBlockerKind::PayloadPathUnsafe
            | ModPlanBlockerKind::DestinationPathUnsafe
            | ModPlanBlockerKind::DestinationEscapesGameRoot => {
                ModPackageConflictKind::ArchiveUnsafePath
            }
            ModPlanBlockerKind::GameIdentityUnknown
            | ModPlanBlockerKind::GameIdentityAmbiguous
            | ModPlanBlockerKind::GameIdentityConflicting
            | ModPlanBlockerKind::GameIdentityMismatch
            | ModPlanBlockerKind::PlatformUnknown
            | ModPlanBlockerKind::PlatformMismatch => ModPackageConflictKind::AmbiguousTarget,
            _ => ModPackageConflictKind::UnknownDestination,
        };
        conflicts.push(ModPackageConflict {
            kind,
            destination_path: None,
            packages: vec![package_id.clone()],
            detail: blocker.detail.clone(),
        });
    }
    if conflicts
        .iter()
        .any(|item| item.kind == ModPackageConflictKind::ArchiveUnsafePath)
    {
        risks.push(ModPackageRisk::UnsafePackage);
    }
    if conflicts
        .iter()
        .any(|item| item.kind == ModPackageConflictKind::AmbiguousTarget)
    {
        risks.push(ModPackageRisk::TargetUnproven);
    }
    dedupe(&mut risks);
    conflicts.sort_by(|a, b| {
        a.destination_path
            .cmp(&b.destination_path)
            .then(a.kind.cmp(&b.kind))
    });
    let target_proven = plan.inspection.blockers.iter().all(|item| {
        !matches!(
            item.kind,
            ModPlanBlockerKind::GameIdentityUnknown
                | ModPlanBlockerKind::GameIdentityAmbiguous
                | ModPlanBlockerKind::GameIdentityConflicting
                | ModPlanBlockerKind::GameIdentityMismatch
                | ModPlanBlockerKind::PlatformUnknown
                | ModPlanBlockerKind::PlatformMismatch
        )
    });
    let readiness = if !plan.inspection.blockers.is_empty() {
        ModPackageReadiness::Refused
    } else if !plan.inspection.conflicts.is_empty() {
        ModPackageReadiness::NeedsReview
    } else if plan.eligible_for_apply() {
        ModPackageReadiness::ReadyToApply
    } else {
        ModPackageReadiness::PreviewSafe
    };
    ModInstallationProjection {
        package_id,
        package_path: plan.package_path.clone(),
        target_platform: format!("{:?}", plan.inspection.selected_game.platform),
        target_proven,
        entries,
        conflicts,
        risks,
        warnings: plan.inspection.warnings.clone(),
        readiness,
        source_unchanged: true,
    }
}

/// Compares already-inspected packages without selecting a load order.
pub fn compare_package_projections(
    projections: &[ModInstallationProjection],
) -> Vec<ModPackageConflict> {
    let mut by_path: BTreeMap<String, Vec<(&ModPackagePreviewEntry, &str)>> = BTreeMap::new();
    for projection in projections {
        for entry in &projection.entries {
            by_path
                .entry(
                    entry
                        .destination_path
                        .to_string_lossy()
                        .to_ascii_lowercase(),
                )
                .or_default()
                .push((entry, &projection.package_id));
        }
    }
    let mut conflicts = Vec::new();
    for entries in by_path.values() {
        let Some((first, first_package)) = entries.first() else {
            continue;
        };
        let exact_paths = entries
            .iter()
            .map(|(entry, _)| entry.destination_path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let case_collision = exact_paths.iter().any(|path| path != &exact_paths[0]);
        if case_collision {
            conflicts.push(ModPackageConflict {
                kind: ModPackageConflictKind::CaseCollision,
                destination_path: Some(first.destination_path.clone()),
                packages: entries
                    .iter()
                    .map(|(_, package)| (*package).to_string())
                    .collect(),
                detail: "package destinations differ only by case".into(),
            });
        }
        let distinct = entries
            .iter()
            .any(|(entry, _)| entry.sha256 != first.sha256);
        if entries.len() > 1 {
            conflicts.push(ModPackageConflict {
                kind: if distinct {
                    ModPackageConflictKind::ModVsModDifferentContent
                } else {
                    ModPackageConflictKind::ModVsModSameContent
                },
                destination_path: Some(first.destination_path.clone()),
                packages: entries
                    .iter()
                    .map(|(_, package)| (*package).to_string())
                    .collect(),
                detail: if distinct {
                    format!(
                        "{} and another package provide different bytes; choose an explicit order",
                        first_package
                    )
                } else {
                    "multiple packages provide identical bytes".into()
                },
            });
        }
    }
    conflicts
}

fn is_installer_or_script(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("exe" | "dll" | "bat" | "cmd" | "ps1" | "sh" | "py" | "msi" | "com")
    )
}

fn dedupe<T: Ord>(values: &mut Vec<T>) {
    values.sort();
    values.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(package: &str, path: &str, hash: &str) -> ModPackagePreviewEntry {
        ModPackagePreviewEntry {
            package_id: package.into(),
            source_path: PathBuf::from(path),
            destination_path: PathBuf::from(path),
            size: 1,
            sha256: hash.into(),
            operation: ArchiveModFileAction::Create,
            destination_state: ProposedFileState::Missing,
        }
    }

    fn projection(
        package: &str,
        entries: Vec<ModPackagePreviewEntry>,
    ) -> ModInstallationProjection {
        ModInstallationProjection {
            package_id: package.into(),
            package_path: PathBuf::from(package),
            target_platform: "Test".into(),
            target_proven: true,
            entries,
            conflicts: Vec::new(),
            risks: Vec::new(),
            warnings: Vec::new(),
            readiness: ModPackageReadiness::PreviewSafe,
            source_unchanged: true,
        }
    }

    #[test]
    fn identical_and_different_mod_collisions_are_distinct() {
        let a = projection("a", vec![entry("a", "Data/Foo.bin", "same")]);
        let b = projection("b", vec![entry("b", "Data/Foo.bin", "same")]);
        assert_eq!(
            compare_package_projections(&[a, b])[0].kind,
            ModPackageConflictKind::ModVsModSameContent
        );
        let a = projection("a", vec![entry("a", "Data/Foo.bin", "one")]);
        let b = projection("b", vec![entry("b", "Data/Foo.bin", "two")]);
        assert_eq!(
            compare_package_projections(&[a, b])[0].kind,
            ModPackageConflictKind::ModVsModDifferentContent
        );
    }

    #[test]
    fn case_collision_is_reported_without_inventing_order() {
        let a = projection("a", vec![entry("a", "Data/Foo.bin", "one")]);
        let b = projection("b", vec![entry("b", "data/foo.bin", "one")]);
        assert!(
            compare_package_projections(&[a, b])
                .iter()
                .any(|c| c.kind == ModPackageConflictKind::CaseCollision)
        );
    }

    #[test]
    fn installer_is_warning_only_and_never_an_execution_request() {
        let a = projection("a", vec![entry("a", "setup.exe", "one")]);
        assert!(is_installer_or_script(&a.entries[0].destination_path));
        assert!(a.source_unchanged);
    }
}
