//! Read-only, provider-neutral planning for alternative game launch paths.
//!
//! This is an evidence projection, not an installer or launcher.  Callers
//! provide exact identity, provider installation, version and owned-asset
//! evidence.  The planner never searches for, downloads, configures or
//! launches a provider.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlternativeLaunchKind {
    Emulator,
    SourcePort,
    EngineReimplementation,
    NativePort,
    Recompilation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlternativeLaunchIdentityConfidence {
    Exact,
    FilenameOnly,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlternativeLaunchAssetKind {
    Grp,
    Rff,
    Art,
    GameDataArchive,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AlternativeLaunchGameIdentity {
    pub canonical_id: Option<String>,
    pub edition: Option<String>,
    pub confidence: AlternativeLaunchIdentityConfidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AlternativeLaunchRequirement {
    pub asset_kind: AlternativeLaunchAssetKind,
    /// A provider/documentation-defined role, never an invented filename.
    pub role: String,
    pub edition: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AlternativeLaunchAssetEvidence {
    pub asset_kind: AlternativeLaunchAssetKind,
    pub identity: Option<String>,
    pub edition: Option<String>,
    pub present: bool,
    pub source: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AlternativeLaunchProvider {
    pub id: String,
    pub name: String,
    pub kind: AlternativeLaunchKind,
    pub version: Option<String>,
    pub version_supported: Option<bool>,
    pub installed: bool,
    pub supported_game_ids: BTreeSet<String>,
    pub requirements: Vec<AlternativeLaunchRequirement>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlternativeLaunchReadiness {
    Ready,
    ProviderMissing,
    OriginalAssetsMissing,
    IdentityInsufficient,
    VersionUnsupported,
    RequirementsUnknown,
    Ambiguous,
    UnsupportedGame,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AlternativeLaunchEvidence {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_kind: AlternativeLaunchKind,
    pub provider_version: Option<String>,
    pub game_identity: AlternativeLaunchGameIdentity,
    pub required_assets: Vec<AlternativeLaunchRequirement>,
    pub discovered_assets: Vec<AlternativeLaunchAssetEvidence>,
    pub source: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AlternativeLaunchProjection {
    pub provider: AlternativeLaunchProvider,
    pub readiness: AlternativeLaunchReadiness,
    pub reason: String,
    pub evidence: AlternativeLaunchEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AlternativeLaunchPlan {
    pub game_identity: AlternativeLaunchGameIdentity,
    pub providers: Vec<AlternativeLaunchProjection>,
    pub read_only: bool,
}

fn evidence_for(
    provider: &AlternativeLaunchProvider,
    game: &AlternativeLaunchGameIdentity,
    assets: &[AlternativeLaunchAssetEvidence],
    reason: &str,
) -> AlternativeLaunchEvidence {
    AlternativeLaunchEvidence {
        provider_id: provider.id.clone(),
        provider_name: provider.name.clone(),
        provider_kind: provider.kind,
        provider_version: provider.version.clone(),
        game_identity: game.clone(),
        required_assets: provider.requirements.clone(),
        discovered_assets: assets.to_vec(),
        source: "caller-supplied launch evidence".into(),
        reason: reason.into(),
    }
}

pub fn project_provider(
    provider: AlternativeLaunchProvider,
    game: AlternativeLaunchGameIdentity,
    assets: Vec<AlternativeLaunchAssetEvidence>,
) -> AlternativeLaunchProjection {
    let (readiness, reason): (AlternativeLaunchReadiness, String) = if game.confidence
        != AlternativeLaunchIdentityConfidence::Exact
        || game.canonical_id.is_none()
    {
        (
            AlternativeLaunchReadiness::IdentityInsufficient,
            "exact canonical game identity is required; filename-only evidence is insufficient"
                .into(),
        )
    } else if provider
        .version_supported
        .is_some_and(|supported| !supported)
    {
        (
            AlternativeLaunchReadiness::VersionUnsupported,
            "provider version is known to be unsupported".into(),
        )
    } else if !provider.installed {
        (
            AlternativeLaunchReadiness::ProviderMissing,
            "provider executable is not present; EmuWiz does not install it".into(),
        )
    } else if game
        .canonical_id
        .as_ref()
        .is_none_or(|id| !provider.supported_game_ids.contains(id))
    {
        (
            AlternativeLaunchReadiness::UnsupportedGame,
            "provider does not declare support for this exact game identity".into(),
        )
    } else if provider.version_supported != Some(true)
        || provider
            .version
            .as_ref()
            .is_none_or(|version| version.trim().is_empty())
    {
        (
            AlternativeLaunchReadiness::RequirementsUnknown,
            "a known provider version and support evidence are required".into(),
        )
    } else if provider.requirements.is_empty() {
        (
            AlternativeLaunchReadiness::RequirementsUnknown,
            "required original assets are not proven by the current provider evidence".into(),
        )
    } else if provider
        .requirements
        .iter()
        .any(|requirement| requirement.edition.is_some() && requirement.edition != game.edition)
    {
        (
            AlternativeLaunchReadiness::Ambiguous,
            "required asset evidence targets a different or unknown edition".into(),
        )
    } else if provider
        .requirements
        .iter()
        .map(|r| r.asset_kind)
        .collect::<BTreeSet<_>>()
        .len()
        != provider.requirements.len()
    {
        (AlternativeLaunchReadiness::Ambiguous,
         "multiple required roles share an asset kind; kind-only evidence cannot prove each role".into())
    } else if provider.requirements.iter().any(|requirement| {
        !assets.iter().any(|asset| {
            asset.present
                && asset.asset_kind == requirement.asset_kind
                && asset.identity.as_deref() == game.canonical_id.as_deref()
                && asset.edition == game.edition
                && (requirement.edition.is_none() || asset.edition == requirement.edition)
        })
    }) {
        (
            AlternativeLaunchReadiness::OriginalAssetsMissing,
            "one or more required original game assets are missing or unverified".into(),
        )
    } else {
        (
            AlternativeLaunchReadiness::Ready,
            "exact game identity, supported provider and required owned assets are evidenced"
                .into(),
        )
    };
    let evidence = evidence_for(&provider, &game, &assets, &reason);
    AlternativeLaunchProjection {
        provider,
        readiness,
        reason,
        evidence,
    }
}

pub fn plan_alternative_launches(
    game_identity: AlternativeLaunchGameIdentity,
    providers: Vec<AlternativeLaunchProvider>,
    assets: Vec<AlternativeLaunchAssetEvidence>,
) -> AlternativeLaunchPlan {
    let projections = providers
        .into_iter()
        .map(|provider| project_provider(provider, game_identity.clone(), assets.clone()))
        .collect();
    AlternativeLaunchPlan {
        game_identity,
        providers: projections,
        read_only: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Synthetic providers: these fixture IDs are not a built-in support list.
    fn raze_provider(
        installed: bool,
        version: Option<String>,
        version_supported: Option<bool>,
        requirements: Vec<AlternativeLaunchRequirement>,
    ) -> AlternativeLaunchProvider {
        AlternativeLaunchProvider {
            id: "synthetic".into(),
            name: "Synthetic provider".into(),
            kind: AlternativeLaunchKind::SourcePort,
            installed,
            version,
            version_supported,
            requirements,
            supported_game_ids: ["blood".into(), "duke-nukem-3d".into()]
                .into_iter()
                .collect(),
        }
    }

    fn game(id: &str) -> AlternativeLaunchGameIdentity {
        AlternativeLaunchGameIdentity {
            canonical_id: Some(id.into()),
            edition: None,
            confidence: AlternativeLaunchIdentityConfidence::Exact,
        }
    }

    fn requirement(kind: AlternativeLaunchAssetKind) -> AlternativeLaunchRequirement {
        AlternativeLaunchRequirement {
            asset_kind: kind,
            role: "provider-declared title data".into(),
            edition: None,
        }
    }

    fn asset(id: &str, kind: AlternativeLaunchAssetKind) -> AlternativeLaunchAssetEvidence {
        AlternativeLaunchAssetEvidence {
            asset_kind: kind,
            identity: Some(id.into()),
            edition: None,
            present: true,
            source: "synthetic owned-asset evidence".into(),
            reason: "synthetic exact evidence".into(),
        }
    }

    fn raze(installed: bool) -> AlternativeLaunchProvider {
        raze_provider(
            installed,
            Some("synthetic-version".into()),
            Some(true),
            vec![requirement(AlternativeLaunchAssetKind::Grp)],
        )
    }

    #[test]
    fn exact_supported_game_and_assets_are_ready() {
        let projection = project_provider(
            raze(true),
            game("duke-nukem-3d"),
            vec![asset("duke-nukem-3d", AlternativeLaunchAssetKind::Grp)],
        );
        assert_eq!(projection.readiness, AlternativeLaunchReadiness::Ready);
    }

    #[test]
    fn missing_provider_and_assets_are_blocked() {
        assert_eq!(
            project_provider(raze(false), game("blood"), vec![]).readiness,
            AlternativeLaunchReadiness::ProviderMissing
        );
        assert_eq!(
            project_provider(raze(true), game("blood"), vec![]).readiness,
            AlternativeLaunchReadiness::OriginalAssetsMissing
        );
    }

    #[test]
    fn identity_and_support_fail_closed() {
        let mut filename = game("blood");
        filename.confidence = AlternativeLaunchIdentityConfidence::FilenameOnly;
        assert_eq!(
            project_provider(raze(true), filename, vec![]).readiness,
            AlternativeLaunchReadiness::IdentityInsufficient
        );
        assert_eq!(
            project_provider(raze(true), game("unknown"), vec![]).readiness,
            AlternativeLaunchReadiness::UnsupportedGame
        );
    }

    #[test]
    fn unknown_and_ambiguous_requirements_are_explicit() {
        assert_eq!(
            project_provider(raze(true), game("blood"), vec![])
                .evidence
                .required_assets[0]
                .role,
            "provider-declared title data"
        );
        let provider = raze_provider(true, Some("v".into()), Some(true), vec![]);
        assert_eq!(
            project_provider(provider, game("blood"), vec![]).readiness,
            AlternativeLaunchReadiness::RequirementsUnknown
        );
        let mut provider = raze(true);
        provider.requirements[0].edition = Some("definitive".into());
        assert_eq!(
            project_provider(provider, game("blood"), vec![]).readiness,
            AlternativeLaunchReadiness::Ambiguous
        );
    }

    #[test]
    fn multiple_providers_coexist_and_emulator_is_not_replaced() {
        let emulator = AlternativeLaunchProvider {
            id: "dosbox".into(),
            name: "DOSBox".into(),
            kind: AlternativeLaunchKind::Emulator,
            version: None,
            version_supported: Some(true),
            installed: true,
            supported_game_ids: ["duke-nukem-3d".into()].into_iter().collect(),
            requirements: vec![requirement(AlternativeLaunchAssetKind::GameDataArchive)],
        };
        let plan = plan_alternative_launches(
            game("duke-nukem-3d"),
            vec![emulator, raze(true)],
            vec![
                asset("duke-nukem-3d", AlternativeLaunchAssetKind::Grp),
                asset("duke-nukem-3d", AlternativeLaunchAssetKind::GameDataArchive),
            ],
        );
        assert_eq!(plan.providers.len(), 2);
        assert!(
            plan.providers
                .iter()
                .any(|p| p.provider.kind == AlternativeLaunchKind::Emulator)
        );
        assert!(plan.read_only);
    }

    #[test]
    fn unknown_version_and_support_never_report_ready() {
        for missing_version in [true, false] {
            let mut provider = raze(true);
            if missing_version {
                provider.version = None;
            } else {
                provider.version_supported = None;
            }
            assert_eq!(
                project_provider(
                    provider,
                    game("blood"),
                    vec![asset("blood", AlternativeLaunchAssetKind::Grp)]
                )
                .readiness,
                AlternativeLaunchReadiness::RequirementsUnknown
            );
        }
    }
    #[test]
    fn unsupported_version_is_refused_without_side_effects() {
        let mut provider = raze(true);
        provider.version_supported = Some(false);
        let projection = project_provider(provider, game("blood"), vec![]);
        assert_eq!(
            projection.readiness,
            AlternativeLaunchReadiness::VersionUnsupported
        );
        assert_eq!(
            projection.evidence.source,
            "caller-supplied launch evidence"
        );
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;
    fn edition_fixture() -> (
        AlternativeLaunchProvider,
        AlternativeLaunchGameIdentity,
        AlternativeLaunchAssetEvidence,
    ) {
        (
            AlternativeLaunchProvider {
                id: "synthetic".into(),
                name: "Synthetic".into(),
                kind: AlternativeLaunchKind::SourcePort,
                version: Some("1".into()),
                version_supported: Some(true),
                installed: true,
                supported_game_ids: ["example".into()].into_iter().collect(),
                requirements: vec![AlternativeLaunchRequirement {
                    asset_kind: AlternativeLaunchAssetKind::GameDataArchive,
                    role: "main data".into(),
                    edition: None,
                }],
            },
            AlternativeLaunchGameIdentity {
                canonical_id: Some("example".into()),
                edition: Some("A".into()),
                confidence: AlternativeLaunchIdentityConfidence::Exact,
            },
            AlternativeLaunchAssetEvidence {
                asset_kind: AlternativeLaunchAssetKind::GameDataArchive,
                identity: Some("example".into()),
                edition: Some("A".into()),
                present: true,
                source: "synthetic observation".into(),
                reason: "fixture".into(),
            },
        )
    }
    #[test]
    fn asset_editions_must_agree_with_the_selected_game() {
        let (provider, game, asset) = edition_fixture();
        assert_eq!(
            project_provider(provider.clone(), game.clone(), vec![asset.clone()]).readiness,
            AlternativeLaunchReadiness::Ready
        );
        for edition in [None, Some("B".into())] {
            let mut other = asset.clone();
            other.edition = edition;
            assert_eq!(
                project_provider(provider.clone(), game.clone(), vec![other]).readiness,
                AlternativeLaunchReadiness::OriginalAssetsMissing
            );
        }
    }
    #[test]
    fn one_kind_cannot_prove_two_different_required_roles() {
        let (mut provider, game, asset) = edition_fixture();
        let mut second_role = provider.requirements[0].clone();
        second_role.role = "expansion data".into();
        provider.requirements.push(second_role);
        assert_eq!(
            project_provider(provider, game, vec![asset]).readiness,
            AlternativeLaunchReadiness::Ambiguous
        );
    }
}
