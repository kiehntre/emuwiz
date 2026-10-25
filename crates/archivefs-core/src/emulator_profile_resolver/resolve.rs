//! The generic resolver: ranks and selects among the candidates an adapter
//! reports. It owns selection; adapters own facts. Nothing here writes.

use std::path::PathBuf;

use super::EmulatorProfileAdapter;
use super::model::*;

/// Resolves which installation + profile an emulator should use *now*.
///
/// Reads the disk afresh on every call - there is no cache, so calling it
/// again after anything changed is the refresh.
pub fn resolve_emulator_profile(
    adapter: &dyn EmulatorProfileAdapter,
    mode: &EmulatorSelectionMode,
) -> ResolutionOutcome {
    let mut discovered = adapter.discover();
    discovered.sort_by(|left, right| left.identity.cmp(&right.identity));
    discovered.dedup_by(|left, right| left.identity == right.identity);

    match mode {
        EmulatorSelectionMode::Forced(pinned) => resolve_forced(adapter, discovered, pinned),
        EmulatorSelectionMode::Preferred(pinned) => resolve_preferred(adapter, discovered, pinned),
        EmulatorSelectionMode::Auto => match select_automatically(&discovered) {
            Selection::Chosen {
                index,
                reason,
                equivalents,
            } => {
                let mut warnings = Vec::new();
                if !equivalents.is_empty() {
                    warnings.push(ResolutionWarning::EquivalentExecutables {
                        others: equivalents,
                    });
                }
                finish(
                    discovered,
                    index,
                    SelectionModeKind::Auto,
                    reason,
                    PreferenceState::NoPreference,
                    warnings,
                )
            }
            Selection::Ambiguous(tied) => ambiguous(&discovered, &tied),
            Selection::Nothing => nothing_usable(&discovered),
        },
    }
}

/// Re-resolves and reports whether the answer differs from `previous`.
pub fn refresh_emulator_profile(
    adapter: &dyn EmulatorProfileAdapter,
    mode: &EmulatorSelectionMode,
    previous: &ResolutionOutcome,
) -> (ResolutionOutcome, bool) {
    let current = resolve_emulator_profile(adapter, mode);
    let changed = &current != previous;
    (current, changed)
}

enum Selection {
    Chosen {
        index: usize,
        reason: SelectionReason,
        equivalents: Vec<PathBuf>,
    },
    Ambiguous(Vec<usize>),
    Nothing,
}

fn select_automatically(candidates: &[EmulatorProfileCandidate]) -> Selection {
    let usable: Vec<usize> = (0..candidates.len())
        .filter(|&index| candidates[index].is_usable())
        .collect();
    match usable.as_slice() {
        [] => return Selection::Nothing,
        [only] => {
            return Selection::Chosen {
                index: *only,
                reason: SelectionReason::OnlyViableCandidate,
                equivalents: Vec::new(),
            };
        }
        _ => {}
    }
    let ranks: Vec<CandidateRank> = candidates.iter().map(CandidateRank::of).collect();
    let top = usable
        .iter()
        .map(|&index| ranks[index])
        .max()
        .expect("non-empty");
    let leaders: Vec<usize> = usable
        .iter()
        .copied()
        .filter(|&index| ranks[index] == top)
        .collect();
    if let [leader] = leaders.as_slice() {
        let runner_up = usable
            .iter()
            .copied()
            .filter(|&index| index != *leader)
            .map(|index| ranks[index])
            .max()
            .expect("at least two usable");
        let factor = top
            .first_difference(&runner_up)
            .expect("a unique leader differs from the runner-up");
        return Selection::Chosen {
            index: *leader,
            reason: SelectionReason::StrongestEvidence(factor),
            equivalents: Vec::new(),
        };
    }
    // Tied leaders. Several executables of one profile are the same answer
    // for anything that needs the profile, so that is not a question to ask.
    let first = &candidates[leaders[0]];
    let same_profile = leaders.iter().all(|&index| {
        let candidate = &candidates[index];
        candidate.identity.profile_root == first.identity.profile_root
            && std::mem::discriminant(&candidate.installation)
                == std::mem::discriminant(&first.installation)
    });
    if same_profile {
        let mut ordered = leaders.clone();
        ordered.sort_by(|&a, &b| {
            candidates[a]
                .identity
                .executable
                .cmp(&candidates[b].identity.executable)
        });
        let chosen = ordered[0];
        let equivalents = ordered[1..]
            .iter()
            .filter_map(|&index| candidates[index].identity.executable.clone())
            .collect();
        return Selection::Chosen {
            index: chosen,
            reason: SelectionReason::EquivalentExecutableOfSameProfile,
            equivalents,
        };
    }
    Selection::Ambiguous(leaders)
}

