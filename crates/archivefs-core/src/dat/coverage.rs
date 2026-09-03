//! Per-platform, per-DAT-source coverage aggregation - pure models plus
//! bounded [`crate::database::Database`] reads. No scan, no hash, no DAT
//! re-parse, no audit, no GUI.
//!
//! # Two things this proves, and the gate between them
//!
//! *Verification metrics* (owned / checked / verified-current /
//! verified-stale / probable / unmatched / ambiguous / unknown / provable
//! duplicates) come straight from `library_dat_identities` and are always
//! available for any `(platform, dat_source_id)` pair.
//!
//! *Expected / Missing / Completion / Full Set* additionally require a
//! durable named expected inventory (`dat_expected_entries` +
//! `dat_expected_inventory_meta`, migrations 0011/0012) **and** proof that
//! the DAT source is explicitly, exactly assigned to the requested
//! platform. [`CoverageSourceScope`] carries that proof from the config
//! layer; [`ExpectedInventoryStatus`] records the outcome of the gate.
//!
//! # The explicit-platform rule
//!
//! `DatSourceEntry.platform == None` means *unassigned*. An unassigned
//! source may still be broadly relevant to DAT selection and verification,
//! but it **must not** provide a platform coverage denominator - `None` is
//! never reinterpreted as "all platforms". Expected is available for
//! `(platform X, source Y)` only when `source_Y.platform == Some(X)`,
//! matched exactly. A source assigned to another platform provides Expected
//! for that platform only, never for X.
//!
//! When the gate does not pass, verification metrics still show, and
//! Expected/Missing/Completion read *unavailable* (not `0`), Full Set reads
//! [`CompleteSetVerdict::NotProvable`] with a specific reason.
//!
//! # Full Set proof
//!
//! [`CompleteSetVerdict::Complete`] is only produced when every one of
//! these holds:
//!
//! - the source is configured and explicitly assigned to the requested
//!   platform (the gate above);
//! - a durable expected inventory row set exists for it;
//! - the captured inventory's revision still matches the source's
//!   configured revision (currentness - no re-parse, reuses the existing
//!   revision string);
//! - `duplicate_names_skipped == 0` (the expected canonical identity set is
//!   proven one-to-one - see [`crate::dat::expected_inventory`]);
//! - `expected_count > 0`;
//! - every expected canonical identity has at least one verified-current
//!   representation (`missing_count == 0`).
//!
//! Duplicate library archives for one expected identity count *once* toward
//! coverage; the extras are reported separately and never block
//! completeness. **Count equality alone never proves complete**: 1000
//! expected against 1000 verified *archive rows* that only cover 950
//! distinct identities is `Incomplete { missing_count: 50 }`, not
//! `Complete`.
//!
//! # Arcade / MAME
//!
//! [`ArcadeDatSetCoverage`] keeps Arcade semantics separate: set
//! completeness always comes from the dependency-aware `SetState` verdicts
//! already persisted in `dat_set_audit_results` (never re-derived from flat
//! file identity). The expected-machine denominator obeys the same
//! explicit-platform rule. See [`ArcadeDatSetCoverage`] for exactly what it
//! can and cannot prove.

use serde::{Deserialize, Serialize};

use super::model::DatEcosystem;

/// What one coverage row counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatCoverageUnit {
    /// One DAT `<game>`/`<machine>` element, at whatever granularity the
    /// catalogue itself declares.
    CanonicalDatEntry,
    /// One Arcade/FinalBurn Neo multi-member set, as `dat_set_audit_results`
    /// tracks it - dependency-aware storage completeness, not one file.
    ArcadeSet,
}

/// What the config layer resolved about a DAT source, passed into a
/// coverage read so [`crate::database::Database`] never has to parse
/// `dat_sources.toml` itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct CoverageSourceScope<'a> {
    /// `DatSourceEntry.platform` - the source's *explicit* platform
    /// assignment. `None` is UNASSIGNED and never a coverage denominator.
    pub source_platform: Option<&'a str>,
    /// The source's currently-configured catalogue revision, for the
    /// currentness check against the captured inventory's revision.
    pub current_source_revision: Option<&'a str>,
    /// Whether the source is still a configured, enabled entry at all.
    pub configured: bool,
}

