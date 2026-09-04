//! Explaining *why* one RomM identity record is stale - never resolving it.
//!
//! [`ExternalVerification::Stale`] already says a record's match against a
//! local file failed after import, and the record's own `evidence` and
//! `conflicts` (set once, at match time, by
//! [`match_record`](crate::identity_source::matching::match_record)) already
//! say what was compared. Neither answers what a person actually wants to
//! know: is this the provider drifting, the local library drifting, a
//! mapping that has since changed, or something this build cannot safely act
//! on at all? A real cache showed roughly 16,373 stale records; a single
//! count that large is not actionable on its own.
//!
//! This module answers it using only evidence the system already has:
//!
//! - the record's own stored `evidence` and `conflicts` (never re-derived
//!   from a title or a filename);
//! - a fresh [`PathMappings::translate`] of the record's `provider_path`
//!   against the *current* mapping configuration - the exact call the real
//!   import path makes, so a changed or removed mapping is detected the same
//!   way it always is;
//! - a caller-supplied local presence probe, the same shape
//!   [`StaleSummary::build`](crate::identity_source::stale::StaleSummary::build)
//!   already uses, so this module never decides for itself how the
//!   filesystem is walked and stays testable without one;
//! - [`explain_duplicate_providers`], reused rather than reimplemented, so a
//!   record whose destination is contested is never treated as safe to
//!   republish just because its own mapping looks fine.
//!
//! # What this is not
//!
//! - **Not a second matching engine.** Nothing here recomputes a fingerprint,
//!   compares a hash, or re-derives [`ExternalVerification`]. A record's
//!   staleness verdict was already decided at match time; this only explains
//!   *why*.
//! - **Not automatic resolution.** No cache is rewritten, no config is
//!   changed, no local file is moved. [`RommStaleSafeAction`] describes what
//!   a *caller* could safely do next - it is never done here.
//! - **Not a second ambiguity engine.** A stale record whose destination is
//!   contested is reported as [`RommStaleReason::DuplicateProviderAmbiguity`]
//!   using [`explain_duplicate_providers`]'s own judgement, not a new one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::duplicate_provider_report::explain_duplicate_providers;
use crate::identity_source::matching::LocalPresence;
use crate::identity_source::model::{ConflictField, ExternalIdentityRecord, ExternalVerification};
use crate::identity_source::path_map::PathMappings;

/// Evidence text pushed at import time when RomM's own catalogue flags a file
/// missing from its own filesystem. Matched the same way
/// [`StaleSummary`](crate::identity_source::stale::StaleSummary) already
/// matches it - a substring check against stored text, never a new field.
const PROVIDER_MISSING_MARKER: &str = "missing from its own filesystem";

/// Why one record is stale, ranked by how directly the evidence explains it.
/// Computed purely from stored evidence, a fresh mapping translation and a
/// local presence probe - never from title text or a filename heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RommStaleReason {
    /// This destination is claimed by more than one RomM record right now
    /// (see [`explain_duplicate_providers`]). Checked first, and overrides
    /// every other reason below: an ambiguous destination is never safe to
    /// republish just because one claimant's own mapping looks fine.
    DuplicateProviderAmbiguity,
    /// The record's `provider_path` no longer translates under the *current*
    /// mapping configuration at all - a mapping was removed, or narrowed,
    /// since this record was imported.
    MappingNoLongerTranslates,
    /// The current mapping translates `provider_path` to a local path that
    /// differs from the cached `archivefs_path`, and that new path does not
    /// exist yet either. The mapping changed, but there is still nothing to
    /// republish.
    TranslatedPathChanged,
    /// The current mapping translates `provider_path` to a local path that
    /// differs from the cached `archivefs_path`, and that new path exists
    /// right now. The cache is simply behind the mapping configuration.
    CacheRepublishNeeded,
    /// The record's stored `conflicts` include a file-size disagreement: the
    /// local file exists, but is not the size RomM recorded.
    FileSizeMismatch,
    /// RomM's own catalogue reports this file missing from its filesystem,
    /// and the mapped local file is missing too.
    ProviderAndLocalMissing,
    /// RomM's own catalogue reports this file missing from its filesystem,
    /// but the mapped local file is present right now.
    ProviderMissing,
    /// The mapped local file is missing or otherwise not a usable file
    /// (a directory, a dangling symlink, an absent parent). RomM does not
    /// report this file as missing on its side.
    LocalMissing,
    /// None of the stored evidence, the current mapping or the local probe
    /// explain the stale verdict. Reported honestly rather than guessed at -
    /// this should be rare, and only occurs if a future match outcome adds a
    /// staleness cause this module does not yet know about.
    UnknownDrift,
}