fn resolve_forced(
    adapter: &dyn EmulatorProfileAdapter,
    mut discovered: Vec<EmulatorProfileCandidate>,
    pinned: &CandidateIdentity,
) -> ResolutionOutcome {
    // Exactly the pinned pair is assessed - never a neighbour.
    let candidate = adapter.assess(pinned);
    if !candidate.is_usable() {
        return ResolutionOutcome::Refused(forced_refusal(pinned, &candidate.unusable));
    }
    discovered.retain(|other| other.identity != *pinned);
    discovered.push(candidate);
    let index = discovered.len() - 1;
    finish(
        discovered,
        index,
        SelectionModeKind::Forced,
        SelectionReason::ForcedByUser,
        PreferenceState::Forced,
        Vec::new(),
    )
}

fn forced_refusal(pinned: &CandidateIdentity, reasons: &[UnusableReason]) -> ForcedRefusal {
    let missing_only = reasons.iter().all(|reason| {
        matches!(
            reason,
            UnusableReason::ExecutableMissing
                | UnusableReason::FlatpakNotInstalled
                | UnusableReason::ProfileDirectoryMissing
                | UnusableReason::MainConfigMissing
        )
    });
    let reasons = reasons.to_vec();
    if missing_only {
        ForcedRefusal::ForcedProfileUnavailable {
            pinned: pinned.clone(),
            reasons,
        }
    } else {
        ForcedRefusal::ForcedProfileInvalid {
            pinned: pinned.clone(),
            reasons,
        }
    }
}

fn resolve_preferred(
    adapter: &dyn EmulatorProfileAdapter,
    mut discovered: Vec<EmulatorProfileCandidate>,
    pinned: &CandidateIdentity,
) -> ResolutionOutcome {
    let present = discovered
        .iter()
        .position(|candidate| candidate.identity == *pinned);
    let preferred = match present {
        Some(index) => Some(index),
        None => {
            // Not found by discovery (e.g. a custom location): assess exactly it.
            let assessed = adapter.assess(pinned);
            if assessed.is_usable() {
                discovered.push(assessed);
                Some(discovered.len() - 1)
            } else {
                None
            }
        }
    };
    if let Some(index) = preferred
        && discovered[index].is_usable()
    {
        return finish(
            discovered,
            index,
            SelectionModeKind::Preferred,
            SelectionReason::PreferredAndUsable,
            PreferenceState::PreferredHonoured,
            Vec::new(),
        );
    }
    let why = match present {
        Some(index) => PreferenceBypassReason::Unusable(discovered[index].unusable.clone()),
        None => PreferenceBypassReason::NoLongerPresent,
    };
    match select_automatically(&discovered) {
        Selection::Chosen {
            index,
            reason,
            equivalents,
        } => {
            let mut warnings = vec![ResolutionWarning::PreferredProfileBypassed {
                preferred: pinned.clone(),
                why: why.clone(),
            }];
            if !equivalents.is_empty() {
                warnings.push(ResolutionWarning::EquivalentExecutables {
                    others: equivalents,
                });
            }
            finish(
                discovered,
                index,
                SelectionModeKind::Preferred,
                reason,
                PreferenceState::PreferredBypassed(why),
                warnings,
            )
        }
        Selection::Ambiguous(tied) => ambiguous(&discovered, &tied),
        Selection::Nothing => nothing_usable(&discovered),
    }
}