impl<'a> CoverageSourceScope<'a> {
    /// A scope for a source that is configured and assigned to `platform`.
    pub fn assigned_to(platform: &'a str, current_source_revision: Option<&'a str>) -> Self {
        Self {
            source_platform: Some(platform),
            current_source_revision,
            configured: true,
        }
    }
}

/// The outcome of the explicit-platform + inventory + currentness gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExpectedInventoryStatus {
    /// Gate passed: a current, durable expected inventory is available as a
    /// denominator. `duplicate_names_skipped > 0` still blocks a `Complete`
    /// full-set verdict (counts may be shown; identity set not proven 1:1).
    Available {
        entry_count: u64,
        duplicate_names_skipped: u64,
    },
    /// The source is not a configured/enabled entry.
    SourceUnconfigured,
    /// The source has no explicit platform assignment. Verification metrics
    /// still apply; no Expected denominator.
    PlatformUnassigned,
    /// The source is assigned to a different platform than the one asked
    /// about.
    PlatformMismatch { source_platform: String },
    /// Assigned and matching, but no successful validation has ever
    /// captured an expected inventory for it yet.
    InventoryMissing,
    /// An inventory exists but its captured generation cannot be proven to
    /// describe the source as currently configured (revision drift).
    InventoryStale { reason: String },
}

impl ExpectedInventoryStatus {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    /// The human-readable reason Expected/Missing/Full Set are unavailable,
    /// for the `NotProvable` verdict and the GUI. Empty string for
    /// [`Self::Available`] (which is not an unavailable state).
    pub fn unavailable_reason(&self) -> String {
        match self {
            Self::Available { .. } => String::new(),
            Self::SourceUnconfigured => "no configured DAT source with this id".to_string(),
            Self::PlatformUnassigned => {
                "this DAT source has no explicit platform assignment, so it cannot provide an \
                 expected-set denominator for any platform"
                    .to_string()
            }
            Self::PlatformMismatch { source_platform } => format!(
                "this DAT source is assigned to {source_platform}, not the requested platform"
            ),
            Self::InventoryMissing => {
                "this DAT source has never been validated successfully, so no expected \
                 inventory has been captured yet"
                    .to_string()
            }
            Self::InventoryStale { reason } => reason.clone(),
        }
    }
}

/// Whether a full-set claim can be made, and its result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum CompleteSetVerdict {
    /// Every expected canonical identity has a verified-current
    /// representation. `extra_duplicate_archives` is how many owned
    /// archives sit beyond one-per-identity - they coexist with
    /// completeness, they do not block it.
    Complete { extra_duplicate_archives: u64 },
    /// The proof requirements were all met except that some expected
    /// identities have no verified-current representation.
    Incomplete { missing_count: u64 },
    /// A full-set claim cannot be made from currently persisted data.
    NotProvable { reason: String },
}

impl CompleteSetVerdict {
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }
    pub fn is_provable(&self) -> bool {
        !matches!(self, Self::NotProvable { .. })
    }
}

/// The metadata row for one source's persisted expected inventory
/// (`dat_expected_inventory_meta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExpectedDatInventoryMeta {
    pub dat_source_id: String,
    pub source_revision: Option<String>,
    pub ecosystem: Option<DatEcosystem>,
    pub entry_count: u64,
    pub duplicate_names_skipped: u64,
    pub validated_at: String,
}

/// One `(platform, dat_source_id)` pair's full DAT coverage - verification
/// metrics always, Expected/Missing/Completion/Full-Set when the scope
/// gate passes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlatformDatCoverage {
    pub platform: String,
    pub dat_source_id: String,
    pub source_name: Option<String>,
    pub ecosystem: Option<DatEcosystem>,
    pub source_revision: Option<String>,
    pub unit: DatCoverageUnit,

    // ---- verification metrics: always populated ----
    /// Library items currently assigned to `platform`, regardless of check
    /// status. An independent library metric - never a completion
    /// denominator (owned can exceed expected: duplicates, hacks,
    /// unmatched, alternative dumps, unrelated local items).
    pub owned_applicable: usize,
    pub checked: usize,
    pub verified_current: usize,
    pub verified_stale: usize,
    pub probable: usize,
    pub unmatched: usize,
    pub ambiguous: usize,
    pub unknown: usize,
    /// Distinct canonical identities with more than one verified-current
    /// library item.
    pub duplicate_canonical_identities: usize,
    /// Archives beyond one-per-identity across those duplicates.
    pub duplicate_extra_archives: usize,

    // ---- expected-set metrics: gated ----
    pub expected_inventory: ExpectedInventoryStatus,
    /// The durable unique expected identity count. `None` unless
    /// `expected_inventory` is [`ExpectedInventoryStatus::Available`].
    pub expected_unique_count: Option<u64>,
    /// How many distinct expected identities have >=1 verified-current
    /// representation. `None` when the gate did not pass.
    pub represented_unique_count: Option<u64>,
    /// `expected_unique_count - represented_unique_count`. `None` when the
    /// gate did not pass.
    pub missing_count: Option<u64>,
    /// `represented_unique_count / expected_unique_count * 100`, based on
    /// **unique identities**, never on archive-row counts, so it can never
    /// exceed 100. `None` when the gate did not pass or the denominator
    /// is zero.
    pub completion_percent: Option<f64>,
    pub complete_set: CompleteSetVerdict,
}

