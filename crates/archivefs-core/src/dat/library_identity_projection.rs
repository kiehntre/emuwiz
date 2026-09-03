//! Turns one completed, single-source DAT audit outcome into the exact
//! records [`crate::database::Database::persist_library_dat_identity`] can
//! safely write - a pure projection of an audit the user already ran, never
//! a new scan, hash, or match.
//!
//! # Why this refuses a combined audit outright
//!
//! [`DatAuditOutcome::source_id`] is a real, durable DAT source identity
//! only for a single-source audit (`run_dat_audit`). A combined audit
//! (`run_combined_dat_audit`)'s flat `report.entries` verdict is merged
//! across every enabled source
//! (`merge_combined_evidence`/`combined_summary`) and the outcome's
//! `source_id` is the synthetic [`COMBINED_AUDIT_SOURCE_ID`] - per-entry
//! attribution to one real source does not exist in `report.entries` for
//! that shape (only `evidence_sources`, a list of *agreeing* observations,
//! carries it, and is a materially different shape from a
//! `library_dat_identities` row). Persisting from a combined outcome would
//! mean either attributing every match to a fake source or guessing which
//! of several real sources actually produced it - both refused here rather
//! than invented. `Database::persist_dat_audit_results` (the sibling
//! Arcade-set writer) already makes the identical choice: a combined audit
//! is never persisted by either, and `dat_sources_page.rs`'s combined-audit
//! completion handler never calls it.
//!
//! # Why only `report.entries`, never `archives[].members[]`
//!
//! An archive-member verdict identifies one `<rom>` *inside* an archive,
//! not the archive itself. [`crate::database::Database`]'s
//! `library_dat_identities` table is one row per library item
//! (`archive_id`) - and the already-existing, already-wired
//! `dat_set_audit_results`/`dat_set_audit_dependencies` tables are the
//! correct, more complete representation for a multi-member Arcade/MAME set
//! (set state, clone/parent, dependency resolution - none of which a raw
//! per-member verdict alone can express, and which
//! `Database::persist_dat_audit_results` already persists from `outcome.sets`
//! for exactly this reason). Building a second, less complete "identity" for
//! the same archive from member verdicts would let the two tables disagree
//! about the same archive. `report.entries` is always the correct,
//! packing-policy-agnostic per-physical-file comparison already used for
//! rename planning, and is what this module reads - the same signal
//! regardless of whether the audited catalogue happens to be a No-Intro/
//! Redump-style flat DAT or a MAME-style set DAT (in the latter case, an
//! outer archive's own whole-file hash usually will not match any single
//! `<rom>` entry, which is a true, if low-value, fact about that DAT
//! against that archive - not a claim about the archive's Arcade set
//! completeness, which remains exclusively `dat_set_audit_results`'s job).
//!
//! # Completeness and negative verdicts
//!
//! Every verdict present in `report.entries` was decided against whatever
//! catalogue substrate the run actually parsed - a DAT file either parses
//! or lands in [`DatAuditOutcome::unreadable_catalogues`], never partially.
//! So a *positive* (identity-carrying) verdict remains fully trustworthy
//! regardless of walk truncation or a sibling unreadable DAT file: the file
//! it names really did match something in the catalogue that did load. A
//! *negative* verdict (`NotInDat` / `NoUsableEvidence`) is different: if
//! any DAT file in this source failed to parse, the loaded index is
//! provably incomplete, and "not in DAT" could be a false negative caused
//! only by the unread portion. This module therefore withholds negative
//! verdicts - never emits a record for them at all, rather than emitting
//! and relying on the database's own partial-run guard - whenever the run
//! is not proven complete (`outcome.truncated` or a non-empty
//! `unreadable_catalogues`), while every positive verdict from the same run
//! is still projected normally. A cancelled run never reaches this module:
//! `run_dat_audit`/`run_combined_dat_audit` return `Err(Cancelled)` before
//! producing any `DatAuditOutcome` at all, so there is nothing to project.

use super::index::DatRomRef;
use super::library_identity_summary::{
    DatAuditCompleteness, LibraryDatIdentityQuery, LibraryItemHashes, PersistedLibraryDatIdentity,
    summarize_library_dat_identity,
};
use super::sources::audit_run::{COMBINED_AUDIT_SOURCE_ID, DatAuditOutcome};

/// One audited file's persistence-ready DAT identity, still keyed by the
/// audit's own path string. Archive-id association (which needs the
/// database) happens after this, never inside this pure projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedLibraryDatIdentity {
    pub local_path: String,
    pub identity: PersistedLibraryDatIdentity,
}

