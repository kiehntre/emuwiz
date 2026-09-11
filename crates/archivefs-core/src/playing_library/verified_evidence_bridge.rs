//! Bridges already-verified, persisted DAT identity
//! ([`crate::database::Database::library_dat_identity_for_item`]) into the
//! Playing Library planner's existing [`super::DatArchiveMatch`] model - the
//! reusable primitive [`match_loose_files_against_dat`](super::match_loose_files_against_dat)'s
//! GUI caller should prefer before ever falling back to loose-file
//! rematching. See "PLAYING LIBRARY VERIFIED-EVIDENCE BRIDGE" for the full
//! design.
//!
//! # Why this is not "another rematcher"
//!
//! This module never hashes a file to *decide* which catalogue entry it
//! matches - that decision was already made, once, by whatever DAT audit
//! produced the persisted [`PersistedLibraryDatIdentity`] row. All this
//! module does is:
//!
//! 1. Look up whether a persisted, hash-verified identity exists for a
//!    candidate path at all ([`Database::find_archive_id_by_absolute_path`],
//!    [`Database::library_dat_identity_for_item`]).
//! 2. Prove that identity is still fresh by recomputing the file's SHA-1
//!    right now and requiring an *exact* match against the SHA-1 the audit
//!    itself compared ([`PersistedLibraryDatIdentity::audited_hashes`]) -
//!    the same cryptographic evidence every hash-based match in this crate
//!    already rests on, not a new or weaker freshness notion.
//! 3. Re-resolve that same, now-reconfirmed SHA-1 against the *currently
//!    loaded* [`ParsedDat`] via [`DatIndex::lookup_sha1`] - exactly the
//!    lookup [`super::matching::match_loose_files_against_dat`]'s own pass 1
//!    performs - so a persisted match can never silently outlive a changed
//!    or reloaded catalogue.
//! 4. Hand the resulting `file_game` map to
//!    [`super::matching::combine_verified_matches`], the *same* CUE/GDI/M3U
//!    structural-combination code the fallback matcher uses, so multi-file
//!    releases keep exactly the same grouping semantics either way.
//!
//! Every candidate that fails any of the above (no persisted row, wrong DAT
//! source, ambiguous evidence, hash drift, or a `facts_json` a current
//! binary cannot deserialize) is returned in
//! [`VerifiedEvidenceBridgeOutcome::needs_fallback`] rather than silently
//! skipped or guessed - the caller is expected to run exactly those
//! remaining paths through the existing
//! [`match_loose_files_against_dat`](super::match_loose_files_against_dat).
//!
//! # What "verified" means here
//!
//! Only [`DatVerificationState::VerifiedSingleMatch`] is eligible - a
//! cryptographic hash that matched exactly one catalogue entry.
//! `Probable` (CRC32-only), `AmbiguousMultipleCandidates`, `Conflicting`,
//! `FilenameOnlyNotVerified`, `NoMatch`, and `NoUsableEvidence` are all
//! treated as "no usable persisted evidence" and deferred to fallback -
//! this bridge never promotes a weaker verdict into a trusted
//! [`DatArchiveMatch`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::dat::index::DatIndex;
use crate::dat::library_identity_summary::{DatVerificationState, PersistedLibraryDatIdentity};
use crate::dat::model::ParsedDat;
use crate::database::Database;
use crate::identity_source::hashing::hash_file;
use crate::safe_read::TrustedRoots;

use super::matching::combine_verified_matches;
use super::{DatArchiveMatch, RejectedLauncher};

/// Why one candidate path could not be bridged from persisted evidence -
/// always recorded, never silently dropped, so a caller (or a test) can
/// distinguish "nothing was ever audited here" from "evidence exists but is
/// no longer trustworthy."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeSkipReason {
    /// No library item is known at this exact path (never scanned, or the
    /// scan is for a different source root).
    NoArchiveRecord,
    /// An archive record exists, but no DAT identity was ever persisted for
    /// it against this exact `dat_source_id`.
    NoPersistedIdentity,
    /// A persisted row exists but could not be read back (schema drift a
    /// current binary cannot deserialize, or a database error) - fails
    /// closed rather than guessing at a partial record.
    PersistedRecordUnreadable,
    /// The persisted verdict is not a single cryptographic-hash match
    /// (`Probable`, ambiguous, conflicting, filename-only, or no match) -
    /// not strong enough evidence to skip fallback verification.
    NotStronglyVerified,
    /// The persisted row itself already records more than one competing
    /// candidate name - never silently resolved to one.
    AmbiguousCandidates,
    /// The audited hash is missing, or the file's current content hash does
    /// not match what was audited - the source changed, or a different
    /// object now lives at this path.
    HashDrifted,
    /// The (now-confirmed-fresh) hash no longer resolves to exactly one
    /// entry in the *currently loaded* catalogue - the DAT itself changed
    /// since the audit, or was reloaded with different content.
    NoLongerUniqueInCatalogue,
    /// The file could not be read/hashed right now at all (permissions,
    /// removed, symlink policy, ...).
    Unreadable,
}

