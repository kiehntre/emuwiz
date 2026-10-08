//! Restart discovery of PS2 restore journals. Read-only, bounded, and honest
//! about what it could not see: an unreadable directory, an enumeration error
//! or more records than the bound is reported as a problem, never as an empty
//! or complete inventory.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::ops::Deref;
use std::os::unix::ffi::OsStrExt;

use super::{
    JOURNAL_PREFIX, Ps2PsuRestoreError, Ps2RecoveryOutcome, Ps2RestoreJournalSummary,
    load_ps2_restore_journal, recover_ps2_psu_restore, recovery_evidence,
};
use std::fs;
use std::path::{Path, PathBuf};

/// Most journals one discovery lists. Records beyond it are counted, not read.
pub const PS2_DISCOVERY_LIMIT: usize = 1024;

/// Why a discovery result must not be read as the complete recovery inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ps2DiscoveryProblem {
    /// The journal directory exists (or could not be shown not to) but cannot
    /// be listed. Records may exist that this result does not show.
    ListingFailed { kind: ErrorKind, detail: String },
    /// Enumeration stopped on an error; entries after it were not seen.
    EnumerationInterrupted { detail: String },
    /// More journals matched than [`PS2_DISCOVERY_LIMIT`]; the newest names
    /// (the greatest in journal-name order, which starts with a fixed-width
    /// hexadecimal timestamp) are listed, `omitted` older records are not.
    Truncated { limit: usize, omitted: usize },
}

impl std::fmt::Display for Ps2DiscoveryProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ListingFailed { detail, .. } => {
                write!(f, "the restore record folder cannot be listed ({detail})")
            }
            Self::EnumerationInterrupted { detail } => {
                write!(
                    f,
                    "listing the restore record folder stopped early ({detail})"
                )
            }
            Self::Truncated { limit, omitted } => write!(
                f,
                "only the newest {limit} restore records are shown; {omitted} older records are not shown"
            ),
        }
    }
}

/// Journals found plus everything that makes the list uncertain. Dereferences
/// to the record slice so a caller can read the records, but the problems
/// travel with them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ps2RestoreDiscovery {
    pub records: Vec<Ps2RestoreJournalSummary>,
    pub problems: Vec<Ps2DiscoveryProblem>,
}

impl Ps2RestoreDiscovery {
    /// True only when the listing was fully read: an empty complete result
    /// means there genuinely are no restore records.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.problems.is_empty()
    }
}

impl Deref for Ps2RestoreDiscovery {
    type Target = [Ps2RestoreJournalSummary];
    fn deref(&self) -> &Self::Target {
        &self.records
    }
}

fn is_journal_name(name: &std::ffi::OsStr) -> bool {
    let bytes = name.as_bytes();
    bytes.starts_with(JOURNAL_PREFIX.as_bytes()) && bytes.ends_with(b".json")
}

/// The newest `PS2_DISCOVERY_LIMIT` journal paths (greatest in name order),
/// returned oldest-first, in bounded memory. Only names are compared; no journal
/// body is read to choose them. Anything that makes the answer uncertain is
/// pushed to `problems`.
fn list_candidates(dir: &Path, problems: &mut Vec<Ps2DiscoveryProblem>) -> Vec<PathBuf> {
    match fs::read_dir(dir) {
        Ok(entries) => collect_newest(
            dir,
            entries.map(|entry| entry.map(|entry| entry.path())),
            problems,
        ),
        Err(error) => {
            // Only a path that is truly absent is "no records". A dangling
            // symlink, ELOOP, EACCES or anything else is uncertainty.
            let absent = error.kind() == ErrorKind::NotFound
                && fs::symlink_metadata(dir).is_err_and(|e| e.kind() == ErrorKind::NotFound);
            if !absent {
                problems.push(Ps2DiscoveryProblem::ListingFailed {
                    kind: error.kind(),
                    detail: format!("{}: {error}", dir.display()),
                });
            }
            Vec::new()
        }
    }
}

