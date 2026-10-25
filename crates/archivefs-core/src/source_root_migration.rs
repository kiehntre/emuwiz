//! Read-only source-root migration contract.
//!
//! This module deliberately plans references, rather than applying them.  A
//! subsystem must opt in (`migratable`) before a path can ever be proposed for
//! rebasing; absolute paths are not evidence that a path belongs to a library.
//! The planner uses exact path components only: it does not use names, hashes,
//! or fuzzy matching.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PathSemantics {
    SourceRelative,
    SourceRootAbsolute,
    DestinationRootAbsolute,
    ExternalProviderPath,
    CacheInternal,
    TransactionHistorical,
    UserSelectedExternal,
    Temporary,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MigrationClassification {
    SafeRebase,
    AlreadyCurrent,
    TargetMissing,
    OutsideNewRoot,
    Ambiguous,
    HistoricalOnly,
    RegenerateInstead,
    ManualReview,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MigrationReference {
    pub id: String,
    pub subsystem: String,
    pub path: PathBuf,
    pub semantics: PathSemantics,
    /// The subsystem has explicitly proved that this field is a library path.
    pub migratable: bool,
    /// A live object must exist at the proposed path before apply is allowed.
    pub requires_existence: bool,
    /// Completed history is evidence and is never rewritten by this contract.
    pub historical: bool,
    /// A live/pending record is distinct from completed historical evidence.
    pub live: bool,
    /// Optional persisted destination claim used to detect contradictory state.
    pub claimed_destination: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MigrationProposal {
    pub reference_id: String,
    pub subsystem: String,
    pub old_path: PathBuf,
    pub candidate_path: Option<PathBuf>,
    pub classification: MigrationClassification,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRootMigration {
    pub old_root: PathBuf,
    pub new_root: PathBuf,
    pub affected_references: Vec<String>,
    pub proposals: Vec<MigrationProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubsystemSnapshot {
    pub subsystem: String,
    pub references: Vec<MigrationReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct MigrationTotals {
    pub safe_rebase: usize,
    pub regenerate_instead: usize,
    pub manual_review: usize,
    pub historical_or_non_migratable: usize,
    pub conflicts: usize,
    pub by_subsystem: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRootMigrationPlan {
    pub migration: SourceRootMigration,
    pub totals: MigrationTotals,
}

/// Plans an aggregate migration. This function performs metadata/existence
/// reads only; it never writes a file, database, cache, or configuration.
pub fn plan_source_root_migration(
    old_root: &Path,
    new_root: &Path,
    snapshots: &[SubsystemSnapshot],
) -> SourceRootMigrationPlan {
    let old_root = normalize_absolute(old_root).unwrap_or_else(|| old_root.to_path_buf());
    let new_root = normalize_absolute(new_root).unwrap_or_else(|| new_root.to_path_buf());
    let mut proposals = Vec::new();
    let mut affected = Vec::new();

    for snapshot in snapshots {
        for reference in &snapshot.references {
            let proposal = plan_reference(&old_root, &new_root, reference, snapshots);
            if !matches!(
                proposal.classification,
                MigrationClassification::NotApplicable
            ) {
                affected.push(reference.id.clone());
            }
            proposals.push(proposal);
        }
    }
    proposals.sort_by(|a, b| {
        a.subsystem
            .cmp(&b.subsystem)
            .then(a.reference_id.cmp(&b.reference_id))
    });
    affected.sort();
    let mut totals = MigrationTotals::default();
    for proposal in &proposals {
        let count = totals
            .by_subsystem
            .entry(proposal.subsystem.clone())
            .or_default();
        *count += 1;
        match proposal.classification {
            MigrationClassification::SafeRebase => totals.safe_rebase += 1,
            MigrationClassification::RegenerateInstead => totals.regenerate_instead += 1,
            MigrationClassification::ManualReview | MigrationClassification::Ambiguous => {
                totals.manual_review += 1
            }
            MigrationClassification::HistoricalOnly | MigrationClassification::NotApplicable => {
                totals.historical_or_non_migratable += 1
            }
            MigrationClassification::OutsideNewRoot | MigrationClassification::TargetMissing => {
                totals.manual_review += 1
            }
            MigrationClassification::AlreadyCurrent => {}
        }
    }
    totals.conflicts = proposals
        .iter()
        .filter(|p| p.classification == MigrationClassification::Ambiguous)
        .count();
    SourceRootMigrationPlan {
        migration: SourceRootMigration {
            old_root,
            new_root,
            affected_references: affected,
            proposals,
        },
        totals,
    }
}

/// Plans one explicit reference. Kept public so adapters can reuse the exact
/// proof rules without adopting the aggregate snapshot representation.
pub fn plan_rebase(
    old_root: &Path,
    new_root: &Path,
    reference: &MigrationReference,
) -> MigrationProposal {
    let old_root = normalize_absolute(old_root).unwrap_or_else(|| old_root.to_path_buf());
    let new_root = normalize_absolute(new_root).unwrap_or_else(|| new_root.to_path_buf());
    plan_reference(&old_root, &new_root, reference, &[])
}

fn plan_reference(
    old_root: &Path,
    new_root: &Path,
    r: &MigrationReference,
    snapshots: &[SubsystemSnapshot],
) -> MigrationProposal {
    let base = |classification, reason: &str, candidate| MigrationProposal {
        reference_id: r.id.clone(),
        subsystem: r.subsystem.clone(),
        old_path: r.path.clone(),
        candidate_path: candidate,
        classification,
        reason: reason.to_string(),
    };
    if r.historical || r.semantics == PathSemantics::TransactionHistorical {
        return base(
            MigrationClassification::HistoricalOnly,
            "Completed or historical transaction evidence is preserved.",
            None,
        );
    }
    if !r.migratable {
        let classification = match r.semantics {
            PathSemantics::CacheInternal => MigrationClassification::RegenerateInstead,
            PathSemantics::UserSelectedExternal
            | PathSemantics::Temporary
            | PathSemantics::ExternalProviderPath
            | PathSemantics::DestinationRootAbsolute => MigrationClassification::NotApplicable,
            _ => MigrationClassification::ManualReview,
        };
        return base(
            classification,
            "The subsystem has not declared this reference safe to migrate.",
            None,
        );
    }
    let Some(old_path) = normalize_absolute(&r.path) else {
        return base(
            MigrationClassification::ManualReview,
            "Path normalization or absolute-path proof failed.",
            None,
        );
    };
    if r.requires_existence
        && fs::symlink_metadata(&r.path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return base(
            MigrationClassification::ManualReview,
            "The source reference is a symlink; its target ownership is not proven.",
            None,
        );
    }
    let Some(relative) = old_path.strip_prefix(old_root).ok() else {
        if normalize_absolute(&r.path).as_deref() == Some(new_root)
            || old_path.starts_with(new_root)
        {
            return base(
                MigrationClassification::AlreadyCurrent,
                "Reference is already under the new source root.",
                Some(old_path),
            );
        }
        return base(
            MigrationClassification::OutsideNewRoot,
            "Reference is not under the old source root.",
            None,
        );
    };
    let Some(candidate) = join_checked(new_root, relative) else {
        return base(
            MigrationClassification::OutsideNewRoot,
            "The preserved suffix escapes the new root.",
            None,
        );
    };
    if candidate == old_path && old_root == new_root {
        return base(
            MigrationClassification::AlreadyCurrent,
            "Source roots are identical.",
            Some(candidate),
        );
    }
    if r.claimed_destination
        .as_ref()
        .and_then(|p| normalize_absolute(p))
        .is_some_and(|p| p != candidate)
    {
        return base(
            MigrationClassification::Ambiguous,
            "Persisted destination claim conflicts with the exact rebased candidate.",
            Some(candidate),
        );
    }
    if snapshots
        .iter()
        .flat_map(|s| s.references.iter())
        .any(|other| {
            other.id != r.id
                && normalize_absolute(&other.path).as_ref() == Some(&candidate)
                && other
                    .claimed_destination
                    .as_ref()
                    .is_some_and(|p| normalize_absolute(p).as_ref() != Some(&candidate))
        })
    {
        return base(
            MigrationClassification::Ambiguous,
            "Another persisted reference claims a different destination.",
            Some(candidate),
        );
    }
    if r.requires_existence && !exists_without_symlink_escape(&candidate, new_root) {
        return base(
            MigrationClassification::TargetMissing,
            "The exact candidate does not exist under the new root.",
            Some(candidate),
        );
    }
    base(
        MigrationClassification::SafeRebase,
        "Exact old-root containment, suffix preservation, destination containment, and required existence are proven.",
        Some(candidate),
    )
}

fn normalize_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(v) => out.push(v),
        }
    }
    Some(out)
}

fn join_checked(root: &Path, suffix: &Path) -> Option<PathBuf> {
    let candidate = normalize_absolute(&root.join(suffix))?;
    candidate.starts_with(root).then_some(candidate)
}

fn exists_without_symlink_escape(candidate: &Path, root: &Path) -> bool {
    if fs::symlink_metadata(root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return false;
    }
    if !candidate.exists() {
        return false;
    }
    match (fs::canonicalize(candidate), fs::canonicalize(root)) {
        (Ok(c), Ok(r)) => c.starts_with(r),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