fn nothing_usable(candidates: &[EmulatorProfileCandidate]) -> ResolutionOutcome {
    let assessments = assess_all(candidates, None);
    let any_installed = candidates.iter().any(|candidate| {
        !candidate.unusable.iter().any(|reason| {
            matches!(
                reason,
                UnusableReason::ExecutableMissing
                    | UnusableReason::FlatpakNotInstalled
                    | UnusableReason::ExecutableNotAbsolute
            )
        })
    });
    if any_installed {
        ResolutionOutcome::NoUsableProfile { assessments }
    } else {
        ResolutionOutcome::NoInstallation { assessments }
    }
}

fn ambiguous(candidates: &[EmulatorProfileCandidate], tied: &[usize]) -> ResolutionOutcome {
    let assessments = assess_all(candidates, None)
        .into_iter()
        .enumerate()
        .map(|(index, mut assessment)| {
            if tied.contains(&index) {
                assessment.standing = Standing::Tied;
            }
            assessment
        })
        .collect::<Vec<_>>();
    let choices = tied
        .iter()
        .map(|&index| assessments[index].clone())
        .collect();
    ResolutionOutcome::Ambiguous {
        choices,
        assessments,
    }
}

fn assess_all(
    candidates: &[EmulatorProfileCandidate],
    selected: Option<usize>,
) -> Vec<CandidateAssessment> {
    let selected_rank = selected.map(|index| CandidateRank::of(&candidates[index]));
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let rank = CandidateRank::of(candidate);
            let standing = if Some(index) == selected {
                Standing::Selected
            } else if !candidate.is_usable() {
                Standing::Unusable(candidate.unusable.clone())
            } else if let Some(selected_rank) = selected_rank {
                if rank > selected_rank {
                    let factor = rank
                        .first_difference(&selected_rank)
                        .expect("different ranks differ somewhere");
                    Standing::NotPreferred {
                        stronger_on: factor,
                    }
                } else {
                    match selected_rank.first_difference(&rank) {
                        Some(lost_on) => Standing::Alternative { lost_on },
                        None => Standing::Tied,
                    }
                }
            } else {
                Standing::Tied
            };
            CandidateAssessment {
                identity: candidate.identity.clone(),
                layout: candidate.layout,
                standing,
                rank,
                evidence: candidate.evidence.clone(),
            }
        })
        .collect()
}

fn finish(
    candidates: Vec<EmulatorProfileCandidate>,
    index: usize,
    mode: SelectionModeKind,
    reason: SelectionReason,
    preference: PreferenceState,
    mut warnings: Vec<ResolutionWarning>,
) -> ResolutionOutcome {
    let assessments = assess_all(&candidates, Some(index));
    let chosen = &candidates[index];
    warnings.extend(
        chosen
            .warnings
            .iter()
            .cloned()
            .map(ResolutionWarning::Configuration),
    );
    let executable = chosen
        .identity
        .executable
        .clone()
        .expect("a usable candidate has an executable");
    ResolutionOutcome::Resolved(Box::new(Resolution {
        profile: ResolvedEmulatorProfile {
            emulator: chosen.identity.emulator,
            identity: chosen.identity.clone(),
            executable,
            installation: chosen.installation.clone(),
            layout: chosen.layout,
            profile_root: chosen.identity.profile_root.clone(),
            main_config: chosen.main_config.clone(),
            folders: chosen.folders.clone(),
            readiness: chosen.readiness.clone(),
            warnings,
            evidence: chosen.evidence.clone(),
            mode,
            reason,
            preference,
            details: chosen.details.clone(),
        },
        assessments,
    }))
}
