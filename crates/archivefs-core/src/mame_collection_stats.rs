//! Deterministic collection statistics over already-evaluated MAME evidence.
//!
//! This module never scans, hashes, parses, or resolves.  It only counts the
//! typed results produced by the current MAME compatibility and alternative
//! layout projections.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::arcade_mame_compatibility::{
    MameCompletenessEvaluation, MameMismatchReason, MameSetCompatibilityState,
};
use crate::mame_authority_impact::MameAuthorityIdentity;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MameDependencyStats {
    /// Each field counts sets, not individual requirements.
    pub rom_member_sets: usize,
    pub parent_sets: usize,
    pub bios_sets: usize,
    pub device_sets: usize,
    pub chd_sets: usize,
    pub dependency_sets: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MameAlternativeLayoutStats {
    pub single_alternative_sets: usize,
    pub multiple_alternative_sets: usize,
    pub satisfied_by_non_first_alternative_sets: usize,
    pub recovered_by_alternative_sets: usize,
    pub unresolved_alternative_sets: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MameCompletenessRatios {
    /// Complete (including warning-compatible) / known sets. Unknown and
    /// evidence-unavailable sets are excluded from this denominator.
    pub known_complete_ratio: Option<f64>,
    /// Complete (including warning-compatible) / all evaluated sets.
    pub confirmed_complete_ratio: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MameCollectionCompletenessStats {
    pub authority: MameAuthorityIdentity,
    pub evaluated_set_names: Vec<String>,
    pub total_sets: usize,
    pub complete_sets: usize,
    pub warning_sets: usize,
    pub incomplete_sets: usize,
    pub unknown_sets: usize,
    pub evidence_unavailable_sets: usize,
    pub unsupported_sets: usize,
    pub dependency_breakdown: MameDependencyStats,
    pub layout_breakdown: MameAlternativeLayoutStats,
    pub ratios: MameCompletenessRatios,
}

impl MameCollectionCompletenessStats {
    pub fn from_results(
        authority: MameAuthorityIdentity,
        results: impl IntoIterator<Item = MameCompletenessEvaluation>,
    ) -> Self {
        let mut results: Vec<_> = results.into_iter().collect();
        results.sort_by_key(result_key);
        results.dedup_by(|left, right| left.set_name == right.set_name);

        let mut stats = Self {
            authority,
            evaluated_set_names: results
                .iter()
                .map(|result| result.set_name.clone())
                .collect(),
            total_sets: results.len(),
            complete_sets: 0,
            warning_sets: 0,
            incomplete_sets: 0,
            unknown_sets: 0,
            evidence_unavailable_sets: 0,
            unsupported_sets: 0,
            dependency_breakdown: MameDependencyStats::default(),
            layout_breakdown: MameAlternativeLayoutStats::default(),
            ratios: MameCompletenessRatios {
                known_complete_ratio: None,
                confirmed_complete_ratio: None,
            },
        };

        for result in &results {
            match result.state {
                MameSetCompatibilityState::Compatible => stats.complete_sets += 1,
                MameSetCompatibilityState::CompatibleWithWarnings => {
                    stats.complete_sets += 1;
                    stats.warning_sets += 1;
                }
                MameSetCompatibilityState::Incompatible => stats.incomplete_sets += 1,
                MameSetCompatibilityState::Unknown => stats.unknown_sets += 1,
                MameSetCompatibilityState::Unsupported => stats.unsupported_sets += 1,
            }
            if result
                .mismatches
                .iter()
                .any(|item| item.reason == MameMismatchReason::EvidenceUnavailable)
            {
                stats.evidence_unavailable_sets += 1;
            }
            count_dependencies(result, &mut stats.dependency_breakdown);
            count_layouts(result, &mut stats.layout_breakdown);
        }

        let known_sets = stats.total_sets.saturating_sub(stats.unknown_sets);
        stats.ratios.known_complete_ratio = ratio(stats.complete_sets, known_sets);
        stats.ratios.confirmed_complete_ratio = ratio(stats.complete_sets, stats.total_sets);
        stats
    }
}

fn result_key(
    result: &MameCompletenessEvaluation,
) -> (String, MameSetCompatibilityState, usize, String) {
    let mismatches = result
        .mismatches
        .iter()
        .map(|item| format!("{:?}:{}:{:?}", item.reason, item.set_name, item.member_name))
        .collect::<Vec<_>>()
        .join("|");
    (
        result.set_name.clone(),
        result.state,
        result.alternatives.len(),
        mismatches,
    )
}

fn ratio(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator != 0).then_some(numerator as f64 / denominator as f64)
}

fn count_dependencies(result: &MameCompletenessEvaluation, stats: &mut MameDependencyStats) {
    let reasons: BTreeSet<_> = result.mismatches.iter().map(|item| item.reason).collect();
    let rom = reasons.iter().any(|reason| {
        matches!(
            reason,
            MameMismatchReason::RequiredFileMissing
                | MameMismatchReason::WrongSize
                | MameMismatchReason::CrcMismatch
                | MameMismatchReason::Sha1Mismatch
        )
    });
    let parent = reasons.contains(&MameMismatchReason::ParentMissing);
    let bios = reasons.contains(&MameMismatchReason::BiosMissing);
    let device = reasons.contains(&MameMismatchReason::DeviceDependencyMissing);
    let chd = reasons.contains(&MameMismatchReason::ChdMissing)
        || reasons.contains(&MameMismatchReason::ChdHashMismatch);
    stats.rom_member_sets += usize::from(rom);
    stats.parent_sets += usize::from(parent);
    stats.bios_sets += usize::from(bios);
    stats.device_sets += usize::from(device);
    stats.chd_sets += usize::from(chd);
    stats.dependency_sets += usize::from(rom || parent || bios || device || chd);
}

fn count_layouts(result: &MameCompletenessEvaluation, stats: &mut MameAlternativeLayoutStats) {
    match result.alternatives.len() {
        0 | 1 => stats.single_alternative_sets += 1,
        _ => {
            stats.multiple_alternative_sets += 1;
            let first_satisfied = result.alternatives.first().is_some_and(is_complete);
            let later_satisfied = result.alternatives.iter().skip(1).any(is_complete);
            if later_satisfied && !first_satisfied {
                stats.satisfied_by_non_first_alternative_sets += 1;
                stats.recovered_by_alternative_sets += 1;
            }
            if result.state == MameSetCompatibilityState::Unknown
                && result
                    .alternatives
                    .iter()
                    .any(|item| item.state == MameSetCompatibilityState::Unknown)
            {
                stats.unresolved_alternative_sets += 1;
            }
        }
    }
}

fn is_complete(result: &crate::arcade_mame_compatibility::MameSetCompatibility) -> bool {
    matches!(
        result.state,
        MameSetCompatibilityState::Compatible | MameSetCompatibilityState::CompatibleWithWarnings
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_mame_compatibility::{
        InstalledMameEvidence, MameCompletenessAlternativeSet, MameSetExpectation,
        ObservedArcadeSetEvidence, ObservedEvidenceCompleteness,
    };
    use crate::dat::model::{DatGameEntry, DatRomEntry};
    use std::path::PathBuf;

    fn authority() -> MameAuthorityIdentity {
        MameAuthorityIdentity {
            artifact_sha256: Some("authority-sha".into()),
            version: Some("0.264".into()),
            provenance: "synthetic DAT".into(),
        }
    }

    fn mame() -> InstalledMameEvidence {
        InstalledMameEvidence::new(
            "mame",
            "0.264",
            "listxml",
            [PathBuf::from("roms")],
            None,
            None,
        )
    }

    fn expectation(name: &str) -> MameSetExpectation {
        MameSetExpectation::from_game(&DatGameEntry {
            name: name.into(),
            roms: vec![DatRomEntry {
                name: "rom".into(),
                size_bytes: Some(1),
                crc32: Some("aa".into()),
                ..Default::default()
            }],
            ..Default::default()
        })
    }

    fn evaluation(name: &str, complete: bool) -> MameCompletenessEvaluation {
        let exp = expectation(name);
        let mut evidence =
            ObservedArcadeSetEvidence::new(name, ObservedEvidenceCompleteness::Complete);
        evidence.observed_set_names.push(name.into());
        if complete {
            evidence
                .observed_roms
                .push(crate::arcade_mame_compatibility::MameObservedRom {
                    set_name: name.into(),
                    name: "rom".into(),
                    size_bytes: Some(1),
                    crc32: Some("aa".into()),
                    sha1: None,
                });
        }
        evidence.audit_alternatives(
            &mame(),
            &MameCompletenessAlternativeSet::from_expectations([exp]),
        )
    }

    #[test]
    fn empty_collection_has_explicit_empty_ratios() {
        let stats = MameCollectionCompletenessStats::from_results(authority(), []);
        assert_eq!(stats.total_sets, 0);
        assert_eq!(stats.ratios.known_complete_ratio, None);
        assert_eq!(stats.ratios.confirmed_complete_ratio, None);
    }

    #[test]
    fn primary_buckets_and_evidence_unavailable_are_not_conflated() {
        let mut unknown = evaluation("unknown", true);
        unknown.state = MameSetCompatibilityState::Unknown;
        unknown
            .mismatches
            .push(crate::arcade_mame_compatibility::MameMismatch {
                reason: MameMismatchReason::EvidenceUnavailable,
                set_name: "unknown".into(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "partial".into(),
            });
        let stats = MameCollectionCompletenessStats::from_results(
            authority(),
            [
                evaluation("complete", true),
                evaluation("incomplete", false),
                unknown,
            ],
        );
        assert_eq!(
            (
                stats.complete_sets,
                stats.incomplete_sets,
                stats.unknown_sets
            ),
            (1, 1, 1)
        );
        assert_eq!(stats.evidence_unavailable_sets, 1);
        assert_eq!(stats.ratios.known_complete_ratio, Some(0.5));
        assert_eq!(stats.ratios.confirmed_complete_ratio, Some(1.0 / 3.0));
    }

    #[test]
    fn all_complete_all_incomplete_and_unresolved_alternatives_count_cleanly() {
        let complete = MameCollectionCompletenessStats::from_results(
            authority(),
            [evaluation("a", true), evaluation("b", true)],
        );
        assert_eq!(complete.complete_sets, 2);
        let incomplete = MameCollectionCompletenessStats::from_results(
            authority(),
            [evaluation("a", false), evaluation("b", false)],
        );
        assert_eq!(incomplete.incomplete_sets, 2);

        let mut evidence =
            ObservedArcadeSetEvidence::new("uncertain", ObservedEvidenceCompleteness::Partial);
        evidence.observed_set_names.push("uncertain".into());
        let mut uncertain_alternative = expectation("uncertain");
        uncertain_alternative.roms[0].name = "other-rom".into();
        let uncertain = evidence.audit_alternatives(
            &mame(),
            &MameCompletenessAlternativeSet::from_expectations([
                expectation("uncertain"),
                uncertain_alternative,
            ]),
        );
        let stats = MameCollectionCompletenessStats::from_results(authority(), [uncertain]);
        assert_eq!(stats.unknown_sets, 1);
        assert_eq!(stats.layout_breakdown.unresolved_alternative_sets, 1);
    }

    #[test]
    fn dependency_reasons_count_each_set_once() {
        let mut result = evaluation("broken", false);
        result.mismatches.extend([
            crate::arcade_mame_compatibility::MameMismatch {
                reason: MameMismatchReason::BiosMissing,
                set_name: "bios".into(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "bios".into(),
            },
            crate::arcade_mame_compatibility::MameMismatch {
                reason: MameMismatchReason::DeviceDependencyMissing,
                set_name: "device".into(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "device".into(),
            },
            crate::arcade_mame_compatibility::MameMismatch {
                reason: MameMismatchReason::ChdMissing,
                set_name: "broken".into(),
                member_name: Some("disk".into()),
                expected_size: None,
                observed_size: None,
                detail: "chd".into(),
            },
            crate::arcade_mame_compatibility::MameMismatch {
                reason: MameMismatchReason::ParentMissing,
                set_name: "parent".into(),
                member_name: None,
                expected_size: None,
                observed_size: None,
                detail: "parent".into(),
            },
        ]);
        let stats = MameCollectionCompletenessStats::from_results(authority(), [result]);
        assert_eq!(stats.dependency_breakdown.dependency_sets, 1);
        assert_eq!(stats.dependency_breakdown.bios_sets, 1);
        assert_eq!(stats.dependency_breakdown.device_sets, 1);
        assert_eq!(stats.dependency_breakdown.chd_sets, 1);
        assert_eq!(stats.dependency_breakdown.parent_sets, 1);
    }

    #[test]
    fn alternative_layouts_are_summarized_without_preference_policy() {
        let first = expectation("layout");
        let mut second = first.clone();
        let mut first = first;
        first.roms[0].name = "rom-a".into();
        second.roms[0].name = "rom-b".into();
        let mut evidence =
            ObservedArcadeSetEvidence::new("layout", ObservedEvidenceCompleteness::Complete);
        evidence.observed_set_names.push("layout".into());
        evidence
            .observed_roms
            .push(crate::arcade_mame_compatibility::MameObservedRom {
                set_name: "layout".into(),
                name: "rom-b".into(),
                size_bytes: Some(1),
                crc32: Some("aa".into()),
                sha1: None,
            });
        let result = evidence.audit_alternatives(
            &mame(),
            &MameCompletenessAlternativeSet::from_expectations([first, second]),
        );
        let stats = MameCollectionCompletenessStats::from_results(authority(), [result]);
        assert_eq!(stats.layout_breakdown.multiple_alternative_sets, 1);
        assert_eq!(
            stats
                .layout_breakdown
                .satisfied_by_non_first_alternative_sets,
            1
        );
    }

    #[test]
    fn ordering_and_provenance_are_stable() {
        let a = MameCollectionCompletenessStats::from_results(
            authority(),
            [evaluation("z", true), evaluation("a", true)],
        );
        let b = MameCollectionCompletenessStats::from_results(
            authority(),
            [evaluation("a", true), evaluation("z", true)],
        );
        assert_eq!(a, b);
        assert_eq!(a.evaluated_set_names, vec!["a", "z"]);
        assert_eq!(
            a.authority.artifact_sha256.as_deref(),
            Some("authority-sha")
        );
    }
}
