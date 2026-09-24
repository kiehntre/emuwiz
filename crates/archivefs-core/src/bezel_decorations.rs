//! Identity-driven, local-first bezel/decorations resolution.
//!
//! This module only resolves and previews decorations.  It does not download
//! assets, edit emulator configuration, or touch source media.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DecorationScope {
    Game,
    System,
    Default,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DecorationSource {
    LocalPack { path: String },
    UserOverride { path: String },
    Provider { name: String, reference: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecorationProvenance {
    pub provider: String,
    pub reference: String,
    pub retrieved_at_unix_seconds: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecorationTarget {
    pub emulator: String,
    pub core: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DecorationReadiness {
    Ready,
    Unsupported { reason: String },
    MissingConfiguration { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DecorationEvidence {
    VerifiedIdentity { identity: String },
    CanonicalDatIdentity { identity: String },
    PlatformIdentity { platform: String },
    ExplicitUserMapping { mapping: String },
    FilenameHint { value: String },
}

impl DecorationEvidence {
    fn strength(&self) -> u8 {
        match self {
            Self::VerifiedIdentity { .. } => 5,
            Self::CanonicalDatIdentity { .. } => 4,
            Self::ExplicitUserMapping { .. } => 3,
            Self::PlatformIdentity { .. } => 2,
            Self::FilenameHint { .. } => 1,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecorationAsset {
    pub id: String,
    pub scope: DecorationScope,
    pub source: DecorationSource,
    pub evidence: DecorationEvidence,
    pub targets: Vec<DecorationTarget>,
    pub readiness: DecorationReadiness,
    pub provenance: DecorationProvenance,
    pub viewport: Option<ViewportMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ViewportMetadata {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecorationResolution {
    pub selected: Option<DecorationAsset>,
    pub candidates: Vec<DecorationAsset>,
    pub reason: String,
    pub conflicts: Vec<String>,
    pub target: DecorationTarget,
}

pub fn resolve_decoration(
    mut candidates: Vec<DecorationAsset>,
    target: DecorationTarget,
) -> DecorationResolution {
    candidates.retain(|asset| {
        (asset.targets.is_empty()
            || asset.targets.iter().any(|candidate| {
                candidate.emulator == target.emulator
                    && (candidate.core.is_none() || candidate.core == target.core)
            }))
            && matches!(asset.readiness, DecorationReadiness::Ready)
    });
    candidates.sort_by(|left, right| {
        right
            .source_rank()
            .cmp(&left.source_rank())
            .then_with(|| right.scope_rank().cmp(&left.scope_rank()))
            .then_with(|| right.evidence.strength().cmp(&left.evidence.strength()))
            .then_with(|| left.id.cmp(&right.id))
    });
    let conflicts = candidates
        .windows(2)
        .filter(|pair| {
            pair[0].evidence.strength() == pair[1].evidence.strength()
                && pair[0].scope == pair[1].scope
        })
        .map(|pair| format!("{} conflicts with {}", pair[0].id, pair[1].id))
        .collect::<Vec<_>>();
    let selected = candidates.first().cloned();
    let reason = selected
        .as_ref()
        .map(|asset| format!("selected {} using explicit evidence", asset.id))
        .unwrap_or_else(|| "no compatible local decoration was found".into());
    DecorationResolution {
        selected,
        candidates,
        reason,
        conflicts,
        target,
    }
}

impl DecorationAsset {
    fn source_rank(&self) -> u8 {
        match self.source {
            DecorationSource::UserOverride { .. } => 3,
            DecorationSource::LocalPack { .. } => 2,
            DecorationSource::Provider { .. } => 1,
        }
    }

    fn scope_rank(&self) -> u8 {
        match self.scope {
            DecorationScope::Game => 3,
            DecorationScope::System => 2,
            DecorationScope::Default => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, scope: DecorationScope, evidence: DecorationEvidence) -> DecorationAsset {
        DecorationAsset {
            id: id.into(),
            scope,
            source: DecorationSource::LocalPack { path: id.into() },
            evidence,
            targets: vec![DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            }],
            readiness: DecorationReadiness::Ready,
            provenance: DecorationProvenance {
                provider: "local-test".into(),
                reference: id.into(),
                retrieved_at_unix_seconds: None,
            },
            viewport: None,
        }
    }

    #[test]
    fn verified_identity_beats_filename_guess() {
        let result = resolve_decoration(
            vec![
                asset(
                    "guess",
                    DecorationScope::Game,
                    DecorationEvidence::FilenameHint {
                        value: "game".into(),
                    },
                ),
                asset(
                    "verified",
                    DecorationScope::Game,
                    DecorationEvidence::VerifiedIdentity {
                        identity: "game-id".into(),
                    },
                ),
            ],
            DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            },
        );
        assert_eq!(result.selected.unwrap().id, "verified");
    }

    #[test]
    fn game_beats_system_and_default_fallbacks() {
        let mut candidates = vec![
            asset(
                "default",
                DecorationScope::Default,
                DecorationEvidence::PlatformIdentity {
                    platform: "snes".into(),
                },
            ),
            asset(
                "system",
                DecorationScope::System,
                DecorationEvidence::PlatformIdentity {
                    platform: "snes".into(),
                },
            ),
            asset(
                "game",
                DecorationScope::Game,
                DecorationEvidence::ExplicitUserMapping {
                    mapping: "game".into(),
                },
            ),
        ];
        for candidate in &mut candidates {
            candidate.targets[0].emulator = "dolphin".into();
        }
        let result = resolve_decoration(
            candidates,
            DecorationTarget {
                emulator: "dolphin".into(),
                core: None,
            },
        );
        assert_eq!(result.selected.unwrap().id, "game");
    }

    #[test]
    fn incompatible_emulator_is_not_selected() {
        let result = resolve_decoration(
            vec![asset(
                "dolphin",
                DecorationScope::Game,
                DecorationEvidence::VerifiedIdentity {
                    identity: "id".into(),
                },
            )],
            DecorationTarget {
                emulator: "dolphin".into(),
                core: None,
            },
        );
        assert!(result.selected.is_none());
    }

    #[test]
    fn user_override_wins_over_provider_default() {
        let mut override_asset = asset(
            "override",
            DecorationScope::Game,
            DecorationEvidence::ExplicitUserMapping {
                mapping: "manual".into(),
            },
        );
        override_asset.source = DecorationSource::UserOverride {
            path: "override.png".into(),
        };
        let result = resolve_decoration(
            vec![
                asset(
                    "provider",
                    DecorationScope::Default,
                    DecorationEvidence::VerifiedIdentity {
                        identity: "id".into(),
                    },
                ),
                override_asset,
            ],
            DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            },
        );
        assert_eq!(result.selected.unwrap().id, "override");
    }

    #[test]
    fn unsupported_target_remains_visible_as_no_ready_selection() {
        let mut candidate = asset(
            "future",
            DecorationScope::Game,
            DecorationEvidence::VerifiedIdentity {
                identity: "id".into(),
            },
        );
        candidate.readiness = DecorationReadiness::Unsupported {
            reason: "adapter is not implemented".into(),
        };
        let result = resolve_decoration(
            vec![candidate],
            DecorationTarget {
                emulator: "retroarch".into(),
                core: None,
            },
        );
        assert!(result.selected.is_none());
        assert!(result.candidates.is_empty());
    }
}
