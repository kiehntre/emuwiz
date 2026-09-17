//! Deterministic arcade-emulator recommendation from existing evidence.
//!
//! This module is a policy projection only. It does not inspect ROMs, parse
//! DATs, discover emulators, launch processes, or change compatibility results.

use serde::Serialize;

use crate::arcade_compatibility::{
    ArcadeEmulatorCompatibility, ArcadeEmulatorCompatibilitySummary,
};
use crate::arcade_fbneo_compatibility::{FbNeoSetCompatibility, FbNeoSetCompatibilityState};
use crate::arcade_mame_compatibility::{MameSetCompatibility, MameSetCompatibilityState};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArcadeEmulator {
    FinalBurnNeo,
    Mame,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArcadeRecommendationChoice {
    FinalBurnNeo,
    Mame,
    Either,
    Neither,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArcadeRecommendationConfidence {
    Proven,
    ProvenWithWarnings,
    InsufficientEvidence,
}

/// Existing persisted preference, when one is available to the caller.
/// This type does not persist or mutate preferences itself.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArcadeUserPreference {
    FinalBurnNeo,
    Mame,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArcadeEmulatorRecommendation {
    pub preferred: ArcadeRecommendationChoice,
    pub alternatives: Vec<ArcadeEmulator>,
    pub confidence: ArcadeRecommendationConfidence,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    /// The compatible emulator that is available for immediate use, when the
    /// input summary distinguishes installed from unavailable emulators.
    pub play_now: Option<ArcadeEmulator>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArcadeRecommendationOptions {
    pub user_preference: Option<ArcadeUserPreference>,
    /// `Some(false)` records a known unavailable emulator without changing
    /// the compatibility evidence supplied to the policy.
    pub mame_available: Option<bool>,
    pub fbneo_available: Option<bool>,
}

/// Recommend from the already evaluated, independent compatibility results.
pub fn recommend_arcade_emulator(
    summary: &ArcadeEmulatorCompatibilitySummary,
    options: ArcadeRecommendationOptions,
) -> ArcadeEmulatorRecommendation {
    let mame = state_from_mame(&summary.mame);
    let fbneo = state_from_fbneo(&summary.fbneo);
    let mame_available = options.mame_available.unwrap_or(!matches!(
        summary.mame,
        ArcadeEmulatorCompatibility::NotInstalled
    ));
    let fbneo_available = options.fbneo_available.unwrap_or(!matches!(
        summary.fbneo,
        ArcadeEmulatorCompatibility::NotInstalled
    ));
    let mut result = recommend_from_evidence(mame, fbneo, options);

    result.play_now = play_now(
        result.preferred,
        mame,
        fbneo,
        mame_available,
        fbneo_available,
    );
    result
}

/// Lower-level adapter useful to callers that have the existing typed states
/// but have not assembled a complete orchestration summary yet.
pub fn recommend_from_states(
    mame: MameSetCompatibilityState,
    fbneo: FbNeoSetCompatibilityState,
    options: ArcadeRecommendationOptions,
) -> ArcadeEmulatorRecommendation {
    let mame = EvidenceState::from_mame(mame);
    let fbneo = EvidenceState::from_fbneo(fbneo);
    let mut result = recommend_from_evidence(mame, fbneo, options);
    result.play_now = play_now(
        result.preferred,
        mame,
        fbneo,
        options.mame_available.unwrap_or(true),
        options.fbneo_available.unwrap_or(true),
    );
    result
}

fn recommend_from_evidence(
    mame: EvidenceState,
    fbneo: EvidenceState,
    options: ArcadeRecommendationOptions,
) -> ArcadeEmulatorRecommendation {
    let mame_usable = mame.is_usable();
    let fbneo_usable = fbneo.is_usable();
    let warnings = warning_list(mame, fbneo);
    let confidence = if warnings.is_empty() && (mame_usable || fbneo_usable) {
        ArcadeRecommendationConfidence::Proven
    } else if mame_usable || fbneo_usable {
        ArcadeRecommendationConfidence::ProvenWithWarnings
    } else {
        ArcadeRecommendationConfidence::InsufficientEvidence
    };

    let (preferred, alternatives, mut reasons) = match (mame, fbneo) {
        (EvidenceState::Compatible { .. }, EvidenceState::Compatible { .. }) => {
            match options.user_preference {
                Some(ArcadeUserPreference::Mame) => (
                    ArcadeRecommendationChoice::Mame,
                    vec![ArcadeEmulator::FinalBurnNeo],
                    vec!["MAME was selected by the existing user preference; both emulators prove compatibility.".into()],
                ),
                Some(ArcadeUserPreference::FinalBurnNeo) => (
                    ArcadeRecommendationChoice::FinalBurnNeo,
                    vec![ArcadeEmulator::Mame],
                    vec!["FinalBurn Neo was selected by the existing user preference; both emulators prove compatibility.".into()],
                ),
                None => (
                    ArcadeRecommendationChoice::FinalBurnNeo,
                    vec![ArcadeEmulator::Mame],
                    vec!["FinalBurn Neo is the normal gameplay choice because both emulators prove compatibility.".into()],
                ),
            }
        }
        (EvidenceState::Compatible { .. }, _) => (
            ArcadeRecommendationChoice::Mame,
            Vec::new(),
            vec!["MAME is the only emulator currently proven compatible.".into()],
        ),
        (_, EvidenceState::Compatible { .. }) => (
            ArcadeRecommendationChoice::FinalBurnNeo,
            Vec::new(),
            vec!["FinalBurn Neo is the only emulator currently proven compatible.".into()],
        ),
        (EvidenceState::Incompatible, EvidenceState::Incompatible) => (
            ArcadeRecommendationChoice::Neither,
            Vec::new(),
            vec!["Neither emulator is currently proven compatible.".into()],
        ),
        _ => (
            ArcadeRecommendationChoice::Unknown,
            Vec::new(),
            vec!["Compatibility evidence is insufficient; uncertainty was not treated as incompatibility.".into()],
        ),
    };

    if matches!(preferred, ArcadeRecommendationChoice::Mame)
        && matches!(
            options.user_preference,
            Some(ArcadeUserPreference::FinalBurnNeo)
        )
    {
        reasons.push("The requested FinalBurn Neo preference was not honored because it is not proven compatible.".into());
    }
    if matches!(preferred, ArcadeRecommendationChoice::FinalBurnNeo)
        && matches!(options.user_preference, Some(ArcadeUserPreference::Mame))
    {
        reasons.push(
            "The requested MAME preference was not honored because it is not proven compatible."
                .into(),
        );
    }

    ArcadeEmulatorRecommendation {
        preferred,
        alternatives,
        confidence,
        reasons,
        warnings,
        play_now: None,
    }
}

fn play_now(
    preferred: ArcadeRecommendationChoice,
    mame: EvidenceState,
    fbneo: EvidenceState,
    mame_available: bool,
    fbneo_available: bool,
) -> Option<ArcadeEmulator> {
    let available = |emulator, state, is_available| {
        if is_available && matches!(state, EvidenceState::Compatible { .. }) {
            Some(emulator)
        } else {
            None
        }
    };
    match preferred {
        ArcadeRecommendationChoice::FinalBurnNeo => {
            available(ArcadeEmulator::FinalBurnNeo, fbneo, fbneo_available)
                .or_else(|| available(ArcadeEmulator::Mame, mame, mame_available))
        }
        ArcadeRecommendationChoice::Mame => available(ArcadeEmulator::Mame, mame, mame_available)
            .or_else(|| available(ArcadeEmulator::FinalBurnNeo, fbneo, fbneo_available)),
        ArcadeRecommendationChoice::Either => {
            available(ArcadeEmulator::FinalBurnNeo, fbneo, fbneo_available)
                .or_else(|| available(ArcadeEmulator::Mame, mame, mame_available))
        }
        ArcadeRecommendationChoice::Neither | ArcadeRecommendationChoice::Unknown => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvidenceState {
    Compatible { warnings: bool },
    Incompatible,
    Unknown,
}

impl EvidenceState {
    fn from_mame(state: MameSetCompatibilityState) -> Self {
        match state {
            MameSetCompatibilityState::Compatible => Self::Compatible { warnings: false },
            MameSetCompatibilityState::CompatibleWithWarnings => {
                Self::Compatible { warnings: true }
            }
            MameSetCompatibilityState::Incompatible | MameSetCompatibilityState::Unsupported => {
                Self::Incompatible
            }
            MameSetCompatibilityState::Unknown => Self::Unknown,
        }
    }

    fn from_fbneo(state: FbNeoSetCompatibilityState) -> Self {
        match state {
            FbNeoSetCompatibilityState::Compatible => Self::Compatible { warnings: false },
            FbNeoSetCompatibilityState::CompatibleWithWarnings => {
                Self::Compatible { warnings: true }
            }
            FbNeoSetCompatibilityState::Incompatible | FbNeoSetCompatibilityState::Unsupported => {
                Self::Incompatible
            }
            FbNeoSetCompatibilityState::Unknown => Self::Unknown,
        }
    }

    fn is_usable(self) -> bool {
        matches!(self, Self::Compatible { .. })
    }
}

fn state_from_mame(result: &ArcadeEmulatorCompatibility<MameSetCompatibility>) -> EvidenceState {
    match result {
        ArcadeEmulatorCompatibility::Evaluated(result) => EvidenceState::from_mame(result.state),
        ArcadeEmulatorCompatibility::NotInstalled | ArcadeEmulatorCompatibility::Unknown => {
            EvidenceState::Unknown
        }
    }
}

fn state_from_fbneo(result: &ArcadeEmulatorCompatibility<FbNeoSetCompatibility>) -> EvidenceState {
    match result {
        ArcadeEmulatorCompatibility::Evaluated(result) => EvidenceState::from_fbneo(result.state),
        ArcadeEmulatorCompatibility::NotInstalled | ArcadeEmulatorCompatibility::Unknown => {
            EvidenceState::Unknown
        }
    }
}

fn warning_list(mame: EvidenceState, fbneo: EvidenceState) -> Vec<String> {
    let mut warnings = Vec::new();
    if matches!(mame, EvidenceState::Compatible { warnings: true }) {
        warnings.push("MAME compatibility is proven with warnings.".into());
    }
    if matches!(fbneo, EvidenceState::Compatible { warnings: true }) {
        warnings.push("FinalBurn Neo compatibility is proven with warnings.".into());
    }
    if matches!(mame, EvidenceState::Unknown) || matches!(fbneo, EvidenceState::Unknown) {
        warnings.push("Some emulator or collection evidence is unknown; absence was not treated as missing content.".into());
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recommend(
        mame: MameSetCompatibilityState,
        fbneo: FbNeoSetCompatibilityState,
    ) -> ArcadeEmulatorRecommendation {
        recommend_from_states(mame, fbneo, ArcadeRecommendationOptions::default())
    }

    #[test]
    fn both_compatible_prefers_fbneo_with_mame_alternative() {
        let result = recommend(
            MameSetCompatibilityState::Compatible,
            FbNeoSetCompatibilityState::Compatible,
        );
        assert_eq!(result.preferred, ArcadeRecommendationChoice::FinalBurnNeo);
        assert_eq!(result.alternatives, vec![ArcadeEmulator::Mame]);
    }

    #[test]
    fn mame_wins_when_fbneo_is_unsupported_or_incompatible() {
        for fbneo in [
            FbNeoSetCompatibilityState::Unsupported,
            FbNeoSetCompatibilityState::Incompatible,
        ] {
            assert_eq!(
                recommend(MameSetCompatibilityState::Compatible, fbneo).preferred,
                ArcadeRecommendationChoice::Mame
            );
        }
    }

    #[test]
    fn fbneo_wins_when_mame_is_incompatible() {
        assert_eq!(
            recommend(
                MameSetCompatibilityState::Incompatible,
                FbNeoSetCompatibilityState::Compatible
            )
            .preferred,
            ArcadeRecommendationChoice::FinalBurnNeo
        );
    }

    #[test]
    fn unknown_never_beats_proven_compatibility() {
        assert_eq!(
            recommend(
                MameSetCompatibilityState::Unknown,
                FbNeoSetCompatibilityState::Compatible
            )
            .preferred,
            ArcadeRecommendationChoice::FinalBurnNeo
        );
        assert_eq!(
            recommend(
                MameSetCompatibilityState::Compatible,
                FbNeoSetCompatibilityState::Unknown
            )
            .preferred,
            ArcadeRecommendationChoice::Mame
        );
    }

    #[test]
    fn both_unknown_is_unknown_and_both_incompatible_is_neither() {
        assert_eq!(
            recommend(
                MameSetCompatibilityState::Unknown,
                FbNeoSetCompatibilityState::Unknown
            )
            .preferred,
            ArcadeRecommendationChoice::Unknown
        );
        assert_eq!(
            recommend(
                MameSetCompatibilityState::Incompatible,
                FbNeoSetCompatibilityState::Unsupported
            )
            .preferred,
            ArcadeRecommendationChoice::Neither
        );
    }

    #[test]
    fn preference_is_respected_only_when_valid() {
        let result = recommend_from_states(
            MameSetCompatibilityState::Compatible,
            FbNeoSetCompatibilityState::Compatible,
            ArcadeRecommendationOptions {
                user_preference: Some(ArcadeUserPreference::Mame),
                ..Default::default()
            },
        );
        assert_eq!(result.preferred, ArcadeRecommendationChoice::Mame);
        let refused = recommend_from_states(
            MameSetCompatibilityState::Incompatible,
            FbNeoSetCompatibilityState::Compatible,
            ArcadeRecommendationOptions {
                user_preference: Some(ArcadeUserPreference::Mame),
                ..Default::default()
            },
        );
        assert_eq!(refused.preferred, ArcadeRecommendationChoice::FinalBurnNeo);
        assert!(
            refused
                .reasons
                .iter()
                .any(|reason| reason.contains("not honored"))
        );
    }

    #[test]
    fn availability_is_separate_from_compatibility_preference() {
        let result = recommend_from_states(
            MameSetCompatibilityState::Compatible,
            FbNeoSetCompatibilityState::Compatible,
            ArcadeRecommendationOptions {
                fbneo_available: Some(false),
                mame_available: Some(true),
                ..Default::default()
            },
        );
        assert_eq!(result.preferred, ArcadeRecommendationChoice::FinalBurnNeo);
        assert_eq!(result.play_now, Some(ArcadeEmulator::Mame));
    }

    #[test]
    fn warning_compatible_states_remain_usable_but_are_explained() {
        let result = recommend(
            MameSetCompatibilityState::CompatibleWithWarnings,
            FbNeoSetCompatibilityState::Unknown,
        );
        assert_eq!(result.preferred, ArcadeRecommendationChoice::Mame);
        assert_eq!(
            result.confidence,
            ArcadeRecommendationConfidence::ProvenWithWarnings
        );
        assert_eq!(result.warnings.len(), 2);
    }

    #[test]
    fn explanation_order_is_deterministic() {
        let first = recommend(
            MameSetCompatibilityState::Unknown,
            FbNeoSetCompatibilityState::CompatibleWithWarnings,
        );
        let second = recommend(
            MameSetCompatibilityState::Unknown,
            FbNeoSetCompatibilityState::CompatibleWithWarnings,
        );
        assert_eq!(first, second);
        assert_eq!(
            first.warnings[0],
            "FinalBurn Neo compatibility is proven with warnings."
        );
        assert_eq!(
            first.warnings[1],
            "Some emulator or collection evidence is unknown; absence was not treated as missing content."
        );
    }
}