/// Why one audit entry was not projected. Every skip is accounted for, per
/// the existing bounded-diagnostics convention
/// (`unreadable_catalogues`/`unhashed`/...) - nothing is silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryDatIdentitySkipReason {
    /// The run's completeness makes a negative ("not in DAT") conclusion
    /// unsafe to persist for this entry - see this module's doc comment.
    /// A positive match from the same run is still persisted.
    NegativeConclusionUnsafe,
}

/// Everything one single-source audit is safe to persist, and everything it
/// explicitly is not, with why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryDatIdentityProjection {
    pub items: Vec<ProjectedLibraryDatIdentity>,
    pub skipped: Vec<(String, LibraryDatIdentitySkipReason)>,
    /// Whether this run's completeness allowed persisting negative
    /// verdicts. `Exhaustive` when the whole source parsed and the walk did
    /// not hit its ceiling; `Partial` otherwise (see this module's doc
    /// comment) - the same value stored on every emitted
    /// [`PersistedLibraryDatIdentity::completeness`].
    pub completeness: DatAuditCompleteness,
}

/// Refuses to project a shape this module cannot safely attribute to one
/// real DAT source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryDatIdentityProjectionRefusal {
    /// `outcome.source_id` is the synthetic combined-audit id - see this
    /// module's doc comment.
    CombinedAudit,
}

/// Projects one completed, single-source [`DatAuditOutcome`] into the exact
/// records a caller can hand to
/// [`crate::database::Database::persist_library_dat_identity`] for each
/// safely-identified library item.
///
/// Pure: no I/O, no database access, no hashing, no DAT re-matching -
/// everything read here is already-computed data on `outcome`.
/// `audited_at` is caller-supplied (rather than read from the clock here)
/// so this stays a pure function; production callers pass the real current
/// timestamp, tests pass a fixed one.
pub fn project_dat_audit_for_library_identity(
    outcome: &DatAuditOutcome,
    audited_at: &str,
) -> Result<LibraryDatIdentityProjection, LibraryDatIdentityProjectionRefusal> {
    if outcome.source_id == COMBINED_AUDIT_SOURCE_ID {
        return Err(LibraryDatIdentityProjectionRefusal::CombinedAudit);
    }

    let completeness = if outcome.truncated || !outcome.unreadable_catalogues.is_empty() {
        DatAuditCompleteness::Partial
    } else {
        DatAuditCompleteness::Exhaustive
    };

    let mut items = Vec::with_capacity(outcome.report.entries.len());
    let mut skipped = Vec::new();
    let empty_hashes = LibraryItemHashes::default();
    let no_matched_refs: &[DatRomRef] = &[];
    for entry in &outcome.report.entries {
        let audited_hashes = outcome
            .known_hashes
            .get(&entry.local_path)
            .map(to_library_item_hashes)
            .unwrap_or_else(|| empty_hashes.clone());

        let query = LibraryDatIdentityQuery {
            outcome,
            verdict: &entry.verdict,
            matched_refs: no_matched_refs,
            audited_hashes: &audited_hashes,
            // Freshness at persist time is irrelevant to what is stored -
            // only `PersistedLibraryDatIdentity::audited_hashes` (below,
            // from the query above) is kept; `current_hashes` only affects
            // the throwaway `LibraryDatIdentitySummary.provenance_freshness`
            // this function never reads.
            current_hashes: None,
        };
        let summary = summarize_library_dat_identity(&query);

        if completeness == DatAuditCompleteness::Partial && summary.is_no_match() {
            skipped.push((
                entry.local_path.clone(),
                LibraryDatIdentitySkipReason::NegativeConclusionUnsafe,
            ));
            continue;
        }

        let persisted = PersistedLibraryDatIdentity::from_summary(
            &summary,
            no_matched_refs,
            &audited_hashes,
            audited_at.to_string(),
            completeness,
        );
        items.push(ProjectedLibraryDatIdentity {
            local_path: entry.local_path.clone(),
            identity: persisted,
        });
    }

    Ok(LibraryDatIdentityProjection {
        items,
        skipped,
        completeness,
    })
}

fn to_library_item_hashes(
    hashes: &super::sources::audit_run::AuditedFileHashes,
) -> LibraryItemHashes {
    LibraryItemHashes {
        size_bytes: hashes.size_bytes,
        crc32: hashes.crc32.clone(),
        md5: hashes.md5.clone(),
        sha1: hashes.sha1.clone(),
        sha256: hashes.sha256.clone(),
    }
}

#[cfg(test)]
mod tests;
