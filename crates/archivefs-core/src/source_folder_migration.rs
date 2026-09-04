//! Read-only source-folder/database adapter.
//!
//! `source_folders.path` is authoritative source-root state and is the only
//! database path projected as a rebase candidate.  Archive paths are split by
//! their actual schema meaning: `relative_path` remains the stable observation
//! key, while `absolute_path_cached` is reported as recompute/regenerate-only.
//! This module has no database mutation path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Serialize;

use crate::database::{Database, PersistedArchive, SourceFolderRecord};
use crate::source_root_migration::{
    MigrationClassification, MigrationProposal, MigrationReference, PathSemantics,
    SourceRootMigrationPlan, SubsystemSnapshot, plan_source_root_migration,
};

/// Input snapshot for the adapter. It is public so callers with an already
/// opened read-only database can test or compose the projection without a
/// second database query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFolderMigrationInput {
    pub old_root: PathBuf,
    pub new_root: PathBuf,
    pub source_folders: Vec<SourceFolderRecord>,
    pub archives: Vec<PersistedArchive>,
    pub historical_paths: Vec<PathBuf>,
    /// If set, only this explicitly owned database source row is migratable.
    pub target_source_id: Option<i64>,
}

impl SourceFolderMigrationInput {
    /// Reads source-folder and archive snapshots through the database's
    /// read-only methods. The database handle is never mutably borrowed.
    pub fn from_database(
        database: &Database,
        old_root: impl Into<PathBuf>,
        new_root: impl Into<PathBuf>,
    ) -> crate::Result<Self> {
        Ok(Self {
            old_root: old_root.into(),
            new_root: new_root.into(),
            source_folders: database.list_source_folders()?,
            archives: database.load_archives()?,
            historical_paths: Vec::new(),
            target_source_id: None,
        })
    }
}

/// One archive observation that must be recomputed after source-root state
/// changes. Neither field is an apply target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveObservationReference {
    pub archive_id: i64,
    pub source_id: i64,
    pub relative_path: PathBuf,
    pub absolute_path_cached: PathBuf,
    pub classification: MigrationClassification,
    pub reason: String,
}

/// Adapter-specific counts, separated from generic source-root counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct SourceFolderMigrationTotals {
    pub source_folder_records: usize,
    pub migratable_source_roots: usize,
    pub source_roots_rebase: usize,
    pub source_roots_already_current: usize,
    pub source_roots_manual_review: usize,
    pub source_roots_unsupported: usize,
    pub archive_observations_to_recompute: usize,
    pub historical_references: usize,
    pub conflicts: usize,
}

/// Complete read-only adapter output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceFolderMigrationReport {
    pub source_roots: SourceRootMigrationPlan,
    pub archive_observations: Vec<ArchiveObservationReference>,
    pub historical: Vec<MigrationProposal>,
    pub totals: SourceFolderMigrationTotals,
}

