//! Shared, typed freshness evidence for imported DAT/catalogue sources.
//!
//! `Active` is a storage/lifecycle fact. This module models the separate
//! question of whether active bytes were compared with a trusted release.
//! Import time, activation, and filenames cannot create `Current`.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::dat::updates::{ManagedDatProvider, ManagedDatState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatFreshnessState {
    Current,
    UpdateAvailable,
    Unknown,
    CheckFailed,
    NeverChecked,
}

/// Closed set of providers allowed to contribute DAT freshness evidence.
/// Descriptive metadata providers such as ScreenScraper are intentionally
/// absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatFreshnessProvider {
    NoIntro,
    Tosec,
    Mame,
    Redump,
    Fbneo,
    LocalComparison,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatFreshnessComparisonSource {
    TrustedUpstream,
    TrustedLocalSnapshot,
    TrustedLocalStagedImport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatFreshnessComparisonBasis {
    ReleaseIdentifier,
    ContentHash,
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatFreshnessFailureReason {
    Network,
    InvalidResponse,
    UntrustedCandidate,
    ComparisonUnavailable,
    Storage,
    Provider,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatFreshnessEvidence {
    pub provider: DatFreshnessProvider,
    pub active_snapshot_version: Option<String>,
    pub compared_candidate_version: Option<String>,
    pub comparison_source: Option<DatFreshnessComparisonSource>,
    pub last_check_at_unix_seconds: Option<u64>,
    pub failure_reason: Option<DatFreshnessFailureReason>,
    pub comparison_basis: Option<DatFreshnessComparisonBasis>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatFreshnessAssessment {
    pub state: DatFreshnessState,
    pub evidence: DatFreshnessEvidence,
}

fn assessment(state: DatFreshnessState, evidence: DatFreshnessEvidence) -> DatFreshnessAssessment {
    DatFreshnessAssessment { state, evidence }
}

pub fn active_without_comparison(
    provider: DatFreshnessProvider,
    active_snapshot_version: Option<&str>,
) -> DatFreshnessAssessment {
    assessment(
        DatFreshnessState::Unknown,
        DatFreshnessEvidence {
            provider,
            active_snapshot_version: active_snapshot_version.map(str::to_owned),
            compared_candidate_version: None,
            comparison_source: None,
            last_check_at_unix_seconds: None,
            failure_reason: None,
            comparison_basis: None,
        },
    )
}

/// TOSEC release directories currently provide local inventory/provenance,
/// not an authoritative remote-current comparison. Keep this adapter narrow
/// so activation/import cannot accidentally become `Current`.
pub fn tosec_active_import_freshness(
    active_snapshot_version: Option<&str>,
) -> DatFreshnessAssessment {
    active_without_comparison(DatFreshnessProvider::Tosec, active_snapshot_version)
}

pub fn never_checked(provider: DatFreshnessProvider) -> DatFreshnessAssessment {
    assessment(
        DatFreshnessState::NeverChecked,
        DatFreshnessEvidence {
            provider,
            active_snapshot_version: None,
            compared_candidate_version: None,
            comparison_source: None,
            last_check_at_unix_seconds: None,
            failure_reason: None,
            comparison_basis: None,
        },
    )
}

pub fn failed_check(
    provider: DatFreshnessProvider,
    active_snapshot_version: Option<&str>,
    source: DatFreshnessComparisonSource,
    checked_at_unix_seconds: u64,
    reason: DatFreshnessFailureReason,
) -> DatFreshnessAssessment {
    assessment(
        DatFreshnessState::CheckFailed,
        DatFreshnessEvidence {
            provider,
            active_snapshot_version: active_snapshot_version.map(str::to_owned),
            compared_candidate_version: None,
            comparison_source: Some(source),
            last_check_at_unix_seconds: Some(checked_at_unix_seconds),
            failure_reason: Some(reason),
            comparison_basis: None,
        },
    )
}

/// Applies a comparison whose ordering was established by the provider's
/// documented release/hash rule. An older candidate cannot prove currentness.
pub fn compare_explicit(
    provider: DatFreshnessProvider,
    active_snapshot_version: Option<&str>,
    candidate_version: Option<&str>,
    ordering: Ordering,
    source: DatFreshnessComparisonSource,
    basis: DatFreshnessComparisonBasis,
    checked_at_unix_seconds: u64,
) -> DatFreshnessAssessment {
    let state = match ordering {
        Ordering::Equal => DatFreshnessState::Current,
        Ordering::Less => DatFreshnessState::UpdateAvailable,
        Ordering::Greater => DatFreshnessState::Unknown,
    };
    assessment(
        state,
        DatFreshnessEvidence {
            provider,
            active_snapshot_version: active_snapshot_version.map(str::to_owned),
            compared_candidate_version: candidate_version.map(str::to_owned),
            comparison_source: Some(source),
            last_check_at_unix_seconds: Some(checked_at_unix_seconds),
            failure_reason: None,
            comparison_basis: Some(basis),
        },
    )
}

/// Conservative adapter over persisted managed state. Active bytes and old
/// check metadata do not prove currentness after activation or rollback.
pub fn managed_dat_freshness(state: Option<&ManagedDatState>) -> DatFreshnessAssessment {
    let Some(state) = state else {
        return never_checked(DatFreshnessProvider::LocalComparison);
    };
    let provider = match state.source_id.provider {
        ManagedDatProvider::MameSoftwareList => DatFreshnessProvider::Mame,
        ManagedDatProvider::RedumpBios | ManagedDatProvider::RedumpGames => {
            DatFreshnessProvider::Redump
        }
        ManagedDatProvider::Fbneo => DatFreshnessProvider::Fbneo,
    };
    if state.last_failure.is_some() {
        return failed_check(
            provider,
            state.upstream_revision.as_deref(),
            DatFreshnessComparisonSource::TrustedUpstream,
            state.last_checked_at_unix_seconds.unwrap_or_default(),
            DatFreshnessFailureReason::Provider,
        );
    }
    active_without_comparison(provider, state.upstream_revision.as_deref())
}

/// Adapter used only when the caller has just completed a successful,
/// provider-specific freshness comparison. Persisted state alone must not
/// call this function, because it cannot distinguish an active update from a
/// later rollback.
pub fn managed_dat_freshness_after_successful_check(
    state: Option<&ManagedDatState>,
) -> DatFreshnessAssessment {
    let Some(state) = state else {
        return never_checked(DatFreshnessProvider::LocalComparison);
    };
    let provider = match state.source_id.provider {
        ManagedDatProvider::MameSoftwareList => DatFreshnessProvider::Mame,
        ManagedDatProvider::RedumpBios | ManagedDatProvider::RedumpGames => {
            DatFreshnessProvider::Redump
        }
        ManagedDatProvider::Fbneo => DatFreshnessProvider::Fbneo,
    };
    if state.last_failure.is_some() {
        return failed_check(
            provider,
            state.upstream_revision.as_deref(),
            DatFreshnessComparisonSource::TrustedUpstream,
            state.last_checked_at_unix_seconds.unwrap_or_default(),
            DatFreshnessFailureReason::Provider,
        );
    }
    if state.last_checked_at_unix_seconds.is_none() || state.upstream_revision.is_none() {
        return active_without_comparison(provider, state.upstream_revision.as_deref());
    }
    compare_explicit(
        provider,
        state.upstream_revision.as_deref(),
        state.upstream_revision.as_deref(),
        Ordering::Equal,
        DatFreshnessComparisonSource::TrustedUpstream,
        DatFreshnessComparisonBasis::ReleaseIdentifier,
        state.last_checked_at_unix_seconds.unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::updates::{ManagedDatSnapshot, ManagedDatSourceDescriptor, ManagedDatState};

    #[test]
    fn active_snapshot_alone_is_unknown() {
        assert_eq!(
            active_without_comparison(DatFreshnessProvider::NoIntro, Some("20250101")).state,
            DatFreshnessState::Unknown
        );
    }

    #[test]
    fn never_checked_is_distinct_from_active_unknown() {
        assert_eq!(
            never_checked(DatFreshnessProvider::Tosec).state,
            DatFreshnessState::NeverChecked
        );
    }

    #[test]
    fn explicit_equal_comparison_is_current() {
        assert_eq!(
            compare_explicit(
                DatFreshnessProvider::NoIntro,
                Some("20250101"),
                Some("20250101"),
                Ordering::Equal,
                DatFreshnessComparisonSource::TrustedLocalSnapshot,
                DatFreshnessComparisonBasis::Version,
                42,
            )
            .state,
            DatFreshnessState::Current
        );
    }

    #[test]
    fn newer_candidate_is_update_available() {
        assert_eq!(
            compare_explicit(
                DatFreshnessProvider::Tosec,
                Some("2024"),
                Some("2025"),
                Ordering::Less,
                DatFreshnessComparisonSource::TrustedLocalStagedImport,
                DatFreshnessComparisonBasis::Version,
                42,
            )
            .state,
            DatFreshnessState::UpdateAvailable
        );
    }

    #[test]
    fn failed_check_does_not_remove_active_evidence() {
        let result = failed_check(
            DatFreshnessProvider::Mame,
            Some("abc"),
            DatFreshnessComparisonSource::TrustedUpstream,
            42,
            DatFreshnessFailureReason::Network,
        );
        assert_eq!(result.state, DatFreshnessState::CheckFailed);
        assert_eq!(result.evidence.active_snapshot_version.as_deref(), Some("abc"));
    }

    #[test]
    fn older_candidate_does_not_fabricate_currentness() {
        assert_eq!(
            compare_explicit(
                DatFreshnessProvider::NoIntro,
                Some("2025"),
                Some("2024"),
                Ordering::Greater,
                DatFreshnessComparisonSource::TrustedLocalSnapshot,
                DatFreshnessComparisonBasis::Version,
                42,
            )
            .state,
            DatFreshnessState::Unknown
        );
    }

    #[test]
    fn screen_scraper_has_no_freshness_source() {
        assert_eq!(
            tosec_active_import_freshness(None)
                .evidence
                .provider,
            DatFreshnessProvider::Tosec
        );
    }

    #[test]
    fn tosec_import_cannot_self_certify_current() {
        assert_eq!(
            tosec_active_import_freshness(Some("2025-01")).state,
            DatFreshnessState::Unknown
        );
    }

    #[test]
    fn unsupported_local_source_stays_unknown() {
        assert_eq!(
            active_without_comparison(DatFreshnessProvider::LocalComparison, Some("local")).state,
            DatFreshnessState::Unknown
        );
    }

    #[test]
    fn managed_mame_state_requires_a_successful_provider_check() {
        let descriptor = ManagedDatSourceDescriptor::mame_software_list("amstrad").unwrap();
        let mut state = ManagedDatState::new(
            &descriptor,
            ManagedDatSnapshot::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(managed_dat_freshness(Some(&state)).state, DatFreshnessState::Unknown);
        state.upstream_revision = Some("bbbbbbbb".into());
        state.last_checked_at_unix_seconds = Some(42);
        assert_eq!(managed_dat_freshness(Some(&state)).state, DatFreshnessState::Unknown);
        assert_eq!(
            managed_dat_freshness_after_successful_check(Some(&state)).state,
            DatFreshnessState::Current
        );
        state.last_failure = Some("timeout".into());
        assert_eq!(
            managed_dat_freshness(Some(&state)).state,
            DatFreshnessState::CheckFailed
        );
        state.last_failure = None;
        state.last_checked_at_unix_seconds = None;
        state.previous_snapshot = Some(
            ManagedDatSnapshot::new(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            )
            .unwrap(),
        );
        assert_eq!(managed_dat_freshness(Some(&state)).state, DatFreshnessState::Unknown);
    }

    #[test]
    fn freshness_evaluation_is_deterministic_and_rollback_needs_recheck() {
        let first = active_without_comparison(DatFreshnessProvider::NoIntro, Some("old"));
        let second = active_without_comparison(DatFreshnessProvider::NoIntro, Some("old"));
        assert_eq!(first, second);
        assert_ne!(first.state, DatFreshnessState::Current);
    }
}
