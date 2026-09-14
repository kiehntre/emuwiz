//! Typed library presentation visibility.
//!
//! Visibility is deliberately separate from source ownership, availability,
//! and dependency evidence. A hidden item can remain fully usable by a
//! subsystem; this module only decides whether ordinary game browsing should
//! render it.

use crate::database::SourceRole;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LibraryVisibility {
    #[default]
    Visible,
    HiddenByDefault,
    UserHidden,
    DependencyOnly,
    AdvancedOnly,
    NotLibraryContent,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VisibilityReason {
    NormalGame,
    User,
    BiosDependency,
    DeviceDependency,
    ParentSupport,
    NonRunnableSupport,
    BiosFirmware,
    SaveVault,
    MemoryCardInventory,
    EmulatorConfig,
    DatMetadata,
    ArtworkMedia,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VisibilityDecision {
    pub visibility: LibraryVisibility,
    pub reason: VisibilityReason,
    /// Availability/evidence is independent of presentation visibility.
    pub evidence_available: bool,
}

impl VisibilityDecision {
    pub fn visible_in_default_library(&self) -> bool {
        self.visibility == LibraryVisibility::Visible
    }

    pub fn visible_in_advanced_library(&self) -> bool {
        self.visibility != LibraryVisibility::NotLibraryContent
    }
}

/// Persistent user overrides are keyed by stable identity, never by a
/// transient row index. Callers should prefer DAT/platform identity and use a
/// path-based key only when no stronger identity exists.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserVisibilityOverrides {
    hidden_keys: BTreeSet<String>,
}

impl UserVisibilityOverrides {
    pub fn hide(&mut self, key: impl Into<String>) {
        self.hidden_keys.insert(key.into());
    }

    pub fn unhide(&mut self, key: &str) {
        self.hidden_keys.remove(key);
    }

    pub fn is_hidden(&self, key: &str) -> bool {
        self.hidden_keys.contains(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.hidden_keys.iter()
    }

    pub fn decision_for(&self, key: &str, base: VisibilityDecision) -> VisibilityDecision {
        if self.is_hidden(key) {
            VisibilityDecision {
                visibility: LibraryVisibility::UserHidden,
                reason: VisibilityReason::User,
                evidence_available: base.evidence_available,
            }
        } else {
            base
        }
    }
}

/// Stable, readable identity key. `canonical_identity` should be a verified
/// platform/DAT identity when available; `fallback` is only used when it is
/// not. This function performs no filesystem access.
pub fn stable_visibility_key(
    platform: Option<&str>,
    canonical_identity: Option<&str>,
    fallback: &str,
) -> String {
    match (platform, canonical_identity) {
        (Some(platform), Some(identity)) if !platform.is_empty() && !identity.is_empty() => {
            format!("identity:{platform}:{identity}")
        }
        _ => format!("fallback:{fallback}"),
    }
}

/// Automatic visibility for source-owned non-game content. These items remain
/// available to their owning subsystem and are not silently treated as games.
pub fn visibility_for_source_role(role: SourceRole) -> VisibilityDecision {
    match role {
        SourceRole::Games | SourceRole::GenericFiles | SourceRole::ArcadeRomset => {
            VisibilityDecision {
                visibility: LibraryVisibility::Visible,
                reason: VisibilityReason::NormalGame,
                evidence_available: true,
            }
        }
        SourceRole::BiosFirmware => non_library(VisibilityReason::BiosFirmware),
        SourceRole::SaveData => non_library(VisibilityReason::SaveVault),
        SourceRole::MemoryCards => non_library(VisibilityReason::MemoryCardInventory),
        SourceRole::EmulatorConfig => non_library(VisibilityReason::EmulatorConfig),
        SourceRole::DatMetadata => non_library(VisibilityReason::DatMetadata),
        SourceRole::ArtworkMedia => non_library(VisibilityReason::ArtworkMedia),
        SourceRole::IncomingUnsorted => VisibilityDecision {
            visibility: LibraryVisibility::HiddenByDefault,
            reason: VisibilityReason::Unknown,
            evidence_available: true,
        },
        SourceRole::Ignored | SourceRole::Unknown => VisibilityDecision {
            visibility: LibraryVisibility::NotLibraryContent,
            reason: VisibilityReason::Unknown,
            evidence_available: false,
        },
    }
}

fn non_library(reason: VisibilityReason) -> VisibilityDecision {
    VisibilityDecision {
        visibility: LibraryVisibility::NotLibraryContent,
        reason,
        evidence_available: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_visibility_keeps_support_evidence_available() {
        let bios = visibility_for_source_role(SourceRole::BiosFirmware);
        assert_eq!(bios.visibility, LibraryVisibility::NotLibraryContent);
        assert!(bios.evidence_available);

        let cards = visibility_for_source_role(SourceRole::MemoryCards);
        assert_eq!(cards.visibility, LibraryVisibility::NotLibraryContent);
        assert!(cards.evidence_available);
    }

    #[test]
    fn default_and_advanced_visibility_are_distinct() {
        let support = VisibilityDecision {
            visibility: LibraryVisibility::DependencyOnly,
            reason: VisibilityReason::DeviceDependency,
            evidence_available: true,
        };
        assert!(!support.visible_in_default_library());
        assert!(support.visible_in_advanced_library());
    }

    #[test]
    fn user_hide_is_reversible_and_does_not_destroy_evidence() {
        let key = stable_visibility_key(Some("PS2"), Some("SLUS-12345"), "/old/path");
        let mut overrides = UserVisibilityOverrides::default();
        let base = VisibilityDecision {
            visibility: LibraryVisibility::Visible,
            reason: VisibilityReason::NormalGame,
            evidence_available: true,
        };
        overrides.hide(&key);
        let hidden = overrides.decision_for(&key, base.clone());
        assert_eq!(hidden.visibility, LibraryVisibility::UserHidden);
        assert!(hidden.evidence_available);
        overrides.unhide(&key);
        assert_eq!(
            overrides.decision_for(&key, base),
            VisibilityDecision {
                visibility: LibraryVisibility::Visible,
                reason: VisibilityReason::NormalGame,
                evidence_available: true,
            }
        );
    }

    #[test]
    fn stable_identity_precedes_path_fallback() {
        assert_eq!(
            stable_visibility_key(Some("MAME"), Some("pacman"), "/roms/pacman.zip"),
            "identity:MAME:pacman"
        );
        assert_eq!(
            stable_visibility_key(None, None, "/roms/pacman.zip"),
            "fallback:/roms/pacman.zip"
        );
    }
}
