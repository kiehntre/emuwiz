//! Read-only projection of an already-resolved media topology into launch
//! planning.  This module owns no grouping, identity, or representation
//! truth; it only turns [`crate::media_set::MediaSet`] and its declarative
//! [`crate::media_set::MediaSwapPlan`] into launch-safe facts.

use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::evidence_resolution::ActionSafety;
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContentRef, LaunchPlan, LaunchPlanSummary, RememberedPreference,
    StandaloneProfileInput, build_launch_plan,
};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::media_set::{
    MediaProfile, MediaSet, MediaSetState, MediaSwapPlan, MediaSwapStep, media_swap_plan,
};
/// Alias kept next to the topology projection so callers do not need to know
/// which generic evidence vocabulary supplies the safety contract.
pub type MediaTopologyActionSafety = ActionSafety;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTopologyMissingMedia {
    pub expected_count: Option<u16>,
    pub expected_unit: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTopologyLaunchProjection {
    pub media_set_id: String,
    pub topology_state: MediaSetState,
    pub start_media: Option<MediaSwapStep>,
    pub media_sequence: Vec<MediaSwapStep>,
    pub swap_plan: MediaSwapPlan,
    pub action_safety: MediaTopologyActionSafety,
    pub missing_media: Vec<MediaTopologyMissingMedia>,
    pub conflicts: Vec<String>,
    pub explanation: String,
}

fn first_launch_media(plan: &MediaSwapPlan) -> Option<MediaSwapStep> {
    let first = plan.ordered_media.first()?;
    let ordinal = first.ordinal.as_ref()?;
    if !matches!(
        first.role,
        crate::media_set::MediaRole::BootMedia
            | crate::media_set::MediaRole::GameMedia
            | crate::media_set::MediaRole::PlayMedia
    ) {
        return None;
    }
    // The topology engine supplies the order.  Requiring every step to have
    // an explicit ordinal prevents this adapter from turning lexical/member
    // order into a guessed start disc.
    if plan.ordered_media.iter().any(|step| step.ordinal.is_none()) {
        return None;
    }
    if plan
        .ordered_media
        .iter()
        .filter_map(|step| step.ordinal.as_ref())
        .map(|value| value.number)
        .min()
        != Some(ordinal.number)
    {
        return None;
    }
    Some(first.clone())
}

fn missing_media(set: &MediaSet) -> Vec<MediaTopologyMissingMedia> {
    let Some((expected, _)) = &set.expected_count else {
        return Vec::new();
    };
    let observed = set
        .members
        .iter()
        .filter_map(|member| member.ordinal.as_ref().map(|ordinal| ordinal.number))
        .collect::<std::collections::BTreeSet<_>>();
    (1..=expected.count)
        .filter(|number| !observed.contains(number))
        .map(|number| MediaTopologyMissingMedia {
            expected_count: Some(expected.count),
            expected_unit: Some(format!("{:?}", expected.unit)),
            detail: format!("required {:?} {} is missing", expected.unit, number),
        })
        .collect::<Vec<_>>()
}

/// Projects one existing media set.  No filesystem, database, network, or
/// process operation occurs here.
pub fn project_media_set_for_launch(
    set: &MediaSet,
    profile: Option<&MediaProfile>,
) -> MediaTopologyLaunchProjection {
    let swap_plan = media_swap_plan(set, profile);
    let start_media = first_launch_media(&swap_plan);
    let missing_media = missing_media(set);
    let mut conflicts = set
        .conflicts
        .iter()
        .filter(|conflict| conflict.blocking)
        .map(|conflict| conflict.detail.clone())
        .collect::<Vec<_>>();
    conflicts.extend(
        swap_plan
            .blockers
            .iter()
            .map(|blocker| blocker.detail.clone()),
    );
    conflicts.sort();
    conflicts.dedup();

    let (action_safety, explanation) = match set.state {
        MediaSetState::ConflictingSet => (
            ActionSafety::Blocked,
            "media members contain conflicting identity or topology evidence".to_string(),
        ),
        MediaSetState::AmbiguousSet => (
            ActionSafety::Blocked,
            "multiple media interpretations remain; review is required".to_string(),
        ),
        MediaSetState::IncompleteSet => (
            ActionSafety::Blocked,
            missing_media
                .first()
                .map(|missing| missing.detail.clone())
                .unwrap_or_else(|| "the media set is incomplete".to_string()),
        ),
        MediaSetState::UnsupportedSet => (
            ActionSafety::Blocked,
            "this media set is unsupported for safe launch planning".to_string(),
        ),
        MediaSetState::UnverifiedSet => (
            ActionSafety::ReviewRequired,
            "media membership is not verified; review the set before launch".to_string(),
        ),
        MediaSetState::CompleteSet if !conflicts.is_empty() => (
            ActionSafety::Blocked,
            "the declarative media plan contains a blocking conflict".to_string(),
        ),
        MediaSetState::CompleteSet if start_media.is_none() => (
            ActionSafety::ReviewRequired,
            "the verified start medium could not be established".to_string(),
        ),
        MediaSetState::CompleteSet => (
            ActionSafety::SafeToAct,
            "the first verified medium is the launch start; later media remain available for explicit swaps".to_string(),
        ),
    };

    MediaTopologyLaunchProjection {
        media_set_id: set.identity.key.value.clone(),
        topology_state: set.state,
        start_media,
        media_sequence: swap_plan.ordered_media.clone(),
        swap_plan,
        action_safety,
        missing_media,
        conflicts,
        explanation,
    }
}

/// Builds the existing launch plan and attaches the topology projection.
/// Topology remains an input-owned truth source; this function only adds a
/// typed fail-closed gate to the existing candidates.
pub fn build_launch_plan_with_media_set(
    identity: &CanonicalIdentityStatus,
    content: &LaunchContentRef,
    standalone_profiles: &[StandaloneProfileInput],
    retroarch: &RetroArchEnvironmentReport,
    remembered: &[RememberedPreference],
    set: &MediaSet,
    profile: Option<&MediaProfile>,
) -> LaunchPlan {
    let projection = project_media_set_for_launch(set, profile);
    let mut plan = build_launch_plan(
        identity,
        content,
        standalone_profiles,
        retroarch,
        remembered,
    );
    let safety = projection.action_safety;
    if safety != ActionSafety::SafeToAct {
        let kind = if safety == ActionSafety::Blocked {
            LaunchBlockerKind::MediaTopologyBlocked
        } else {
            LaunchBlockerKind::MediaTopologyReviewRequired
        };
        for candidate in &mut plan.candidates {
            candidate
                .blockers
                .push(LaunchBlocker::new(kind, projection.explanation.clone()));
            candidate.readiness = LaunchReadiness::Blocked;
        }
        let mut summary = LaunchPlanSummary {
            candidates: plan.candidates.len(),
            ..Default::default()
        };
        summary.blocked = plan.candidates.len();
        plan.summary = summary;
    }
    plan.media_topology = Some(projection);
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_set::{
        Equivalence, EvidenceKind, IdentityKey, MediaAvailability, MediaEvidence, MediaFamily,
        MediaOrdinal, MediaRecord, MediaRole, MediaSetIdentity, MediaSource, Provenance,
    };
    use std::{collections::BTreeSet, path::PathBuf};

    fn record(name: &str, ordinal: u16) -> MediaRecord {
        let mut evidence = MediaEvidence::new(EvidenceKind::TrustedDat, "test DAT");
        evidence.release = Some(IdentityKey::new("release", "game"));
        evidence.medium = Some(IdentityKey::new("disc", ordinal.to_string()));
        evidence.ordinal = Some(MediaOrdinal {
            number: ordinal,
            unit: crate::media_set::OrdinalUnit::Disc,
        });
        MediaRecord {
            source: MediaSource {
                path: PathBuf::from(name),
                archive_member: None,
            },
            platform: Some("PlayStation".into()),
            family: Some(MediaFamily::Optical),
            format: "chd".into(),
            availability: MediaAvailability::Observed,
            evidence: vec![evidence],
            warnings: Vec::new(),
        }
    }

    fn set(count: u16) -> MediaSet {
        let mut members = Vec::new();
        for ordinal in 1..=count {
            let record = record(&format!("disc{ordinal}.chd"), ordinal);
            let representation = crate::media_set::MediaRepresentation {
                record,
                release_identity: Some(MediaSetIdentity {
                    key: IdentityKey::new("release", "game"),
                    provenance: vec![Provenance::new(EvidenceKind::TrustedDat, "test")],
                    verified: true,
                }),
                media_identity: Some(crate::media_set::MediaIdentity {
                    key: IdentityKey::new("disc", ordinal.to_string()),
                    provenance: Provenance::new(EvidenceKind::TrustedDat, "test"),
                    equivalence: Equivalence::AuthorityMapping,
                }),
                ordinal: Some(MediaOrdinal {
                    number: ordinal,
                    unit: crate::media_set::OrdinalUnit::Disc,
                }),
                side: None,
                role: MediaRole::GameMedia,
                variant: Default::default(),
                expected_count: None,
                side_layout: crate::media_set::SideLayout::WholeMedium,
                expected_sides: BTreeSet::new(),
                confidence: crate::media_set::MediaSetConfidence::Proven,
                conflicts: Vec::new(),
            };
            members.push(crate::media_set::MediaSetMember {
                id: format!("disc{ordinal}"),
                ordinal: Some(MediaOrdinal {
                    number: ordinal,
                    unit: crate::media_set::OrdinalUnit::Disc,
                }),
                role: MediaRole::GameMedia,
                sides: BTreeSet::new(),
                side_layout: crate::media_set::SideLayout::WholeMedium,
                representations: vec![representation],
            });
        }
        MediaSet {
            identity: MediaSetIdentity {
                key: IdentityKey::new("release", "game"),
                provenance: Vec::new(),
                verified: true,
            },
            platform: Some("PlayStation".into()),
            family: Some(MediaFamily::Optical),
            variant: Default::default(),
            members,
            expected_count: Some((
                crate::media_set::ExpectedCount {
                    count,
                    unit: crate::media_set::OrdinalUnit::Disc,
                },
                Provenance::new(EvidenceKind::TrustedDat, "test"),
            )),
            state: MediaSetState::CompleteSet,
            confidence: crate::media_set::MediaSetConfidence::Proven,
            conflicts: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn complete_set_projects_verified_start_and_order() {
        let projection = project_media_set_for_launch(&set(3), None);
        assert_eq!(projection.action_safety, ActionSafety::SafeToAct);
        assert_eq!(projection.start_media.as_ref().unwrap().member_id, "disc1");
        assert_eq!(projection.media_sequence.len(), 3);
    }

    #[test]
    fn incomplete_set_is_blocked_with_required_media() {
        let mut set = set(1);
        set.expected_count = Some((
            crate::media_set::ExpectedCount {
                count: 2,
                unit: crate::media_set::OrdinalUnit::Disc,
            },
            Provenance::new(EvidenceKind::TrustedDat, "test"),
        ));
        set.state = MediaSetState::IncompleteSet;
        let projection = project_media_set_for_launch(&set, None);
        assert_eq!(projection.action_safety, ActionSafety::Blocked);
        assert_eq!(projection.missing_media.len(), 1);
    }

    #[test]
    fn ambiguous_conflicting_and_unverified_sets_fail_closed() {
        for state in [MediaSetState::AmbiguousSet, MediaSetState::ConflictingSet] {
            let mut set = set(2);
            set.state = state;
            assert_eq!(
                project_media_set_for_launch(&set, None).action_safety,
                ActionSafety::Blocked
            );
        }
        let mut unverified = set(2);
        unverified.state = MediaSetState::UnverifiedSet;
        assert_eq!(
            project_media_set_for_launch(&unverified, None).action_safety,
            ActionSafety::ReviewRequired
        );
    }

    #[test]
    fn unknown_start_is_review_required_even_for_complete_set() {
        let mut set = set(2);
        set.members[0].ordinal = None;
        set.members[0].representations[0].ordinal = None;
        let projection = project_media_set_for_launch(&set, None);
        assert_eq!(projection.action_safety, ActionSafety::ReviewRequired);
        assert!(projection.start_media.is_none());
    }

    #[test]
    fn one_hundred_thousand_projections_are_linear_and_deterministic() {
        let set = set(3);
        let started = std::time::Instant::now();
        let mut digest = 0usize;
        for _ in 0..100_000 {
            let projection = project_media_set_for_launch(&set, None);
            digest = digest.wrapping_add(projection.media_sequence.len());
            assert_eq!(projection.action_safety, ActionSafety::SafeToAct);
        }
        assert_eq!(digest, 300_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }
}