/// Builds a deterministic, read-only source-folder/database migration report.
pub fn plan_source_folder_migration(
    input: &SourceFolderMigrationInput,
) -> SourceFolderMigrationReport {
    let owned_ids: BTreeSet<i64> = input
        .source_folders
        .iter()
        .filter(|row| input.target_source_id.is_none_or(|id| id == row.id))
        .map(|row| row.id)
        .collect();

    let references: Vec<MigrationReference> = input
        .source_folders
        .iter()
        .map(|row| MigrationReference {
            id: format!("source-folder:{}", row.id),
            subsystem: "source-folder-database".to_string(),
            path: row.path.clone(),
            semantics: PathSemantics::SourceRootAbsolute,
            migratable: owned_ids.contains(&row.id),
            requires_existence: true,
            historical: false,
            live: true,
            claimed_destination: None,
        })
        .collect();

    let mut root_plan = plan_source_root_migration(
        &input.old_root,
        &input.new_root,
        &[SubsystemSnapshot {
            subsystem: "source-folder-database".to_string(),
            references,
        }],
    );

    let roots_overlap =
        input.old_root.starts_with(&input.new_root) || input.new_root.starts_with(&input.old_root);
    if roots_overlap && input.old_root != input.new_root {
        for proposal in &mut root_plan.migration.proposals {
            if matches!(
                proposal.classification,
                MigrationClassification::SafeRebase
                    | MigrationClassification::AlreadyCurrent
                    | MigrationClassification::TargetMissing
                    | MigrationClassification::OutsideNewRoot
            ) {
                proposal.classification = MigrationClassification::Ambiguous;
                proposal.reason =
                    "Old and new source roots overlap; ownership cannot be proven safely."
                        .to_string();
            }
        }
    }

    // Two explicitly persisted source rows must never silently share one
    // destination. Compare only exact normalized candidates, never names.
    let mut by_candidate: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (index, proposal) in root_plan.migration.proposals.iter().enumerate() {
        if proposal.classification == MigrationClassification::SafeRebase {
            if let Some(candidate) = &proposal.candidate_path {
                by_candidate
                    .entry(candidate.clone())
                    .or_default()
                    .push(index);
            }
        }
    }
    let mut conflicts = 0;
    for indices in by_candidate.values().filter(|indices| indices.len() > 1) {
        conflicts += indices.len();
        for index in indices {
            let proposal = &mut root_plan.migration.proposals[*index];
            proposal.classification = MigrationClassification::Ambiguous;
            proposal.reason =
                "Multiple persisted source rows claim the same exact destination.".to_string();
        }
    }

    let mut archive_observations: Vec<_> = input
        .archives
        .iter()
        .filter(|archive| owned_ids.contains(&archive.source_folder_id))
        .map(|archive| ArchiveObservationReference {
            archive_id: archive.id,
            source_id: archive.source_folder_id,
            relative_path: archive.relative_path.clone(),
            absolute_path_cached: archive.absolute_path.clone(),
            classification: MigrationClassification::RegenerateInstead,
            reason: "archives.relative_path is the stable source-relative observation key; absolute_path_cached must be recomputed after source state changes.".to_string(),
        })
        .collect();
    archive_observations.sort_by(|a, b| {
        a.source_id
            .cmp(&b.source_id)
            .then(a.archive_id.cmp(&b.archive_id))
    });

    let historical: Vec<_> = input
        .historical_paths
        .iter()
        .enumerate()
        .map(|(index, path)| MigrationProposal {
            reference_id: format!("scan-history:{index}"),
            subsystem: "source-folder-database-history".to_string(),
            old_path: path.clone(),
            candidate_path: None,
            classification: MigrationClassification::HistoricalOnly,
            reason: "Completed scan/history evidence is preserved and not rewritten.".to_string(),
        })
        .collect();

    root_plan.totals.conflicts += conflicts;
    let mut totals = SourceFolderMigrationTotals {
        source_folder_records: input.source_folders.len(),
        migratable_source_roots: owned_ids.len(),
        archive_observations_to_recompute: archive_observations.len(),
        historical_references: historical.len(),
        conflicts,
        ..Default::default()
    };
    for proposal in &root_plan.migration.proposals {
        match proposal.classification {
            MigrationClassification::SafeRebase => totals.source_roots_rebase += 1,
            MigrationClassification::AlreadyCurrent => totals.source_roots_already_current += 1,
            MigrationClassification::Ambiguous
            | MigrationClassification::ManualReview
            | MigrationClassification::OutsideNewRoot
            | MigrationClassification::TargetMissing => totals.source_roots_manual_review += 1,
            MigrationClassification::NotApplicable => totals.source_roots_unsupported += 1,
            _ => {}
        }
    }
    SourceFolderMigrationReport {
        source_roots: root_plan,
        archive_observations,
        historical,
        totals,
    }
}

#[cfg(test)]
mod tests;