/// What a caller could safely do about one stale record. Never executed by
/// this module - a read-only conclusion for a person or a later refresh
/// pipeline to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RommStaleSafeAction {
    /// Ordinary provider or library drift. Nothing to do.
    NoAction,
    /// A provider-side refresh could change RomM's own missing-from-fs
    /// verdict, since the local file is actually present.
    RefreshMayResolve,
    /// The current mapping already proves a new, unambiguous, existing local
    /// path. The cache can be safely republished with it.
    RepublishSafe,
    /// The local file's presence changed since this record was last matched
    /// (it is present now, having previously been recorded as missing), or
    /// its size disagreement needs a fresh local comparison. Re-running
    /// local matching - not a provider refresh - is what would resolve this.
    LocalRescanRequired,
    /// The evidence is ambiguous, or points at a mapping-configuration
    /// question only a person can settle.
    UserReview,
    /// This record cannot be safely classified from current evidence.
    Unsupported,
}

/// Secondary, individually-inspectable flags behind one [`RommStaleReason`].
/// The reason names *why* a record was marked stale; these flags say exactly
/// what was observed to reach it, so a caller never has to re-derive them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RommStaleFlags {
    /// RomM's own catalogue reports this file missing from its filesystem.
    pub provider_reports_missing: bool,
    /// The record's stored `conflicts` include a file-size disagreement.
    pub file_size_mismatch: bool,
    /// The current mapping no longer translates `provider_path` at all.
    pub mapping_no_longer_translates: bool,
    /// The current mapping translates `provider_path` to a path other than
    /// the cached `archivefs_path`.
    pub translated_path_changed: bool,
    /// This destination is claimed by more than one RomM record right now.
    pub duplicate_provider_ambiguous: bool,
    /// What the local presence probe found at the path this record's
    /// staleness is actually judged against (the current translation when
    /// the mapping still applies, otherwise the cached path).
    pub local_presence: LocalPresence,
}

/// One stale record, explained. Never mutated, never acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommStaleExplanation {
    pub provider_game_id: String,
    pub provider_path: String,
    /// The local path this record pointed to at import/match time, if any.
    pub cached_archivefs_path: Option<PathBuf>,
    /// What the current mapping configuration translates `provider_path` to
    /// right now, if anything - independent of what is cached.
    pub current_translated_path: Option<PathBuf>,
    pub reason: RommStaleReason,
    pub safe_action: RommStaleSafeAction,
    pub flags: RommStaleFlags,
}

impl RommStaleReason {
    /// A person-readable explanation, safe to show as-is: never suggests
    /// deleting a local file, and never suggests a mapping-configuration
    /// change unless the reason itself is about the mapping.
    pub fn message(self) -> &'static str {
        match self {
            Self::DuplicateProviderAmbiguity => {
                "More than one RomM record claims this local destination right now, so \
                 republishing any one of them would silently choose a winner. See the \
                 duplicate-provider report before doing anything with this record."
            }
            Self::MappingNoLongerTranslates => {
                "RomM's path for this record no longer matches any configured mapping. It may \
                 have been imported under a mapping that has since been removed or narrowed."
            }
            Self::TranslatedPathChanged => {
                "The current mapping now resolves this RomM path to a different local file than \
                 the one cached, but that file is not present yet either."
            }
            Self::CacheRepublishNeeded => {
                "The current mapping already resolves this RomM path to a local file that \
                 exists. This cache record can be safely republished on the next refresh."
            }
            Self::FileSizeMismatch => {
                "The local file exists, but its size no longer matches what RomM recorded."
            }
            Self::ProviderAndLocalMissing => {
                "RomM no longer reports this file, and the mapped local file is missing too. \
                 This looks like ordinary library drift."
            }
            Self::ProviderMissing => {
                "RomM reports this file as missing from its own filesystem, but the mapped \
                 local file is present. A refresh from RomM may resolve this."
            }
            Self::LocalMissing => {
                "The mapped local file is no longer present, though RomM does not report it \
                 missing on its side."
            }
            Self::UnknownDrift => {
                "This record is marked stale, but current evidence does not explain why. \
                 Treat it as ordinary drift unless it recurs."
            }
        }
    }
}

