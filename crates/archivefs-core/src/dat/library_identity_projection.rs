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
//! # Archive members: projected to the parent only when unambiguous
//!
//! `report.entries` is the per-physical-file comparison, so a ZIP/7z row is
//! normally judged by its *outer* hash and lands as `NotInDat` even when its
//! one ROM member matches exactly. `project_archive_members_onto_parents`
//! therefore lifts a member result onto the outer archive's row, failing
//! closed: the pass must be complete and stable, every member must be exact
//! for one and the same DAT game, empty, or a known-ancillary file, and no
//! partial catalogue set may be involved. Collections, weak/ambiguous
//! members, unknown extra ROMs, nested archives and unreadable archives stay
//! unverified. The row's `audited_hashes` are the outer archive's (so the
//! usual freshness check invalidates it if the container changes) and
//! [`PersistedLibraryDatIdentity::archive_member`] records the member.
//! Arcade/MAME set verdicts remain `dat_set_audit_results`' job.
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

use super::archive::{ArchiveMemberStatus, ArchivePassCompletion};
use super::audit::AuditVerdict;
use super::index::DatRomRef;
use super::library_identity_summary::{
    ArchiveMemberProvenance, DatAuditCompleteness, DatVerificationState, LibraryDatIdentityQuery,
    LibraryItemHashes, PersistedLibraryDatIdentity, summarize_library_dat_identity,
};
use super::set::SetState;
use super::sources::audit_run::{
    COMBINED_AUDIT_SOURCE_ID, DatArchiveAudit, DatArchiveMemberAudit, DatAuditOutcome,
    safe_archive_member_name,
};

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

    project_archive_members_onto_parents(
        outcome,
        audited_at,
        completeness,
        &mut items,
        &mut skipped,
    );

    Ok(LibraryDatIdentityProjection {
        items,
        skipped,
        completeness,
    })
}

/// Member extensions that may sit next to a ROM without being a second game.
/// Deliberately tiny: an unknown member that is merely "not in the DAT" could
/// be another ROM, so anything outside this list blocks parent projection.
const ANCILLARY_EXTENSIONS: &[&str] = &[
    "txt", "nfo", "diz", "md", "url", "jpg", "jpeg", "png", "gif", "pdf", "xml", "htm", "html",
];

fn is_ancillary_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        ANCILLARY_EXTENSIONS
            .iter()
            .any(|a| ext.eq_ignore_ascii_case(a))
    })
}

/// The one member whose exact identity may stand for the whole ZIP/7z, or
/// `None` (fail closed) unless *every* member is accounted for: a complete,
/// stable pass; no nested archive or unsafe name; each member either exact
/// for one and the same DAT game, empty, or a known-ancillary file the DAT
/// does not claim. Anything weak, ambiguous, unreadable or unknown blocks it.
fn parent_representative_member<'a>(
    outcome: &DatAuditOutcome,
    archive: &'a DatArchiveAudit,
) -> Option<&'a DatArchiveMemberAudit> {
    if !matches!(archive.completion, ArchivePassCompletion::Complete)
        || archive.outer_identity.is_none()
        || !matches!(archive.format.as_str(), "zip" | "7z")
    {
        return None;
    }
    let mut representative: Option<(&DatArchiveMemberAudit, &str)> = None;
    for member in &archive.members {
        let name = &member.evidence.member_name_display;
        if member.evidence.is_nested_archive || !safe_archive_member_name(name) {
            return None;
        }
        match (&member.evidence.status, &member.verdict) {
            (ArchiveMemberStatus::EmptyFile, _) => {}
            (ArchiveMemberStatus::HashComplete, Some(AuditVerdict::Exact { game_name, .. })) => {
                match representative {
                    Some((_, known)) if known != game_name => return None,
                    Some(_) => {}
                    None => representative = Some((member, game_name)),
                }
            }
            (
                ArchiveMemberStatus::HashComplete,
                Some(AuditVerdict::NotInDat | AuditVerdict::NoUsableEvidence),
            ) if is_ancillary_name(name) => {}
            _ => return None,
        }
    }
    let (member, game) = representative?;
    // A game this archive only partly holds is not a verified game.
    let partial_set = outcome.sets.iter().any(|set| {
        set.archive_path == archive.archive_path
            && set.identity.game_name == game
            && set.state != SetState::Complete
    });
    (!partial_set).then_some(member)
}

/// Projects a safe exact member result onto the outer archive's own row.
/// `audited_hashes` is the *outer* archive's, so the existing freshness
/// comparison (archive hash vs library hash) invalidates it if the container
/// changes; the member's own hash lives in `hash_evidence`.
fn project_archive_members_onto_parents(
    outcome: &DatAuditOutcome,
    audited_at: &str,
    completeness: DatAuditCompleteness,
    items: &mut Vec<ProjectedLibraryDatIdentity>,
    skipped: &mut Vec<(String, LibraryDatIdentitySkipReason)>,
) {
    for archive in &outcome.archives {
        let Some(member) = parent_representative_member(outcome, archive) else {
            continue;
        };
        let (Some(verdict), Some(hashes)) = (&member.verdict, &member.evidence.hashes) else {
            continue;
        };
        let local_path = archive.archive_path.to_string_lossy().into_owned();
        let Some(outer) = outcome
            .known_hashes
            .get(&local_path)
            .map(to_library_item_hashes)
        else {
            continue;
        };
        if outer.sha256.is_none()
            && outer.sha1.is_none()
            && outer.md5.is_none()
            && outer.crc32.is_none()
        {
            continue; // no baseline to ever prove freshness against
        }
        // A positive whole-file verdict for the outer archive already stands.
        let existing = items.iter().position(|item| item.local_path == local_path);
        if existing.is_some_and(|at| items[at].identity.carries_identity()) {
            continue;
        }
        let member_hashes = LibraryItemHashes {
            size_bytes: Some(member.evidence.logical_size),
            crc32: Some(hashes.crc32.clone()),
            md5: Some(hashes.md5.clone()),
            sha1: Some(hashes.sha1.clone()),
            sha256: Some(hashes.sha256.clone()),
        };
        let summary = summarize_library_dat_identity(&LibraryDatIdentityQuery {
            outcome,
            verdict,
            matched_refs: &member.matched_refs,
            audited_hashes: &member_hashes,
            current_hashes: None,
        });
        let DatVerificationState::VerifiedSingleMatch { algorithm } = &summary.verification_state
        else {
            continue;
        };
        let mut identity = PersistedLibraryDatIdentity::from_summary(
            &summary,
            &member.matched_refs,
            &member_hashes,
            audited_at.to_string(),
            completeness,
        );
        identity.archive_member = Some(ArchiveMemberProvenance {
            member_name: member.evidence.member_name_display.clone(),
            member_index: member.evidence.index,
            archive_format: archive.format.clone(),
            algorithm: algorithm.clone(),
        });
        identity.audited_hashes = outer;
        let projected = ProjectedLibraryDatIdentity {
            local_path,
            identity,
        };
        match existing {
            Some(at) => items[at] = projected,
            None => items.push(projected),
        }
        skipped.retain(|(path, _)| path != &archive.archive_path.to_string_lossy());
    }
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
mod archive_parent_e2e;
#[cfg(test)]
mod tests;
