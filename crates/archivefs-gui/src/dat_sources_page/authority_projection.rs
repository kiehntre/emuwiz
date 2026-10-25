//! Read-only projection of core DAT authority expectations for the DAT page.
//!
//! This module deliberately owns no authority rules.  It joins the core's
//! expectation with facts already available to the page and keeps coverage
//! separate from freshness so an active source cannot become "Current" merely
//! because it is installed.

use archivefs_core::dat::coverage_expectations::{
    CoverageSourceRole, ExpectedAuthoritativeSource, ExpectedCoverageSource,
    PlatformCoverageExpectation, PlatformCoverageState, UnsupportedPlatformKind,
    expected_authoritative_coverage,
};
use archivefs_core::identity_source::freshness::DatFreshnessState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorityEvidenceFact {
    pub(crate) platform: Option<String>,
    pub(crate) source: ExpectedAuthoritativeSource,
    pub(crate) active: bool,
    pub(crate) managed: bool,
    pub(crate) freshness: DatFreshnessState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthoritySourceView {
    pub(crate) label: String,
    pub(crate) role: CoverageSourceRole,
    pub(crate) active: bool,
    pub(crate) managed: bool,
    pub(crate) freshness: DatFreshnessState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorityProjectionRow {
    pub(crate) platform: String,
    pub(crate) expected_sources: Vec<AuthoritySourceView>,
    pub(crate) coverage: PlatformCoverageState,
    pub(crate) reason: String,
}

pub(crate) fn project_platform(
    platform_hint: Option<&str>,
    facts: &[AuthorityEvidenceFact],
) -> AuthorityProjectionRow {
    let expectation = expected_authoritative_coverage(platform_hint);
    match expectation {
        PlatformCoverageExpectation::UnsupportedOrUnknown { platform, kind, .. } => {
            let coverage = match kind {
                UnsupportedPlatformKind::Unknown => PlatformCoverageState::UnknownPlatform,
                UnsupportedPlatformKind::KnownButUnmapped => {
                    PlatformCoverageState::NoExpectedSource
                }
            };
            AuthorityProjectionRow {
                platform: platform.unwrap_or_else(|| "Unknown platform".to_string()),
                expected_sources: Vec::new(),
                coverage,
                reason: match coverage {
                    PlatformCoverageState::UnknownPlatform => {
                        "The platform is unknown, so no authoritative DAT source is inferred."
                            .to_string()
                    }
                    _ => "No authoritative DAT source is currently expected for this platform."
                        .to_string(),
                },
            }
        }
        PlatformCoverageExpectation::NoKnownAuthoritativeSource { platform, .. } => {
            AuthorityProjectionRow {
                platform,
                expected_sources: Vec::new(),
                coverage: PlatformCoverageState::NoExpectedSource,
                reason: "No authoritative DAT source is currently expected for this platform."
                    .to_string(),
            }
        }
        PlatformCoverageExpectation::ExpectedAuthoritativeSource { platform, source } => {
            project_expected(platform, vec![source], facts)
        }
        PlatformCoverageExpectation::MultipleCandidateSources {
            platform, sources, ..
        } => project_expected(platform, sources, facts),
    }
}

fn project_expected(
    platform: String,
    expected: Vec<ExpectedCoverageSource>,
    facts: &[AuthorityEvidenceFact],
) -> AuthorityProjectionRow {
    let expected_sources = expected
        .iter()
        .map(|source| {
            let matching = facts.iter().filter(|fact| {
                fact.platform.as_deref() == Some(platform.as_str()) && fact.source == source.source
            });
            let facts = matching.collect::<Vec<_>>();
            AuthoritySourceView {
                label: source.source.label().to_string(),
                role: source.role,
                active: facts.iter().any(|fact| fact.active),
                managed: facts.iter().any(|fact| fact.active && fact.managed),
                freshness: aggregate_freshness(&facts),
            }
        })
        .collect::<Vec<_>>();

    let any_fact = facts.iter().any(|fact| {
        fact.platform.as_deref() == Some(platform.as_str())
            && expected
                .iter()
                .any(|expected| expected.source == fact.source)
    });
    let any_active = expected_sources.iter().any(|source| source.active);
    let any_unmanaged = expected_sources
        .iter()
        .any(|source| source.active && !source.managed);
    let update_available = expected_sources
        .iter()
        .any(|source| source.active && source.freshness == DatFreshnessState::UpdateAvailable);

    let coverage = if !any_fact {
        PlatformCoverageState::ExpectedButMissing
    } else if !any_active {
        PlatformCoverageState::ExpectedButInactive
    } else if update_available {
        PlatformCoverageState::ExpectedButStale
    } else if any_unmanaged {
        PlatformCoverageState::ExpectedButUnmanaged
    } else {
        PlatformCoverageState::Covered
    };

    let reason = match coverage {
        PlatformCoverageState::ExpectedButMissing => {
            format!(
                "Expected {} evidence is not installed.",
                source_labels(&expected_sources)
            )
        }
        PlatformCoverageState::ExpectedButInactive => {
            format!(
                "Expected {} evidence is installed but inactive.",
                source_labels(&expected_sources)
            )
        }
        PlatformCoverageState::ExpectedButStale => {
            format!(
                "{} evidence is active, but an update is available.",
                source_labels(&expected_sources)
            )
        }
        PlatformCoverageState::ExpectedButUnmanaged => {
            format!(
                "{} evidence is active but is user-managed.",
                source_labels(&expected_sources)
            )
        }
        PlatformCoverageState::Covered => "Expected evidence is active.".to_string(),
        PlatformCoverageState::NoExpectedSource | PlatformCoverageState::UnknownPlatform => {
            "No authoritative DAT source is currently expected for this platform.".to_string()
        }
    };

    AuthorityProjectionRow {
        platform,
        expected_sources,
        coverage,
        reason,
    }
}

fn source_labels(sources: &[AuthoritySourceView]) -> String {
    sources
        .iter()
        .map(|source| source.label.as_str())
        .collect::<Vec<_>>()
        .join(" + ")
}

fn aggregate_freshness(facts: &[&AuthorityEvidenceFact]) -> DatFreshnessState {
    if facts.is_empty() {
        return DatFreshnessState::NeverChecked;
    }
    [
        DatFreshnessState::CheckFailed,
        DatFreshnessState::UpdateAvailable,
        DatFreshnessState::Current,
        DatFreshnessState::Unknown,
        DatFreshnessState::NeverChecked,
    ]
    .into_iter()
    .find(|state| {
        facts
            .iter()
            .any(|fact| fact.active && fact.freshness == *state)
    })
    .unwrap_or(DatFreshnessState::NeverChecked)
}

pub(crate) fn freshness_label(state: DatFreshnessState) -> &'static str {
    match state {
        DatFreshnessState::Current => "Current",
        DatFreshnessState::UpdateAvailable => "Update available",
        DatFreshnessState::Unknown => "Unknown",
        DatFreshnessState::CheckFailed => "Check failed",
        DatFreshnessState::NeverChecked => "Never checked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(
        platform: &'static str,
        source: ExpectedAuthoritativeSource,
        active: bool,
        managed: bool,
        freshness: DatFreshnessState,
    ) -> AuthorityEvidenceFact {
        AuthorityEvidenceFact {
            platform: Some(platform.to_string()),
            source,
            active,
            managed,
            freshness,
        }
    }

    fn dat(ecosystem: archivefs_core::dat::model::DatEcosystem) -> ExpectedAuthoritativeSource {
        ExpectedAuthoritativeSource::Dat(ecosystem)
    }

    #[test]
    fn expected_no_intro_missing_is_explicit() {
        let row = project_platform(Some("Game Boy"), &[]);
        assert_eq!(row.coverage, PlatformCoverageState::ExpectedButMissing);
        assert!(row.reason.contains("No-Intro"));
    }

    #[test]
    fn redump_active_unknown_is_not_current() {
        let row = project_platform(
            Some("PS2"),
            &[fact(
                "PS2",
                dat(archivefs_core::dat::model::DatEcosystem::Redump),
                true,
                true,
                DatFreshnessState::Unknown,
            )],
        );
        assert_eq!(row.coverage, PlatformCoverageState::Covered);
        assert_eq!(
            row.expected_sources[0].freshness,
            DatFreshnessState::Unknown
        );
    }

    #[test]
    fn freshness_states_are_preserved() {
        for state in [
            DatFreshnessState::Current,
            DatFreshnessState::UpdateAvailable,
            DatFreshnessState::CheckFailed,
            DatFreshnessState::NeverChecked,
        ] {
            let row = project_platform(
                Some("PS2"),
                &[fact(
                    "PS2",
                    dat(archivefs_core::dat::model::DatEcosystem::Redump),
                    true,
                    true,
                    state,
                )],
            );
            assert_eq!(row.expected_sources[0].freshness, state);
        }
    }

    #[test]
    fn inactive_and_unmanaged_states_are_distinct() {
        let inactive = project_platform(
            Some("PS2"),
            &[fact(
                "PS2",
                dat(archivefs_core::dat::model::DatEcosystem::Redump),
                false,
                true,
                DatFreshnessState::NeverChecked,
            )],
        );
        assert_eq!(
            inactive.coverage,
            PlatformCoverageState::ExpectedButInactive
        );
        let unmanaged = project_platform(
            Some("PS2"),
            &[fact(
                "PS2",
                dat(archivefs_core::dat::model::DatEcosystem::Redump),
                true,
                false,
                DatFreshnessState::Unknown,
            )],
        );
        assert_eq!(
            unmanaged.coverage,
            PlatformCoverageState::ExpectedButUnmanaged
        );
    }

    #[test]
    fn update_available_is_expected_but_stale() {
        let row = project_platform(
            Some("PS2"),
            &[fact(
                "PS2",
                dat(archivefs_core::dat::model::DatEcosystem::Redump),
                true,
                true,
                DatFreshnessState::UpdateAvailable,
            )],
        );
        assert_eq!(row.coverage, PlatformCoverageState::ExpectedButStale);
    }

    #[test]
    fn multiple_sources_keep_core_order_and_roles() {
        let row = project_platform(
            Some("Arcade"),
            &[
                fact(
                    "Arcade",
                    dat(archivefs_core::dat::model::DatEcosystem::FBNeo),
                    true,
                    false,
                    DatFreshnessState::Unknown,
                ),
                fact(
                    "Arcade",
                    dat(archivefs_core::dat::model::DatEcosystem::MAMEArcade),
                    true,
                    true,
                    DatFreshnessState::Current,
                ),
            ],
        );
        assert_eq!(row.expected_sources.len(), 2);
        assert_eq!(row.expected_sources[0].label, "MAME listxml");
        assert_eq!(row.expected_sources[0].role, CoverageSourceRole::Primary);
        assert_eq!(row.expected_sources[1].label, "FinalBurn Neo");
        assert_eq!(
            row.expected_sources[1].role,
            CoverageSourceRole::Supplementary
        );
    }

    #[test]
    fn unmapped_and_unknown_platforms_are_not_authoritative() {
        assert_eq!(
            project_platform(Some("Philips CD-i"), &[]).coverage,
            PlatformCoverageState::NoExpectedSource
        );
        assert_eq!(
            project_platform(Some("made-up"), &[]).coverage,
            PlatformCoverageState::UnknownPlatform
        );
        assert!(
            project_platform(Some("Philips CD-i"), &[])
                .expected_sources
                .is_empty()
        );
    }

    #[test]
    fn scummvm_and_tosec_expectations_use_core_sources() {
        let scumm = project_platform(Some("ScummVM"), &[]);
        assert_eq!(scumm.expected_sources[0].label, "Official ScummVM detector");
        let tosec = project_platform(Some("Amiga"), &[]);
        assert_eq!(tosec.expected_sources[0].label, "TOSEC");
    }

    #[test]
    fn no_authoritative_metadata_provider_can_appear() {
        let row = project_platform(Some("Game Boy"), &[]);
        assert!(
            row.expected_sources
                .iter()
                .all(|source| source.label != "ScreenScraper" && source.label != "RomM")
        );
    }
}