/// Deterministic totals over every stale record explained. Every count below
/// is a partition of `stale`: each record contributes to exactly one reason
/// and exactly one safe-action count, so both sets of counts sum to `stale`
/// with no double-counting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RommStaleSummary {
    pub total_in_cache: usize,
    pub stale: usize,
    pub explanations: Vec<RommStaleExplanation>,

    pub duplicate_provider_ambiguous: usize,
    pub mapping_no_longer_translates: usize,
    pub translated_path_changed: usize,
    pub cache_republish_needed: usize,
    pub file_size_mismatch: usize,
    pub provider_and_local_missing: usize,
    pub provider_missing: usize,
    pub local_missing: usize,
    pub unknown_drift: usize,

    pub no_action: usize,
    pub refresh_may_resolve: usize,
    pub republish_safe: usize,
    pub local_rescan_required: usize,
    pub user_review: usize,
    pub unsupported: usize,
}

impl RommStaleSummary {
    pub fn count_by_reason(&self, reason: RommStaleReason) -> usize {
        match reason {
            RommStaleReason::DuplicateProviderAmbiguity => self.duplicate_provider_ambiguous,
            RommStaleReason::MappingNoLongerTranslates => self.mapping_no_longer_translates,
            RommStaleReason::TranslatedPathChanged => self.translated_path_changed,
            RommStaleReason::CacheRepublishNeeded => self.cache_republish_needed,
            RommStaleReason::FileSizeMismatch => self.file_size_mismatch,
            RommStaleReason::ProviderAndLocalMissing => self.provider_and_local_missing,
            RommStaleReason::ProviderMissing => self.provider_missing,
            RommStaleReason::LocalMissing => self.local_missing,
            RommStaleReason::UnknownDrift => self.unknown_drift,
        }
    }

    pub fn count_by_safe_action(&self, action: RommStaleSafeAction) -> usize {
        match action {
            RommStaleSafeAction::NoAction => self.no_action,
            RommStaleSafeAction::RefreshMayResolve => self.refresh_may_resolve,
            RommStaleSafeAction::RepublishSafe => self.republish_safe,
            RommStaleSafeAction::LocalRescanRequired => self.local_rescan_required,
            RommStaleSafeAction::UserReview => self.user_review,
            RommStaleSafeAction::Unsupported => self.unsupported,
        }
    }
}

/// Explains every [`ExternalVerification::Stale`] record in `records`
/// against `mappings` - the exact [`PathMappings`] currently configured.
///
/// Read-only end to end: `records` and `mappings` are only ever read, no
/// record is edited, no cache or config file is written, and the only I/O is
/// whatever `presence_for` performs (a `stat`, supplied by the caller, so a
/// test can run with none at all). Never hashes a file and never contacts
/// RomM.
pub fn explain_stale_records(
    records: &[ExternalIdentityRecord],
    mappings: &PathMappings,
    presence_for: impl Fn(&Path) -> LocalPresence,
) -> RommStaleSummary {
    let contested: BTreeSet<PathBuf> = explain_duplicate_providers(records, mappings)
        .groups
        .into_iter()
        .map(|group| group.destination)
        .collect();

    let mut stale: Vec<&ExternalIdentityRecord> = records
        .iter()
        .filter(|record| record.verification == ExternalVerification::Stale)
        .collect();
    // Sorted so two calls over the same records - regardless of the cache's
    // own iteration order - produce the same explanation order every time.
    stale.sort_by(|left, right| {
        left.provider_game_id
            .cmp(&right.provider_game_id)
            .then_with(|| left.provider_path.cmp(&right.provider_path))
    });

    let mut summary = RommStaleSummary {
        total_in_cache: records.len(),
        stale: stale.len(),
        ..Default::default()
    };

    for record in stale {
        let explanation = explain_one(record, mappings, &presence_for, &contested);
        tally(&mut summary, &explanation);
        summary.explanations.push(explanation);
    }

    summary
}