fn collect_newest(
    dir: &Path,
    entries: impl Iterator<Item = std::io::Result<PathBuf>>,
    problems: &mut Vec<Ps2DiscoveryProblem>,
) -> Vec<PathBuf> {
    let mut newest = BTreeSet::new();
    let mut matched = 0usize;
    for entry in entries {
        match entry {
            Ok(path) => {
                if path.file_name().is_some_and(is_journal_name) {
                    matched += 1;
                    newest.insert(path);
                    // Evict the oldest name so memory stays at limit + 1 paths.
                    if newest.len() > PS2_DISCOVERY_LIMIT {
                        newest.pop_first();
                    }
                }
            }
            Err(error) => {
                problems.push(Ps2DiscoveryProblem::EnumerationInterrupted {
                    detail: format!("{}: {error}", dir.display()),
                });
                break;
            }
        }
    }
    if matched > PS2_DISCOVERY_LIMIT {
        problems.push(Ps2DiscoveryProblem::Truncated {
            limit: PS2_DISCOVERY_LIMIT,
            omitted: matched - PS2_DISCOVERY_LIMIT,
        });
    }
    newest.into_iter().collect()
}

fn summarize(path: PathBuf) -> Ps2RestoreJournalSummary {
    match load_ps2_restore_journal(&path) {
        Ok(journal) => {
            let evidence_problem = recovery_evidence::problem(&journal);
            Ps2RestoreJournalSummary {
                operation_id: Some(journal.operation_id.clone()),
                phase: Some(journal.phase),
                card_path: Some(journal.card_path.clone()),
                error: None,
                needs_recovery: journal.phase.needs_recovery(),
                needs_attention: journal.phase.needs_attention() || evidence_problem.is_some(),
                undo_available: journal.phase == super::Ps2RestorePhase::Published,
                detail: evidence_problem.or_else(|| journal.detail.clone()),
                path,
            }
        }
        Err(error) => Ps2RestoreJournalSummary {
            path,
            operation_id: None,
            phase: None,
            card_path: None,
            error: Some(error.to_string()),
            needs_recovery: true,
            needs_attention: true,
            undo_available: false,
            detail: None,
        },
    }
}

/// Restart discovery: every restore journal in `journal_dir` (the newest
/// [`PS2_DISCOVERY_LIMIT`] if there are more, oldest-first), including corrupt
/// ones. Read-only.
/// A directory that does not exist is a complete, empty result; any other
/// failure to list it is a [`Ps2DiscoveryProblem`].
#[must_use]
pub fn discover_ps2_restore_journals(journal_dir: &Path) -> Ps2RestoreDiscovery {
    let mut problems = Vec::new();
    let records = list_candidates(journal_dir, &mut problems)
        .into_iter()
        .map(summarize)
        .collect();
    Ps2RestoreDiscovery { records, problems }
}

/// Outcome of judging every interrupted operation found.
#[derive(Debug)]
pub struct Ps2RecoveryRun {
    pub outcomes: Vec<(PathBuf, Result<Ps2RecoveryOutcome, Ps2PsuRestoreError>)>,
    /// Discovery problems: records that were not judged because they were not seen.
    pub problems: Vec<Ps2DiscoveryProblem>,
}

/// Judge every interrupted operation in `journal_dir` (see
/// [`recover_ps2_psu_restore`]); never writes a card. Only records that
/// discovery actually listed are judged; `problems` says if others may exist.
pub fn recover_all_interrupted_ps2_restores(
    journal_dir: &Path,
    unix_seconds: u64,
) -> Ps2RecoveryRun {
    let discovery = discover_ps2_restore_journals(journal_dir);
    let outcomes = discovery
        .records
        .into_iter()
        .filter(|summary| summary.needs_recovery)
        .map(|summary| {
            let outcome = recover_ps2_psu_restore(&summary.path, unix_seconds);
            (summary.path, outcome)
        })
        .collect();
    Ps2RecoveryRun {
        outcomes,
        problems: discovery.problems,
    }
}

#[cfg(test)]
mod tests;