/// One `(platform, dat_source_id)` Arcade/FinalBurn Neo source's set-level
/// coverage.
///
/// Set completeness is always the dependency-aware `SetState` from
/// `dat_set_audit_results` (which already folds in the dependency verdict
/// via `dat::dependency::apply_dependency_state`), never re-derived from
/// flat file identity. The expected-machine denominator obeys the
/// explicit-platform gate.
///
/// # Missing sets
///
/// A `dat_set_audit_results` row exists only for a machine some local
/// archive was found for. `missing_sets` is therefore expected machine
/// names with **no** `dat_set_audit_results` row at all;
/// `represented_complete_sets` is expected machine names with a `Complete`
/// row. A machine with rows that are all `Incomplete`/`NeedsReview` is
/// neither missing nor represented-complete - it shows in
/// `incomplete_sets`/`needs_review_sets`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArcadeDatSetCoverage {
    pub platform: String,
    pub dat_source_id: String,
    /// The `platform` column value observed on this source's
    /// `dat_set_audit_results` rows (provenance only - the gate uses
    /// `CoverageSourceScope`, not this).
    pub source_platform_of_row: Option<String>,
    pub ecosystem: Option<DatEcosystem>,
    pub source_revision: Option<String>,
    pub unit: DatCoverageUnit,

    pub checked_sets: usize,
    /// Distinct machine names with at least one dependency-aware `Complete`
    /// row.
    pub complete_sets: usize,
    pub incomplete_sets: usize,
    pub bad_metadata_sets: usize,
    pub needs_review_sets: usize,
    pub stale_sets: usize,

    pub expected_inventory: ExpectedInventoryStatus,
    pub expected_sets: Option<u64>,
    pub represented_complete_sets: Option<u64>,
    pub missing_sets: Option<u64>,
    pub completion_percent: Option<f64>,
    pub complete_set: CompleteSetVerdict,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_equality_alone_is_never_complete() {
        // 1000 expected, 1000 verified archive rows, but only 950 distinct
        // identities represented -> Incomplete, never Complete.
        let verdict = CompleteSetVerdict::Incomplete { missing_count: 50 };
        assert!(!verdict.is_complete());
        assert!(verdict.is_provable());
    }

    #[test]
    fn complete_coexists_with_extra_duplicate_archives() {
        let verdict = CompleteSetVerdict::Complete {
            extra_duplicate_archives: 12,
        };
        assert!(verdict.is_complete());
    }

    #[test]
    fn an_unassigned_source_reports_a_clear_reason_not_a_zero_denominator() {
        let status = ExpectedInventoryStatus::PlatformUnassigned;
        assert!(!status.is_available());
        assert!(status.unavailable_reason().contains("no explicit platform"));
    }

    #[test]
    fn a_platform_mismatch_names_the_actual_assigned_platform() {
        let status = ExpectedInventoryStatus::PlatformMismatch {
            source_platform: "SNES".to_string(),
        };
        assert!(status.unavailable_reason().contains("SNES"));
    }

    #[test]
    fn duplicate_names_block_complete_even_with_zero_missing() {
        // The DB layer constructs NotProvable directly when
        // duplicate_names_skipped > 0; this guards the type contract that
        // NotProvable is not complete and not "provably incomplete" either.
        let verdict = CompleteSetVerdict::NotProvable {
            reason: "duplicate <game> names".to_string(),
        };
        assert!(!verdict.is_complete());
        assert!(!verdict.is_provable());
    }
}
