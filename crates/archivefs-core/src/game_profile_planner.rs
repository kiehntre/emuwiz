//! Read-only planning for portable per-game emulator profile recommendations.
//!
//! This module classifies candidate settings and preserves their provenance.
//! It never writes an emulator configuration, chooses between conflicting
//! values, or treats a filename as proof of a game-specific profile.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameProfileSettingClass {
    PortableGameOverride,
    HostSpecific,
    PersonalPreference,
    GlobalDangerous,
    Unknown,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameProfileApplicability {
    Recommendable,
    HostBound,
    PersonalChoice,
    Dangerous,
    Unknown,
    Unsupported,
    InsufficientIdentity,
    Stale,
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameProfileIdentityConfidence {
    Exact,
    Strong,
    FilenameOnly,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameProfileHostContext {
    pub operating_system: Option<String>,
    pub gpu: Option<String>,
    pub gpu_driver: Option<String>,
    pub refresh_hz: Option<u32>,
    pub resolution: Option<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameProfileCandidate {
    pub platform: String,
    pub game_identity: Option<String>,
    pub identity_confidence: GameProfileIdentityConfidence,
    pub emulator: String,
    pub emulator_version: Option<String>,
    pub profile: Option<String>,
    pub host: GameProfileHostContext,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameProfileEvidence {
    pub provider: String,
    pub source_version: Option<String>,
    pub source_date: Option<String>,
    pub platform: Option<String>,
    pub game_identity: Option<String>,
    pub game_identity_confidence: GameProfileIdentityConfidence,
    pub emulator: String,
    pub emulator_version: Option<String>,
    pub profile: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameProfileSetting {
    pub key: String,
    pub proposed_value: String,
    pub evidence: GameProfileEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameProfileRecommendation {
    pub key: String,
    pub proposed_value: Option<String>,
    pub classification: GameProfileSettingClass,
    pub applicability: GameProfileApplicability,
    pub evidence: Vec<GameProfileEvidence>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameProfileConflict {
    pub key: String,
    pub values: Vec<String>,
    pub evidence: Vec<GameProfileEvidence>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameProfilePlan {
    pub candidate: GameProfileCandidate,
    pub recommendations: Vec<GameProfileRecommendation>,
    pub conflicts: Vec<GameProfileConflict>,
    pub read_only: bool,
}

/// Conservative initial classifier for PCSX2 settings.  The names are
/// intentionally key-oriented and accept common PCSX2 INI section prefixes;
/// unrecognized keys remain `Unknown` instead of being guessed portable.
pub fn classify_pcsx2_setting(key: &str) -> GameProfileSettingClass {
    let key = key
        .to_ascii_lowercase()
        .replace(['.', '/', '\\', '-', ' '], "");
    if key.is_empty() {
        return GameProfileSettingClass::Unknown;
    }
    if key.contains("bios")
        || key.contains("memorycard")
        || key.contains("memcard")
        || key.contains("gamepath")
        || key.contains("hddpath")
        || key.contains("folder")
        || key.contains("directory")
        || key.contains("logpath")
        || key.contains("updater")
        || key.contains("windowgeometry")
        || key.contains("uistate")
    {
        return GameProfileSettingClass::GlobalDangerous;
    }
    if key.contains("controller")
        || key.contains("hotkey")
        || key.contains("fullscreen")
        || key.contains("osd")
        || key.contains("theme")
        || key.contains("screenshot")
        || key.contains("achievement")
        || key.contains("input")
    {
        return GameProfileSettingClass::PersonalPreference;
    }
    if key.contains("adapter")
        || key.contains("renderer")
        || key.contains("extrathread")
        || key.contains("refresh")
        || key.contains("resolution")
        || key.contains("upscale")
        || key.contains("anisotropy")
        || key.contains("vsync")
    {
        return GameProfileSettingClass::HostSpecific;
    }
    if key.contains("gamefix")
        || key.contains("speedhack")
        || key.contains("cyclerate")
        || key.contains("cycleskip")
        || key.contains("vucyclesteal")
        || key.contains("mtvu")
        || key.contains("skipdraw")
        || key.contains("halfpixel")
        || key.contains("blending")
        || key.contains("mipmap")
        || key.contains("deinterlace")
        || key.contains("userhack")
        || key.contains("texturepreload")
    {
        return GameProfileSettingClass::PortableGameOverride;
    }
    GameProfileSettingClass::Unknown
}

fn applicability_for(
    candidate: &GameProfileCandidate,
    setting: &GameProfileSetting,
    class: GameProfileSettingClass,
) -> (GameProfileApplicability, String) {
    let evidence = &setting.evidence;
    if evidence.emulator != candidate.emulator {
        return (
            GameProfileApplicability::Unsupported,
            "setting belongs to a different emulator".into(),
        );
    }
    if evidence
        .platform
        .as_deref()
        .is_some_and(|platform| platform != candidate.platform)
    {
        return (
            GameProfileApplicability::Unsupported,
            "setting belongs to a different platform".into(),
        );
    }
    if evidence.profile.is_some() && evidence.profile != candidate.profile {
        return (
            GameProfileApplicability::Unsupported,
            "setting belongs to a different emulator profile".into(),
        );
    }
    if evidence.game_identity_confidence == GameProfileIdentityConfidence::FilenameOnly
        || evidence.game_identity.is_none()
        || candidate.game_identity.is_none()
        || candidate.identity_confidence != GameProfileIdentityConfidence::Exact
    {
        return (
            GameProfileApplicability::InsufficientIdentity,
            "exact game identity is required for a trusted per-game recommendation".into(),
        );
    }
    if evidence.game_identity != candidate.game_identity {
        return (
            GameProfileApplicability::Unsupported,
            "setting is bound to a different game identity".into(),
        );
    }
    if evidence.emulator_version.is_some()
        && evidence.emulator_version != candidate.emulator_version
    {
        return (
            GameProfileApplicability::Stale,
            "setting evidence targets a different or unproven emulator version".into(),
        );
    }
    match class {
        GameProfileSettingClass::PortableGameOverride => (
            GameProfileApplicability::Recommendable,
            "exact game and emulator identity support a portable candidate".into(),
        ),
        GameProfileSettingClass::HostSpecific => (
            GameProfileApplicability::HostBound,
            "value depends on host hardware or display capability".into(),
        ),
        GameProfileSettingClass::PersonalPreference => (
            GameProfileApplicability::PersonalChoice,
            "value is user preference rather than game compatibility".into(),
        ),
        GameProfileSettingClass::GlobalDangerous => (
            GameProfileApplicability::Dangerous,
            "portable recommendations must not carry global paths or live state".into(),
        ),
        GameProfileSettingClass::Unknown => (
            GameProfileApplicability::Unknown,
            "setting semantics are not proven by the current classifier".into(),
        ),
        GameProfileSettingClass::Unsupported => (
            GameProfileApplicability::Unsupported,
            "setting is outside the supported recommendation surface".into(),
        ),
    }
}

/// Classify candidate settings for one exact emulator/game context.
pub fn plan_game_profile_recommendations(
    candidate: GameProfileCandidate,
    settings: Vec<GameProfileSetting>,
) -> GameProfilePlan {
    let mut grouped: BTreeMap<String, Vec<GameProfileSetting>> = BTreeMap::new();
    for setting in settings {
        grouped
            .entry(setting.key.clone())
            .or_default()
            .push(setting);
    }
    let mut recommendations = Vec::new();
    let mut conflicts = Vec::new();
    for (key, settings) in grouped {
        let values: Vec<String> = settings
            .iter()
            .map(|setting| setting.proposed_value.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let class = if candidate.emulator.eq_ignore_ascii_case("PCSX2") {
            classify_pcsx2_setting(&key)
        } else {
            GameProfileSettingClass::Unknown
        };
        if values.len() > 1 {
            let evidence: Vec<GameProfileEvidence> = settings
                .iter()
                .map(|setting| setting.evidence.clone())
                .collect();
            conflicts.push(GameProfileConflict {
                key: key.clone(),
                values,
                evidence: evidence.clone(),
                reason: "candidate sources disagree; no value was selected".into(),
            });
            recommendations.push(GameProfileRecommendation {
                key,
                proposed_value: None,
                classification: class,
                applicability: GameProfileApplicability::Conflict,
                evidence,
                reason: "conflicting values require explicit user/provider resolution".into(),
            });
            continue;
        }
        let setting = &settings[0];
        let (applicability, reason) = applicability_for(&candidate, setting, class);
        recommendations.push(GameProfileRecommendation {
            key,
            proposed_value: Some(setting.proposed_value.clone()),
            classification: class,
            applicability,
            evidence: settings
                .into_iter()
                .map(|setting| setting.evidence)
                .collect(),
            reason,
        });
    }
    GameProfilePlan {
        candidate,
        recommendations,
        conflicts,
        read_only: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(version: Option<&str>) -> GameProfileCandidate {
        GameProfileCandidate {
            platform: "PS2".into(),
            game_identity: Some("SLUS-00001".into()),
            identity_confidence: GameProfileIdentityConfidence::Exact,
            emulator: "PCSX2".into(),
            emulator_version: version.map(str::to_string),
            profile: Some("portable".into()),
            host: GameProfileHostContext {
                operating_system: Some("linux".into()),
                gpu: Some("test-gpu".into()),
                gpu_driver: None,
                refresh_hz: Some(60),
                resolution: Some((1920, 1080)),
            },
        }
    }

    fn setting(key: &str, value: &str, version: Option<&str>) -> GameProfileSetting {
        GameProfileSetting {
            key: key.into(),
            proposed_value: value.into(),
            evidence: GameProfileEvidence {
                provider: "community-review".into(),
                source_version: Some("2026-09".into()),
                source_date: Some("2026-09-27".into()),
                platform: Some("PS2".into()),
                game_identity: Some("SLUS-00001".into()),
                game_identity_confidence: GameProfileIdentityConfidence::Exact,
                emulator: "PCSX2".into(),
                emulator_version: version.map(str::to_string),
                profile: Some("portable".into()),
                reason: "reviewed game-specific evidence".into(),
            },
        }
    }

    fn one<'a>(plan: &'a GameProfilePlan, key: &str) -> &'a GameProfileRecommendation {
        plan.recommendations
            .iter()
            .find(|item| item.key == key)
            .unwrap()
    }

    #[test]
    fn exact_gamefix_is_recommendable() {
        let plan = plan_game_profile_recommendations(
            candidate(Some("2.0")),
            vec![setting("Gamefixes.Enable", "true", Some("2.0"))],
        );
        assert_eq!(
            one(&plan, "Gamefixes.Enable").classification,
            GameProfileSettingClass::PortableGameOverride
        );
        assert_eq!(
            one(&plan, "Gamefixes.Enable").applicability,
            GameProfileApplicability::Recommendable
        );
    }

    #[test]
    fn gpu_adapter_is_host_specific() {
        let plan = plan_game_profile_recommendations(
            candidate(Some("2.0")),
            vec![setting("GSdx.Adapter", "0", Some("2.0"))],
        );
        assert_eq!(
            one(&plan, "GSdx.Adapter").applicability,
            GameProfileApplicability::HostBound
        );
    }

    #[test]
    fn bios_and_memory_card_paths_are_dangerous() {
        let plan = plan_game_profile_recommendations(
            candidate(Some("2.0")),
            vec![
                setting("Filenames.BIOS", "/user/bios.bin", Some("2.0")),
                setting("MemoryCards.Mcd001", "/user/card.ps2", Some("2.0")),
            ],
        );
        assert_eq!(
            one(&plan, "Filenames.BIOS").classification,
            GameProfileSettingClass::GlobalDangerous
        );
        assert_eq!(
            one(&plan, "MemoryCards.Mcd001").classification,
            GameProfileSettingClass::GlobalDangerous
        );
    }

    #[test]
    fn hotkey_and_controller_are_personal_preferences() {
        let plan = plan_game_profile_recommendations(
            candidate(Some("2.0")),
            vec![
                setting("Hotkeys.ToggleFullscreen", "F11", Some("2.0")),
                setting("Input.Pad1", "SDL-0", Some("2.0")),
            ],
        );
        assert_eq!(
            one(&plan, "Hotkeys.ToggleFullscreen").classification,
            GameProfileSettingClass::PersonalPreference
        );
        assert_eq!(
            one(&plan, "Input.Pad1").classification,
            GameProfileSettingClass::PersonalPreference
        );
    }

    #[test]
    fn unknown_key_is_not_promoted() {
        let plan = plan_game_profile_recommendations(
            candidate(Some("2.0")),
            vec![setting("Mystery.Option", "1", Some("2.0"))],
        );
        assert_eq!(
            one(&plan, "Mystery.Option").applicability,
            GameProfileApplicability::Unknown
        );
    }

    #[test]
    fn filename_only_identity_is_insufficient() {
        let mut item = setting("Gamefixes.Enable", "true", Some("2.0"));
        item.evidence.game_identity = Some("SLUS-00001".into());
        item.evidence.game_identity_confidence = GameProfileIdentityConfidence::FilenameOnly;
        let mut game = candidate(Some("2.0"));
        game.identity_confidence = GameProfileIdentityConfidence::FilenameOnly;
        let plan = plan_game_profile_recommendations(game, vec![item]);
        assert_eq!(
            one(&plan, "Gamefixes.Enable").applicability,
            GameProfileApplicability::InsufficientIdentity
        );
    }

    #[test]
    fn version_mismatch_is_stale() {
        let plan = plan_game_profile_recommendations(
            candidate(Some("2.0")),
            vec![setting("Gamefixes.Enable", "true", Some("1.7"))],
        );
        assert_eq!(
            one(&plan, "Gamefixes.Enable").applicability,
            GameProfileApplicability::Stale
        );
    }

    #[test]
    fn conflicting_values_are_explicit_and_unselected() {
        let mut second = setting("Gamefixes.Enable", "false", Some("2.0"));
        second.evidence.provider = "second-provider".into();
        let plan = plan_game_profile_recommendations(
            candidate(Some("2.0")),
            vec![setting("Gamefixes.Enable", "true", Some("2.0")), second],
        );
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(
            one(&plan, "Gamefixes.Enable").applicability,
            GameProfileApplicability::Conflict
        );
        assert!(one(&plan, "Gamefixes.Enable").proposed_value.is_none());
    }

    #[test]
    fn identical_values_deduplicate_with_all_provenance() {
        let mut second = setting("Gamefixes.Enable", "true", Some("2.0"));
        second.evidence.provider = "second-provider".into();
        let plan = plan_game_profile_recommendations(
            candidate(Some("2.0")),
            vec![setting("Gamefixes.Enable", "true", Some("2.0")), second],
        );
        assert!(plan.conflicts.is_empty());
        assert_eq!(one(&plan, "Gamefixes.Enable").evidence.len(), 2);
    }

    #[test]
    fn plan_has_no_apply_surface_and_is_read_only() {
        let plan = plan_game_profile_recommendations(candidate(Some("2.0")), Vec::new());
        assert!(plan.read_only);
    }
}
