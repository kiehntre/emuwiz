use super::{
    engine::{issue, role_order},
    model::*,
};
use std::collections::{BTreeMap, BTreeSet};

/// A declarative plan only: no playlist, command, filesystem write or launch gate.
pub fn media_swap_plan(set: &MediaSet, profile: Option<&MediaProfile>) -> MediaSwapPlan {
    let mut blockers = set
        .conflicts
        .iter()
        .filter(|c| c.blocking)
        .cloned()
        .collect::<Vec<_>>();
    let mut warnings = set.warnings.clone();
    let mut steps = Vec::new();
    if set.state != MediaSetState::CompleteSet {
        blockers.push(issue(
            ConflictKind::UnprovenGrouping,
            format!(
                "Topology state is {:?}; this is not launch authorization",
                set.state
            ),
            true,
        ));
    }
    let compatible_profile = profile.filter(|p| {
        crate::platform::platform_by_id(&p.platform)
            .or_else(|| crate::platform::platform_for_alias(&p.platform))
            .map(|p| p.id)
            == set.platform.as_deref()
    });
    if profile.is_some() && compatible_profile.is_none() {
        blockers.push(issue(
            ConflictKind::PlatformConflict,
            "Selected profile belongs to another platform",
            true,
        ));
    }
    for member in &set.members {
        // Whole-medium images already contain their sides. Separate side images
        // retain explicit side steps and never increase the physical disk count.
        let separate = member.side_layout != SideLayout::WholeMedium && !member.sides.is_empty();
        let sides = if separate {
            member.sides.iter().cloned().map(Some).collect::<Vec<_>>()
        } else {
            vec![None]
        };
        for side in sides {
            let mut choices = member
                .representations
                .iter()
                .filter(|r| {
                    r.record.availability == MediaAvailability::Observed
                        && (side.is_none() || r.side == side)
                })
                .collect::<Vec<_>>();
            choices.sort_by(|a, b| a.record.source.cmp(&b.record.source));
            let all = choices
                .iter()
                .map(|r| r.record.source.clone())
                .collect::<Vec<_>>();
            let preferred = if let Some(p) = compatible_profile {
                choices.retain(|r| p.supported_formats.contains(&r.record.format));
                choices.sort_by_key(|r| {
                    p.preferred_formats
                        .iter()
                        .position(|f| f == &r.record.format)
                        .unwrap_or(usize::MAX)
                });
                if choices.is_empty() {
                    blockers.push(issue(
                        ConflictKind::UnavailableRepresentation,
                        format!(
                            "Profile {} supports none of the representations for {}",
                            p.id, member.id
                        ),
                        true,
                    ));
                    None
                } else if choices.len() == 1
                    || rank(&choices[0].record.format, p) < rank(&choices[1].record.format, p)
                {
                    Some(choices[0].record.source.clone())
                } else {
                    blockers.push(issue(
                        ConflictKind::UnresolvedRepresentation,
                        format!(
                            "Profile {} does not select one representation for {}",
                            p.id, member.id
                        ),
                        true,
                    ));
                    None
                }
            } else if choices.len() == 1 {
                Some(choices[0].record.source.clone())
            } else {
                warnings.push("No selected profile establishes a preferred representation".into());
                blockers.push(issue(
                    ConflictKind::UnresolvedRepresentation,
                    format!("Choose a representation for {}", member.id),
                    true,
                ));
                None
            };
            let role = choices.first().map_or(member.role, |r| r.role);
            steps.push(MediaSwapStep {
                member_id: member.id.clone(),
                ordinal: member.ordinal.clone(),
                side,
                role,
                preferred_representation: preferred,
                alternatives: all,
            });
        }
    }
    let provenance = set
        .members
        .iter()
        .flat_map(|m| {
            m.representations
                .iter()
                .flat_map(|r| r.record.evidence.iter().map(|e| e.provenance.clone()))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut transitions = Vec::new();
    let local_provenance = set
        .members
        .iter()
        .map(|m| {
            let sources = m
                .representations
                .iter()
                .flat_map(|r| &r.record.evidence)
                .filter(|e| {
                    e.ordinal.is_some()
                        || e.side.is_some()
                        || e.role.is_some()
                        || !e.relationships.is_empty()
                })
                .map(|e| e.provenance.clone())
                .collect::<BTreeSet<_>>();
            (m.id.as_str(), sources)
        })
        .collect::<BTreeMap<_, _>>();
    let medium_members = set
        .members
        .iter()
        .flat_map(|m| {
            m.representations.iter().flat_map(move |r| {
                r.record
                    .evidence
                    .iter()
                    .filter(|e| e.provenance.trusted())
                    .filter_map(move |e| e.medium.as_ref().map(|id| (id, m.id.as_str())))
            })
        })
        .collect::<BTreeMap<_, _>>();
    let explicit_transitions = set
        .members
        .iter()
        .flat_map(|m| {
            m.representations
                .iter()
                .flat_map(move |r| r.record.evidence.iter().map(move |e| (m, e)))
        })
        .flat_map(|(m, e)| {
            e.relationships.iter().filter_map(|rel| {
                medium_members
                    .get(&rel.target)
                    .map(|target| ((m.id.as_str(), *target), rel.kind))
            })
        })
        .collect::<BTreeMap<_, _>>();
    for (i, pair) in steps.windows(2).enumerate() {
        let kind = if pair[0].member_id == pair[1].member_id {
            TransitionKind::ChangeSide
        } else if set.family == Some(MediaFamily::Tape)
            && pair[0].role == MediaRole::BootMedia
            && role_order(pair[1].role) == 2
        {
            TransitionKind::LoaderToProgram
        } else if set.family == Some(MediaFamily::Tape)
            && role_order(pair[0].role) == 2
            && pair[1].role == MediaRole::DataMedia
        {
            TransitionKind::ProgramToData
        } else if pair[1]
            .ordinal
            .as_ref()
            .is_some_and(|o| o.unit == OrdinalUnit::Part)
        {
            TransitionKind::LoadPart
        } else {
            TransitionKind::InsertMedium
        };
        transitions.push(MediaTransition {
            from: i,
            to: i + 1,
            kind: explicit_transitions
                .get(&(pair[0].member_id.as_str(), pair[1].member_id.as_str()))
                .copied()
                .unwrap_or(kind),
            provenance: local_provenance
                .get(pair[0].member_id.as_str())
                .into_iter()
                .flatten()
                .chain(
                    local_provenance
                        .get(pair[1].member_id.as_str())
                        .into_iter()
                        .flatten(),
                )
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        });
    }
    warnings.sort();
    warnings.dedup();
    blockers.sort();
    blockers.dedup();
    MediaSwapPlan {
        release_identity: set.identity.clone(),
        platform: set.platform.clone(),
        semantics: set.family.map(|f| match f {
            MediaFamily::Optical => SwapSemantics::OpticalSequence,
            MediaFamily::Floppy => SwapSemantics::FloppySwap,
            MediaFamily::Tape => SwapSemantics::TapeLoad,
        }),
        ordered_media: steps,
        transitions,
        profile: profile.cloned(),
        warnings,
        blockers,
        confidence: set.confidence,
        provenance,
    }
}
fn rank(format: &str, profile: &MediaProfile) -> usize {
    profile
        .preferred_formats
        .iter()
        .position(|f| f == format)
        .unwrap_or(usize::MAX)
}