/// The result of trying to bridge every `candidates` path from persisted
/// evidence before any fallback matching runs.
#[derive(Debug, Clone, Default)]
pub struct VerifiedEvidenceBridgeOutcome {
    /// Matches reconstructed entirely from freshness-valid persisted
    /// evidence - safe to feed straight into
    /// [`super::build_playing_library_plan`] alongside (or instead of) any
    /// fallback-matched result.
    pub matches: Vec<DatArchiveMatch>,
    /// Rejected multi-file releases discovered while combining persisted
    /// evidence (e.g. a CUE whose bridged tracks verify against more than
    /// one catalogue entry) - same shape and meaning as
    /// [`super::matching::MatchOutcome::rejected_launchers`].
    pub rejected_launchers: Vec<RejectedLauncher>,
    /// Every candidate path this bridge did not resolve, with why - the
    /// caller must run exactly these through the existing fallback matcher,
    /// never assume they are unmatched.
    pub needs_fallback: Vec<(PathBuf, BridgeSkipReason)>,
}

/// Attempts to reconstruct verified [`DatArchiveMatch`] values for
/// `candidates` from persisted evidence before any fallback rematching.
///
/// `database` is read-only here: no row is written, no schema migrated, no
/// file touched. `dat` must be the *currently loaded* catalogue for
/// `dat_source_id` - persisted evidence is only trusted after being
/// re-resolved against it (see the module doc comment).
pub fn bridge_verified_evidence(
    database: &Database,
    dat_source_id: &str,
    dat: &ParsedDat,
    candidates: &[PathBuf],
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
) -> VerifiedEvidenceBridgeOutcome {
    let index = DatIndex::build(dat);
    let mut file_game: BTreeMap<PathBuf, usize> = BTreeMap::new();
    let mut needs_fallback: Vec<(PathBuf, BridgeSkipReason)> = Vec::new();

    for candidate in candidates {
        // A `.cue`/`.gdi`/`.m3u` launcher is never individually hashed or
        // persisted by any real DAT provider (see the module doc comment
        // and `matching`'s own doc comment) - it is only ever resolved
        // structurally, below, via `combine_verified_matches`. Treating one
        // as "needs fallback" here would be both misleading (there was
        // never persisted evidence to look up) and, worse, would cause the
        // caller's fallback pass to re-attempt combining it without the
        // companions this bridge already consumed, producing a spurious
        // rejection. Simply skip it in this per-file loop.
        if is_launcher_extension(candidate) {
            continue;
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            // Cancellation mid-bridge: every remaining candidate must still
            // be accounted for so the caller's fallback pass sees it -
            // never silently dropped.
            needs_fallback.push((candidate.clone(), BridgeSkipReason::Unreadable));
            continue;
        }
        match bridge_one_candidate(database, dat_source_id, &index, candidate, trusted, cancel) {
            Ok(game_index) => {
                file_game.insert(candidate.clone(), game_index);
            }
            Err(reason) => needs_fallback.push((candidate.clone(), reason)),
        }
    }

    let combined = combine_verified_matches(dat, candidates, file_game, trusted, cancel);

    VerifiedEvidenceBridgeOutcome {
        matches: combined.matches,
        rejected_launchers: combined.rejected_launchers,
        needs_fallback,
    }
}

/// Matches the exact extension set `super::matching` treats as a structural
/// launcher - never individually verified, only ever resolved via its
/// referenced files.
fn is_launcher_extension(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "cue" | "gdi" | "m3u" | "m3u8"
    )
}

fn bridge_one_candidate(
    database: &Database,
    dat_source_id: &str,
    index: &DatIndex,
    candidate: &Path,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
) -> Result<usize, BridgeSkipReason> {
    let archive_id = database
        .find_archive_id_by_absolute_path(candidate)
        .map_err(|_| BridgeSkipReason::PersistedRecordUnreadable)?
        .ok_or(BridgeSkipReason::NoArchiveRecord)?;

    let persisted: PersistedLibraryDatIdentity = database
        .library_dat_identity_for_item(archive_id, dat_source_id)
        .map_err(|_| BridgeSkipReason::PersistedRecordUnreadable)?
        .ok_or(BridgeSkipReason::NoPersistedIdentity)?;

    if !matches!(
        persisted.verification_state,
        DatVerificationState::VerifiedSingleMatch { .. }
    ) {
        return Err(BridgeSkipReason::NotStronglyVerified);
    }
    if !persisted.ambiguous_candidates.is_empty() {
        return Err(BridgeSkipReason::AmbiguousCandidates);
    }
    let Some(audited_sha1) = persisted.audited_hashes.sha1.as_deref() else {
        return Err(BridgeSkipReason::HashDrifted);
    };

    let current =
        hash_file(candidate, trusted, Some(cancel)).map_err(|_| BridgeSkipReason::Unreadable)?;
    if !audited_sha1
        .trim()
        .eq_ignore_ascii_case(current.sha1.trim())
    {
        return Err(BridgeSkipReason::HashDrifted);
    }

    let refs = index.lookup_sha1(&current.sha1);
    let distinct_games: std::collections::BTreeSet<usize> =
        refs.iter().map(|entry| entry.game_index).collect();
    match distinct_games.len() {
        1 => Ok(*distinct_games.iter().next().expect("len() == 1")),
        _ => Err(BridgeSkipReason::NoLongerUniqueInCatalogue),
    }
}

#[cfg(test)]
mod tests;