fn tally(summary: &mut RommStaleSummary, explanation: &RommStaleExplanation) {
    match explanation.reason {
        RommStaleReason::DuplicateProviderAmbiguity => summary.duplicate_provider_ambiguous += 1,
        RommStaleReason::MappingNoLongerTranslates => summary.mapping_no_longer_translates += 1,
        RommStaleReason::TranslatedPathChanged => summary.translated_path_changed += 1,
        RommStaleReason::CacheRepublishNeeded => summary.cache_republish_needed += 1,
        RommStaleReason::FileSizeMismatch => summary.file_size_mismatch += 1,
        RommStaleReason::ProviderAndLocalMissing => summary.provider_and_local_missing += 1,
        RommStaleReason::ProviderMissing => summary.provider_missing += 1,
        RommStaleReason::LocalMissing => summary.local_missing += 1,
        RommStaleReason::UnknownDrift => summary.unknown_drift += 1,
    }
    match explanation.safe_action {
        RommStaleSafeAction::NoAction => summary.no_action += 1,
        RommStaleSafeAction::RefreshMayResolve => summary.refresh_may_resolve += 1,
        RommStaleSafeAction::RepublishSafe => summary.republish_safe += 1,
        RommStaleSafeAction::LocalRescanRequired => summary.local_rescan_required += 1,
        RommStaleSafeAction::UserReview => summary.user_review += 1,
        RommStaleSafeAction::Unsupported => summary.unsupported += 1,
    }
}

fn explain_one(
    record: &ExternalIdentityRecord,
    mappings: &PathMappings,
    presence_for: &impl Fn(&Path) -> LocalPresence,
    contested: &BTreeSet<PathBuf>,
) -> RommStaleExplanation {
    let cached_path = record.archivefs_path.clone();
    let translation = mappings.translate(&record.provider_path);
    let current_translated_path = translation.archivefs_path().map(Path::to_path_buf);

    let duplicate_provider_ambiguous = cached_path
        .as_deref()
        .is_some_and(|path| contested.contains(path));

    // Ambiguity is checked before anything else: a contested destination is
    // never safe to republish just because one claimant's own mapping looks
    // fine, no matter what else is true about this specific record.
    if duplicate_provider_ambiguous {
        let mapping_no_longer_translates = current_translated_path.is_none();
        let translated_path_changed = current_translated_path != cached_path;
        return RommStaleExplanation {
            provider_game_id: record.provider_game_id.clone(),
            provider_path: record.provider_path.clone(),
            cached_archivefs_path: cached_path.clone(),
            current_translated_path,
            reason: RommStaleReason::DuplicateProviderAmbiguity,
            safe_action: RommStaleSafeAction::UserReview,
            flags: RommStaleFlags {
                provider_reports_missing: provider_reports_missing(record),
                file_size_mismatch: has_file_size_conflict(record),
                mapping_no_longer_translates,
                translated_path_changed,
                duplicate_provider_ambiguous: true,
                local_presence: cached_path.as_deref().map(presence_for).unwrap_or_default(),
            },
        };
    }

    let provider_reports_missing = provider_reports_missing(record);
    let file_size_mismatch = has_file_size_conflict(record);

    // Mapping drift: does the current configuration even agree with what is
    // cached? Checked before provider/local evidence, since a record whose
    // mapping has changed is not explained by what the *old* mapping's
    // target looked like.
    if current_translated_path.is_none() {
        let local_presence = cached_path.as_deref().map(presence_for).unwrap_or_default();
        return RommStaleExplanation {
            provider_game_id: record.provider_game_id.clone(),
            provider_path: record.provider_path.clone(),
            cached_archivefs_path: cached_path,
            current_translated_path: None,
            reason: RommStaleReason::MappingNoLongerTranslates,
            safe_action: RommStaleSafeAction::UserReview,
            flags: RommStaleFlags {
                provider_reports_missing,
                file_size_mismatch,
                mapping_no_longer_translates: true,
                translated_path_changed: true,
                duplicate_provider_ambiguous: false,
                local_presence,
            },
        };
    }

    if current_translated_path != cached_path {
        let new_path = current_translated_path.clone().expect("checked Some above");
        let new_path_presence = presence_for(&new_path);
        let (reason, safe_action) = if new_path_presence == LocalPresence::File {
            (
                RommStaleReason::CacheRepublishNeeded,
                RommStaleSafeAction::RepublishSafe,
            )
        } else {
            (
                RommStaleReason::TranslatedPathChanged,
                RommStaleSafeAction::UserReview,
            )
        };
        return RommStaleExplanation {
            provider_game_id: record.provider_game_id.clone(),
            provider_path: record.provider_path.clone(),
            cached_archivefs_path: cached_path,
            current_translated_path: Some(new_path),
            reason,
            safe_action,
            flags: RommStaleFlags {
                provider_reports_missing,
                file_size_mismatch,
                mapping_no_longer_translates: false,
                translated_path_changed: true,
                duplicate_provider_ambiguous: false,
                local_presence: new_path_presence,
            },
        };
    }

    // Mapping is unchanged: the cached path is still exactly what the
    // current configuration would produce. Whatever made this record stale
    // is provider or local evidence, not a mapping question.
    let Some(path) = cached_path.clone() else {
        // Defensive only: `match_record` never reaches `Stale` without a
        // resolved `archivefs_path`, so this cannot occur in practice.
        return RommStaleExplanation {
            provider_game_id: record.provider_game_id.clone(),
            provider_path: record.provider_path.clone(),
            cached_archivefs_path: None,
            current_translated_path,
            reason: RommStaleReason::UnknownDrift,
            safe_action: RommStaleSafeAction::Unsupported,
            flags: RommStaleFlags {
                provider_reports_missing,
                file_size_mismatch,
                mapping_no_longer_translates: false,
                translated_path_changed: false,
                duplicate_provider_ambiguous: false,
                local_presence: LocalPresence::default(),
            },
        };
    };

    let local_presence = presence_for(&path);
    let local_missing_now = local_presence != LocalPresence::File;

    let (reason, safe_action) = if file_size_mismatch {
        (
            RommStaleReason::FileSizeMismatch,
            RommStaleSafeAction::LocalRescanRequired,
        )
    } else if provider_reports_missing && local_missing_now {
        (
            RommStaleReason::ProviderAndLocalMissing,
            RommStaleSafeAction::NoAction,
        )
    } else if provider_reports_missing {
        (
            RommStaleReason::ProviderMissing,
            RommStaleSafeAction::RefreshMayResolve,
        )
    } else if local_missing_now {
        (RommStaleReason::LocalMissing, RommStaleSafeAction::NoAction)
    } else {
        // Neither side currently reports a problem: the cached `Stale`
        // verdict was reached at match time (most likely the local file was
        // absent then) and the local file is present again now. Re-running
        // local matching, not a provider refresh, is what would resolve it.
        (
            RommStaleReason::LocalMissing,
            RommStaleSafeAction::LocalRescanRequired,
        )
    };

    RommStaleExplanation {
        provider_game_id: record.provider_game_id.clone(),
        provider_path: record.provider_path.clone(),
        cached_archivefs_path: Some(path),
        current_translated_path,
        reason,
        safe_action,
        flags: RommStaleFlags {
            provider_reports_missing,
            file_size_mismatch,
            mapping_no_longer_translates: false,
            translated_path_changed: false,
            duplicate_provider_ambiguous: false,
            local_presence,
        },
    }
}

