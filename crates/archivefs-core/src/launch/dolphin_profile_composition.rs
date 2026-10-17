//! Read-only composition of verified Dolphin launch readiness and per-game
//! profile recommendations. No Dolphin configuration is written.

use crate::game_profile_planner::{
    GameProfileApplicability, GameProfileCandidate, GameProfileConflict, GameProfileEvidence,
    GameProfileIdentityConfidence, GameProfilePlan, GameProfileRecommendation, GameProfileSetting,
    GameProfileSettingClass,
};
use crate::launch::dolphin_command::{DolphinCommandPlan, build_dolphin_command_plan};
use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::LaunchReadiness;
use crate::patch_manager::{DolphinLaunchBlocker, DolphinNativeLaunchBinding};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DolphinProfileApplyCapability {
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DolphinProfileLaunchStatus {
    Ready,
    ReadyWithRecommendations,
    NeedsReview,
    Blocked,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinLocalProfileConflict {
    pub key: String,
    pub local_value: String,
    pub recommended_value: String,
    pub local_evidence: GameProfileEvidence,
    pub recommendation_evidence: Vec<GameProfileEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinProfileLaunchProvenance {
    pub platform: String,
    pub game_identity: String,
    pub identity_confidence: GameProfileIdentityConfidence,
    pub profile: Option<String>,
    pub profile_emulator_version: Option<String>,
    pub current_emulator_version: Option<String>,
    pub source_evidence: Vec<GameProfileEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinProfileLaunchPlan {
    pub command_plan: DolphinCommandPlan,
    pub readiness: LaunchReadiness,
    pub status: DolphinProfileLaunchStatus,
    pub apply_capability: DolphinProfileApplyCapability,
    pub portable_recommendations: Vec<GameProfileRecommendation>,
    pub host_specific_recommendations: Vec<GameProfileRecommendation>,
    pub personal_settings_excluded: Vec<GameProfileRecommendation>,
    pub dangerous_settings_excluded: Vec<GameProfileRecommendation>,
    pub other_settings: Vec<GameProfileRecommendation>,
    pub conflicts: Vec<GameProfileConflict>,
    pub local_conflicts: Vec<DolphinLocalProfileConflict>,
    pub warnings: Vec<String>,
    pub provenance: DolphinProfileLaunchProvenance,
    pub firmware: crate::launch::readiness::FirmwareReadiness,
    pub read_only: bool,
}

/// Compose the existing Dolphin command plan with exact-game profile
/// recommendations and already-read local per-game settings. The profile is
/// evidence only: this Dolphin launch path has no proven ephemeral override
/// mechanism, so no INI, user directory, or command argument is changed.
pub fn plan_dolphin_launch_with_game_profile(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<DolphinNativeLaunchBinding, DolphinLaunchBlocker>,
    profile: GameProfilePlan,
    current_dolphin_version: Option<&str>,
    local_game_settings: &[GameProfileSetting],
) -> DolphinProfileLaunchPlan {
    let command_plan = build_dolphin_command_plan(identity, candidate, binding);
    let selected_profile = match &candidate.target {
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } if *adapter_id == "dolphin" => Some(profile_id.as_str()),
        _ => None,
    };
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(value) => Some(value),
        _ => None,
    };
    let profile_candidate = &profile.candidate;
    let exact_binding = resolved.is_some_and(|resolved| {
        selected_profile.is_some()
            && profile_candidate.emulator.eq_ignore_ascii_case("Dolphin")
            && profile_candidate.platform == resolved.platform_id
            && profile_candidate.game_identity.as_deref() == Some(resolved.game_key.as_str())
            && profile_candidate.identity_confidence == GameProfileIdentityConfidence::Exact
            && profile_candidate.profile.as_deref() == selected_profile
    });
    let version_unknown = current_dolphin_version.is_none_or(str::is_empty);
    let version_stale = current_dolphin_version != profile_candidate.emulator_version.as_deref()
        && profile_candidate.emulator_version.is_some();
    let mut warnings = Vec::new();
    warnings.extend(
        candidate
            .warnings
            .iter()
            .map(|warning| format!("{:?}: {}", warning.kind, warning.detail)),
    );
    if !exact_binding {
        warnings.push("Dolphin profile is not bound to the exact resolved game, platform, and selected profile".into());
    }
    if version_unknown {
        warnings.push("Installed Dolphin version is unknown; setting support is unproven".into());
    }
    if version_stale {
        warnings.push("Dolphin profile version evidence is stale or differs from the selected emulator version".into());
    }
    let recommendation_stale = profile
        .recommendations
        .iter()
        .any(|recommendation| recommendation.applicability == GameProfileApplicability::Stale);
    if recommendation_stale && !version_stale {
        warnings
            .push("one or more Dolphin profile recommendations have stale version evidence".into());
    }
    let profile_needs_review = profile.recommendations.iter().any(|recommendation| {
        matches!(
            recommendation.applicability,
            GameProfileApplicability::Unknown
                | GameProfileApplicability::Unsupported
                | GameProfileApplicability::InsufficientIdentity
                | GameProfileApplicability::Conflict
        )
    });
    if profile_needs_review {
        warnings.push(
            "one or more profile settings have mismatched identity or conflicting evidence".into(),
        );
    }

    let mut portable_recommendations = Vec::new();
    let mut host_specific_recommendations = Vec::new();
    let mut personal_settings_excluded = Vec::new();
    let mut dangerous_settings_excluded = Vec::new();
    let mut other_settings = Vec::new();
    let conflicts = profile.conflicts.clone();
    let recommendations = &profile.recommendations;
    for recommendation in recommendations {
        match recommendation.classification {
            GameProfileSettingClass::PortableGameOverride => {
                if exact_binding
                    && !version_unknown
                    && !version_stale
                    && !recommendation_stale
                    && recommendation.applicability == GameProfileApplicability::Recommendable
                {
                    portable_recommendations.push(recommendation.clone());
                } else {
                    other_settings.push(recommendation.clone());
                }
            }
            GameProfileSettingClass::HostSpecific => {
                host_specific_recommendations.push(recommendation.clone());
            }
            GameProfileSettingClass::PersonalPreference => {
                personal_settings_excluded.push(recommendation.clone());
            }
            GameProfileSettingClass::GlobalDangerous => {
                dangerous_settings_excluded.push(recommendation.clone());
            }
            GameProfileSettingClass::Unknown | GameProfileSettingClass::Unsupported => {
                other_settings.push(recommendation.clone());
            }
        }
    }

    let mut local_conflicts = Vec::new();
    if exact_binding && !version_stale && !recommendation_stale {
        for recommendation in &portable_recommendations {
            let Some(recommended_value) = recommendation.proposed_value.as_deref() else {
                continue;
            };
            for local in local_game_settings
                .iter()
                .filter(|setting| setting.key == recommendation.key)
            {
                if local.proposed_value != recommended_value {
                    local_conflicts.push(DolphinLocalProfileConflict {
                        key: recommendation.key.clone(),
                        local_value: local.proposed_value.clone(),
                        recommended_value: recommended_value.into(),
                        local_evidence: local.evidence.clone(),
                        recommendation_evidence: recommendation.evidence.clone(),
                    });
                }
            }
        }
    }
    for conflict in &local_conflicts {
        warnings.push(format!(
            "{} conflicts with an existing local Dolphin game setting",
            conflict.key
        ));
    }
    if !portable_recommendations.is_empty() {
        warnings.push("portable Dolphin profile recommendations are retained as read-only evidence; per-launch application is unavailable".into());
    }
    if !host_specific_recommendations.is_empty() {
        warnings.push(
            "host-specific Dolphin settings need local selection and were not imported".into(),
        );
    }

    let readiness = if command_plan.command.is_none() {
        LaunchReadiness::Blocked
    } else if !warnings.is_empty() || !candidate.warnings.is_empty() {
        LaunchReadiness::ReadyWithWarnings
    } else {
        candidate.readiness
    };
    let status = if readiness == LaunchReadiness::Blocked {
        DolphinProfileLaunchStatus::Blocked
    } else if !exact_binding
        || version_unknown
        || version_stale
        || recommendation_stale
        || profile_needs_review
        || !host_specific_recommendations.is_empty()
        || !conflicts.is_empty()
        || !local_conflicts.is_empty()
    {
        DolphinProfileLaunchStatus::NeedsReview
    } else if !portable_recommendations.is_empty() {
        DolphinProfileLaunchStatus::ReadyWithRecommendations
    } else if selected_profile.is_none() {
        DolphinProfileLaunchStatus::Unsupported
    } else {
        DolphinProfileLaunchStatus::Ready
    };
    let mut source_evidence = recommendations
        .iter()
        .flat_map(|recommendation| recommendation.evidence.iter().cloned())
        .collect::<Vec<_>>();
    for conflict in &conflicts {
        source_evidence.extend(conflict.evidence.iter().cloned());
    }
    for conflict in &local_conflicts {
        source_evidence.push(conflict.local_evidence.clone());
        source_evidence.extend(conflict.recommendation_evidence.iter().cloned());
    }
    let provenance = DolphinProfileLaunchProvenance {
        platform: profile_candidate.platform.clone(),
        game_identity: profile_candidate.game_identity.clone().unwrap_or_default(),
        identity_confidence: profile_candidate.identity_confidence,
        profile: profile_candidate.profile.clone(),
        profile_emulator_version: profile_candidate.emulator_version.clone(),
        current_emulator_version: current_dolphin_version.map(str::to_string),
        source_evidence,
    };
    DolphinProfileLaunchPlan {
        command_plan,
        readiness,
        status,
        apply_capability: DolphinProfileApplyCapability::Unavailable,
        portable_recommendations,
        host_specific_recommendations,
        personal_settings_excluded,
        dangerous_settings_excluded,
        other_settings,
        conflicts,
        local_conflicts,
        warnings,
        provenance,
        firmware: candidate.firmware,
        read_only: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_profile_planner::{
        GameProfileEvidence, GameProfileIdentityConfidence, GameProfilePlan, GameProfileSetting,
    };
    use crate::launch::planning::{
        CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
        ResolvedIdentity,
    };
    use crate::launch::readiness::{FirmwareReadiness, LaunchBlocker, LaunchBlockerKind};
    use crate::patch_manager::{DolphinLaunchBlocker, DolphinUserDirectoryMode};

    fn candidate(platform: &str, game_id: &str) -> (CanonicalIdentityStatus, LaunchCandidate) {
        (
            CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: platform.into(),
                game_key: game_id.into(),
            }),
            LaunchCandidate {
                target: LaunchTarget::Standalone {
                    adapter_id: "dolphin",
                    profile_id: "dolphin-native".into(),
                    profile_path: None,
                },
                content: LaunchContentRef {
                    kind: Some(LaunchContentKind::OpticalDisc),
                    container: Some(LaunchContainerKind::PlainFile),
                    resolved_path: Some("/games/game.iso".into()),
                    requires_mount: false,
                    provenance: "verified fixture".into(),
                },
                firmware: FirmwareReadiness::NotRequired,
                blockers: Vec::new(),
                warnings: Vec::new(),
                readiness: LaunchReadiness::Ready,
                preference: CandidatePreference::SoleEligible,
            },
        )
    }

    fn binding() -> Result<DolphinNativeLaunchBinding, DolphinLaunchBlocker> {
        Ok(DolphinNativeLaunchBinding {
            executable: "/usr/bin/dolphin-emu".into(),
            installation: crate::launch::installation::LaunchInstallation::Native,
            user_directory_mode: DolphinUserDirectoryMode::DefaultNative,
        })
    }

    fn setting(
        key: &str,
        value: &str,
        platform: &str,
        game_id: &str,
        version: Option<&str>,
    ) -> GameProfileSetting {
        GameProfileSetting {
            key: key.into(),
            proposed_value: value.into(),
            evidence: GameProfileEvidence {
                provider: "fixture profile".into(),
                source_version: Some("1".into()),
                source_date: Some("2026-09-01".into()),
                platform: Some(platform.into()),
                game_identity: Some(game_id.into()),
                game_identity_confidence: GameProfileIdentityConfidence::Exact,
                emulator: "Dolphin".into(),
                emulator_version: Some(version.unwrap_or("5.0").to_string()),
                profile: Some("dolphin-native".into()),
                reason: "verified fixture evidence".into(),
            },
        }
    }

    fn profile_plan(
        platform: &str,
        game_id: &str,
        settings: Vec<GameProfileSetting>,
    ) -> GameProfilePlan {
        crate::game_profile_planner::plan_game_profile_recommendations(
            dolphin_game_profile_candidate(platform, game_id, "dolphin-native", Some("5.0".into())),
            settings,
        )
    }

    fn compose(
        platform: &str,
        game_id: &str,
        settings: Vec<GameProfileSetting>,
        local: &[GameProfileSetting],
    ) -> DolphinProfileLaunchPlan {
        let (identity, candidate) = candidate(platform, game_id);
        plan_dolphin_launch_with_game_profile(
            &identity,
            &candidate,
            &binding(),
            profile_plan(platform, game_id, settings),
            Some("5.0"),
            local,
        )
    }

    #[test]
    fn exact_gamecube_and_wii_profiles_retain_portable_recommendations() {
        for (platform, id) in [("GameCube", "GALE01"), ("Wii", "RMCE01")] {
            let plan = compose(
                platform,
                id,
                vec![setting("MMU", "1", platform, id, None)],
                &[],
            );
            assert_eq!(
                plan.status,
                DolphinProfileLaunchStatus::ReadyWithRecommendations
            );
            assert_eq!(plan.portable_recommendations[0].key, "MMU");
            assert_eq!(
                plan.apply_capability,
                DolphinProfileApplyCapability::Unavailable
            );
            assert_eq!(
                plan.command_plan.command.as_ref().unwrap().arguments.len(),
                2
            );
        }
    }

    #[test]
    fn wrong_game_region_or_platform_never_receives_portable_profile_values() {
        let plan = compose(
            "Wii",
            "RMCE01",
            vec![setting("MMU", "1", "GameCube", "GALE01", None)],
            &[],
        );
        assert!(plan.portable_recommendations.is_empty());
        assert_eq!(plan.status, DolphinProfileLaunchStatus::NeedsReview);
    }

    #[test]
    fn host_personal_dangerous_and_unknown_values_stay_separate() {
        let platform = "GameCube";
        let id = "GALE01";
        let plan = compose(
            platform,
            id,
            vec![
                setting("Backend", "Vulkan", platform, id, None),
                setting("InternalResolution", "3x", platform, id, None),
                setting("Fullscreen", "True", platform, id, None),
                setting("ControllerProfile", "custom", platform, id, None),
                setting("NANDPath", "/private/nand", platform, id, None),
                setting("MemoryCardPath", "/private/card.raw", platform, id, None),
                setting("NetworkCredential", "secret", platform, id, None),
                setting("FutureSetting", "x", platform, id, None),
            ],
            &[],
        );
        assert_eq!(plan.host_specific_recommendations.len(), 2);
        assert_eq!(plan.personal_settings_excluded.len(), 2);
        assert_eq!(plan.dangerous_settings_excluded.len(), 3);
        assert_eq!(plan.other_settings.len(), 1);
        assert!(plan.portable_recommendations.is_empty());
        assert_eq!(plan.status, DolphinProfileLaunchStatus::NeedsReview);
    }

    #[test]
    fn local_game_setting_conflict_retains_both_values() {
        let platform = "GameCube";
        let id = "GALE01";
        let incoming = setting("MMU", "1", platform, id, None);
        let local = setting("MMU", "0", platform, id, None);
        let plan = compose(platform, id, vec![incoming], &[local]);
        assert_eq!(plan.local_conflicts.len(), 1);
        assert_eq!(plan.local_conflicts[0].local_value, "0");
        assert_eq!(plan.local_conflicts[0].recommended_value, "1");
        assert_eq!(plan.status, DolphinProfileLaunchStatus::NeedsReview);
    }

    #[test]
    fn stale_version_identity_and_firmware_blockers_remain_independent() {
        let platform = "Wii";
        let id = "RMCE01";
        let stale = setting("MMU", "1", platform, id, Some("4.0"));
        let plan = compose(platform, id, vec![stale], &[]);
        assert!(plan.portable_recommendations.is_empty());
        assert_eq!(plan.status, DolphinProfileLaunchStatus::NeedsReview);

        let (identity, mut candidate) = candidate(platform, id);
        candidate.firmware = FirmwareReadiness::Missing;
        candidate.blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::RequiredFirmwareMissing,
            "fixture missing firmware blocker",
        ));
        let plan = plan_dolphin_launch_with_game_profile(
            &identity,
            &candidate,
            &binding(),
            profile_plan(platform, id, vec![setting("MMU", "1", platform, id, None)]),
            Some("5.0"),
            &[],
        );
        assert_eq!(plan.readiness, LaunchReadiness::Blocked);
        assert_eq!(plan.status, DolphinProfileLaunchStatus::Blocked);
        assert_eq!(plan.firmware, FirmwareReadiness::Missing);
    }

    #[test]
    fn filename_only_identity_is_untrusted_and_no_profile_config_is_written() {
        let platform = "GameCube";
        let id = "GALE01";
        let mut profile = profile_plan(platform, id, vec![setting("MMU", "1", platform, id, None)]);
        profile.candidate.identity_confidence = GameProfileIdentityConfidence::FilenameOnly;
        let (identity, candidate) = candidate(platform, id);
        let before_args = build_dolphin_command_plan(&identity, &candidate, &binding())
            .command
            .unwrap()
            .arguments;
        let plan = plan_dolphin_launch_with_game_profile(
            &identity,
            &candidate,
            &binding(),
            profile,
            Some("5.0"),
            &[],
        );
        assert!(plan.portable_recommendations.is_empty());
        assert_eq!(
            plan.apply_capability,
            DolphinProfileApplyCapability::Unavailable
        );
        assert_eq!(
            plan.command_plan.command.unwrap().arguments,
            before_args,
            "profile composition must not change argv"
        );
    }
}

/// Convenience for exact current Dolphin context; identities and version are
/// still supplied by the caller and are never inferred from filenames.
pub fn dolphin_game_profile_candidate(
    platform: &str,
    game_identity: &str,
    profile_id: &str,
    dolphin_version: Option<String>,
) -> GameProfileCandidate {
    GameProfileCandidate {
        platform: platform.into(),
        game_identity: Some(game_identity.into()),
        identity_confidence: GameProfileIdentityConfidence::Exact,
        emulator: "Dolphin".into(),
        emulator_version: dolphin_version,
        profile: Some(profile_id.into()),
        host: crate::game_profile_planner::GameProfileHostContext {
            operating_system: None,
            gpu: None,
            gpu_driver: None,
            refresh_hz: None,
            resolution: None,
        },
    }
}

mod inspection;
pub use inspection::{DolphinGuidanceInspection, inspect_dolphin_profile_guidance};