fn provider_reports_missing(record: &ExternalIdentityRecord) -> bool {
    record
        .evidence
        .iter()
        .any(|line| line.contains(PROVIDER_MISSING_MARKER))
}

fn has_file_size_conflict(record: &ExternalIdentityRecord) -> bool {
    record
        .conflicts
        .iter()
        .any(|conflict| conflict.field == ConflictField::FileSize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::model::{ExternalHash, IdentityConflict, IdentityProvider};
    use crate::identity_source::path_map::{PathMapping, ProviderPathKind};

    #[allow(clippy::too_many_arguments)]
    fn record(
        game_id: &str,
        provider_path: &str,
        archivefs_path: Option<&str>,
        verification: ExternalVerification,
        evidence: Vec<&str>,
        conflicts: Vec<IdentityConflict>,
    ) -> ExternalIdentityRecord {
        ExternalIdentityRecord {
            provider: IdentityProvider::Romm,
            server_id: "romm-test".to_string(),
            provider_platform_id: Some("1".to_string()),
            provider_game_id: game_id.to_string(),
            provider_file_id: None,
            provider_path: provider_path.to_string(),
            archivefs_path: archivefs_path.map(PathBuf::from),
            title: None,
            platform_candidate: Some("nes".to_string()),
            provider_platform_name: Some("nes".to_string()),
            regions: Vec::new(),
            revision: None,
            hashes: Vec::<ExternalHash>::new(),
            file_size_bytes: Some(1_024),
            metadata_provider_ids: Vec::new(),
            artwork: None,
            related_files: Vec::new(),
            sibling_game_ids: Vec::new(),
            imported_at_unix_seconds: 1_785_000_000,
            provider_updated_at: None,
            verification,
            conflicts,
            evidence: evidence.into_iter().map(str::to_string).collect(),
            synopsis: None,
            genres: Vec::new(),
            players: None,
            rating: None,
            release_year: None,
        }
    }

    fn mapping(prefix: &str, destination: &str, aliases: Vec<&str>) -> PathMapping {
        PathMapping {
            provider_prefix: prefix.to_string(),
            archivefs_prefix: PathBuf::from(destination),
            provider_aliases: aliases.into_iter().map(str::to_string).collect(),
        }
    }

    fn mappings(entries: Vec<PathMapping>) -> PathMappings {
        PathMappings::validate(&entries, &[], ProviderPathKind::ProviderRelative)
            .expect("test mappings must validate")
    }

    fn no_files(_path: &Path) -> LocalPresence {
        LocalPresence::Absent
    }

    fn only_present(present: PathBuf) -> impl Fn(&Path) -> LocalPresence {
        move |path| {
            if path == present {
                LocalPresence::File
            } else {
                LocalPresence::Absent
            }
        }
    }

    fn size_conflict() -> IdentityConflict {
        IdentityConflict {
            field: ConflictField::FileSize,
            external: "1024".to_string(),
            local: "2048".to_string(),
            detail: "size changed".to_string(),
        }
    }

    #[test]
    fn provider_missing_only_is_refresh_may_resolve_when_local_exists() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let path = PathBuf::from("/mnt/games/nes/Game.zip");
        let records = vec![record(
            "1",
            "roms/nes/Game.zip",
            Some("/mnt/games/nes/Game.zip"),
            ExternalVerification::Stale,
            vec!["RomM reports this file as missing from its own filesystem"],
            Vec::new(),
        )];

        let summary = explain_stale_records(&records, &mappings, only_present(path));

        assert_eq!(summary.stale, 1);
        assert_eq!(
            summary.explanations[0].reason,
            RommStaleReason::ProviderMissing
        );
        assert_eq!(
            summary.explanations[0].safe_action,
            RommStaleSafeAction::RefreshMayResolve
        );
        assert_eq!(summary.provider_missing, 1);
        assert_eq!(summary.refresh_may_resolve, 1);
    }

    #[test]
    fn local_missing_only_is_no_action() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let records = vec![record(
            "1",
            "roms/nes/Game.zip",
            Some("/mnt/games/nes/Game.zip"),
            ExternalVerification::Stale,
            vec!["nothing exists at this path"],
            Vec::new(),
        )];

        let summary = explain_stale_records(&records, &mappings, no_files);

        assert_eq!(
            summary.explanations[0].reason,
            RommStaleReason::LocalMissing
        );
        assert_eq!(
            summary.explanations[0].safe_action,
            RommStaleSafeAction::NoAction
        );
        assert_eq!(summary.local_missing, 1);
        assert_eq!(summary.no_action, 1);
    }

    #[test]
    fn both_provider_and_local_missing() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let records = vec![record(
            "1",
            "roms/nes/Game.zip",
            Some("/mnt/games/nes/Game.zip"),
            ExternalVerification::Stale,
            vec!["RomM reports this file as missing from its own filesystem"],
            Vec::new(),
        )];

        let summary = explain_stale_records(&records, &mappings, no_files);

        assert_eq!(
            summary.explanations[0].reason,
            RommStaleReason::ProviderAndLocalMissing
        );
        assert_eq!(
            summary.explanations[0].safe_action,
            RommStaleSafeAction::NoAction
        );
        assert_eq!(summary.provider_and_local_missing, 1);
    }

    #[test]
    fn size_mismatch_requires_local_rescan() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let path = PathBuf::from("/mnt/games/nes/Game.zip");
        let records = vec![record(
            "1",
            "roms/nes/Game.zip",
            Some("/mnt/games/nes/Game.zip"),
            ExternalVerification::Stale,
            vec!["the file's size no longer matches what RomM recorded"],
            vec![size_conflict()],
        )];

        let summary = explain_stale_records(&records, &mappings, only_present(path));

        assert_eq!(
            summary.explanations[0].reason,
            RommStaleReason::FileSizeMismatch
        );
        assert_eq!(
            summary.explanations[0].safe_action,
            RommStaleSafeAction::LocalRescanRequired
        );
        assert_eq!(summary.file_size_mismatch, 1);
        assert_eq!(summary.local_rescan_required, 1);
    }

    #[test]
    fn mapping_no_longer_translates_is_user_review() {
        // No mappings configured at all, so `provider_path` cannot translate.
        let mappings = mappings(vec![mapping("roms/other", "/mnt/games/other", vec![])]);
        let records = vec![record(
            "1",
            "roms/nes/Game.zip",
            Some("/mnt/games/nes/Game.zip"),
            ExternalVerification::Stale,
            vec!["nothing exists at this path"],
            Vec::new(),
        )];

        let summary = explain_stale_records(&records, &mappings, no_files);

        assert_eq!(
            summary.explanations[0].reason,
            RommStaleReason::MappingNoLongerTranslates
        );
        assert_eq!(
            summary.explanations[0].safe_action,
            RommStaleSafeAction::UserReview
        );
        assert!(summary.explanations[0].current_translated_path.is_none());
        assert_eq!(summary.mapping_no_longer_translates, 1);
    }

    #[test]
    fn translated_path_changed_but_not_present_is_user_review() {
        // Mapping destination changed since import; the new target is absent.
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes-moved", vec![])]);
        let records = vec![record(
            "1",
            "roms/nes/Game.zip",
            Some("/mnt/games/nes/Game.zip"),
            ExternalVerification::Stale,
            vec!["nothing exists at this path"],
            Vec::new(),
        )];

        let summary = explain_stale_records(&records, &mappings, no_files);

        assert_eq!(
            summary.explanations[0].reason,
            RommStaleReason::TranslatedPathChanged
        );
        assert_eq!(
            summary.explanations[0].safe_action,
            RommStaleSafeAction::UserReview
        );
        assert_eq!(
            summary.explanations[0].current_translated_path,
            Some(PathBuf::from("/mnt/games/nes-moved/Game.zip"))
        );
        assert_eq!(summary.translated_path_changed, 1);
    }

    #[test]
    fn stale_but_null_cached_path_and_current_mapping_proves_valid_path_is_republish_safe() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes-new", vec![])]);
        let new_path = PathBuf::from("/mnt/games/nes-new/Game.zip");
        let records = vec![record(
            "1",
            "roms/nes/Game.zip",
            Some("/mnt/games/nes/Game.zip"),
            ExternalVerification::Stale,
            vec!["nothing exists at this path"],
            Vec::new(),
        )];

        let summary = explain_stale_records(&records, &mappings, only_present(new_path.clone()));

        assert_eq!(
            summary.explanations[0].reason,
            RommStaleReason::CacheRepublishNeeded
        );
        assert_eq!(
            summary.explanations[0].safe_action,
            RommStaleSafeAction::RepublishSafe
        );
        assert_eq!(
            summary.explanations[0].current_translated_path,
            Some(new_path)
        );
        assert_eq!(summary.cache_republish_needed, 1);
        assert_eq!(summary.republish_safe, 1);
    }

    #[test]
    fn duplicate_provider_ambiguity_blocks_republish() {
        // Two mappings EmuWiz has not declared equivalent, colliding on one
        // destination - a `MappingCollision`-shaped fixture, reused here to
        // prove that ambiguity is checked before mapping drift or provider
        // evidence and always wins.
        let mappings = mappings(vec![
            mapping("roms/nes", "/mnt/games/shared", vec![]),
            mapping("roms/famicom", "/mnt/games/shared/nested", vec![]),
        ]);
        let destination = PathBuf::from("/mnt/games/shared/nested/Game.zip");
        let records = vec![
            record(
                "1",
                "roms/nes/nested/Game.zip",
                Some(destination.to_str().unwrap()),
                ExternalVerification::Stale,
                vec!["nothing exists at this path"],
                Vec::new(),
            ),
            record(
                "2",
                "roms/famicom/Game.zip",
                Some(destination.to_str().unwrap()),
                ExternalVerification::Ambiguous,
                Vec::new(),
                Vec::new(),
            ),
        ];

        let summary = explain_stale_records(&records, &mappings, no_files);

        assert_eq!(summary.stale, 1);
        assert_eq!(
            summary.explanations[0].reason,
            RommStaleReason::DuplicateProviderAmbiguity
        );
        assert_eq!(
            summary.explanations[0].safe_action,
            RommStaleSafeAction::UserReview
        );
        assert!(summary.explanations[0].flags.duplicate_provider_ambiguous);
        assert_eq!(summary.duplicate_provider_ambiguous, 1);
        assert_eq!(summary.republish_safe, 0);
    }

    #[test]
    fn non_stale_records_are_excluded() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let records = vec![
            record(
                "1",
                "roms/nes/Confirmed.zip",
                Some("/mnt/games/nes/Confirmed.zip"),
                ExternalVerification::ConfirmedExternal,
                Vec::new(),
                Vec::new(),
            ),
            record(
                "2",
                "roms/nes/Probable.zip",
                Some("/mnt/games/nes/Probable.zip"),
                ExternalVerification::ProbableExternal,
                Vec::new(),
                Vec::new(),
            ),
        ];

        let summary = explain_stale_records(&records, &mappings, no_files);

        assert_eq!(summary.stale, 0);
        assert_eq!(summary.total_in_cache, 2);
        assert!(summary.explanations.is_empty());
    }

    #[test]
    fn output_is_deterministic_across_repeated_calls() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let records = vec![
            record(
                "2",
                "roms/nes/B.zip",
                Some("/mnt/games/nes/B.zip"),
                ExternalVerification::Stale,
                vec!["nothing exists at this path"],
                Vec::new(),
            ),
            record(
                "1",
                "roms/nes/A.zip",
                Some("/mnt/games/nes/A.zip"),
                ExternalVerification::Stale,
                vec!["nothing exists at this path"],
                Vec::new(),
            ),
        ];

        let first = explain_stale_records(&records, &mappings, no_files);
        let second = explain_stale_records(&records, &mappings, no_files);

        assert_eq!(first, second);
        assert_eq!(first.explanations[0].provider_game_id, "1");
        assert_eq!(first.explanations[1].provider_game_id, "2");
    }

    #[test]
    fn totals_never_double_count_a_record() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let records = vec![
            record(
                "1",
                "roms/nes/A.zip",
                Some("/mnt/games/nes/A.zip"),
                ExternalVerification::Stale,
                vec!["RomM reports this file as missing from its own filesystem"],
                Vec::new(),
            ),
            record(
                "2",
                "roms/nes/B.zip",
                Some("/mnt/games/nes/B.zip"),
                ExternalVerification::Stale,
                vec!["nothing exists at this path"],
                Vec::new(),
            ),
        ];

        let summary = explain_stale_records(&records, &mappings, no_files);

        let reason_total = summary.duplicate_provider_ambiguous
            + summary.mapping_no_longer_translates
            + summary.translated_path_changed
            + summary.cache_republish_needed
            + summary.file_size_mismatch
            + summary.provider_and_local_missing
            + summary.provider_missing
            + summary.local_missing
            + summary.unknown_drift;
        let action_total = summary.no_action
            + summary.refresh_may_resolve
            + summary.republish_safe
            + summary.local_rescan_required
            + summary.user_review
            + summary.unsupported;

        assert_eq!(reason_total, summary.stale);
        assert_eq!(action_total, summary.stale);
    }

    #[test]
    fn nothing_here_ever_changes_the_records_or_mappings_it_was_given() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let before = mappings.clone();
        let records = vec![record(
            "1",
            "roms/nes/A.zip",
            Some("/mnt/games/nes/A.zip"),
            ExternalVerification::Stale,
            vec!["nothing exists at this path"],
            Vec::new(),
        )];
        let records_before = records.clone();

        let _ = explain_stale_records(&records, &mappings, no_files);

        assert_eq!(records, records_before);
        assert_eq!(mappings, before);
    }

    #[test]
    fn calling_this_never_touches_the_real_destination_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("Game.zip");
        std::fs::write(&file_path, b"contents").expect("write fixture file");
        let before_bytes = std::fs::read(&file_path).expect("read fixture file");
        let before_mtime = std::fs::metadata(&file_path)
            .expect("stat fixture file")
            .modified()
            .expect("mtime");

        let mappings = mappings(vec![mapping(
            "roms/nes",
            dir.path().to_str().unwrap(),
            vec![],
        )]);
        let records = vec![record(
            "1",
            "roms/nes/Game.zip",
            Some(file_path.to_str().unwrap()),
            ExternalVerification::Stale,
            vec!["nothing exists at this path"],
            Vec::new(),
        )];

        let presence_for = |path: &Path| -> LocalPresence {
            if path.is_file() {
                LocalPresence::File
            } else {
                LocalPresence::Absent
            }
        };
        let _ = explain_stale_records(&records, &mappings, presence_for);

        let after_bytes = std::fs::read(&file_path).expect("read fixture file after");
        let after_mtime = std::fs::metadata(&file_path)
            .expect("stat fixture file after")
            .modified()
            .expect("mtime after");
        assert_eq!(before_bytes, after_bytes);
        assert_eq!(before_mtime, after_mtime);
    }

    /// Signature-pinning anchor: if this stops compiling, the public API
    /// shape changed. `records`/`mappings`/`presence_for` are the only
    /// inputs - no `IdentityCache`, no filesystem walk, no network client.
    #[test]
    fn explain_stale_records_takes_only_local_in_memory_types() {
        let records: Vec<ExternalIdentityRecord> = Vec::new();
        let mappings = mappings(Vec::new());
        let summary = explain_stale_records(&records, &mappings, no_files);
        assert_eq!(summary.stale, 0);
    }
}
