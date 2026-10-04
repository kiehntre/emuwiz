//! Whether a game whose identity is not confirmed is something a person can or
//! should act on.
//!
//! "No confirmed identity" is the ordinary state of most of a large library:
//! EmuWiz only records a verified identity when exact evidence has been read,
//! and many systems have no reference database at all. That is information,
//! not a fault. This module turns the facts already in the catalogue into one
//! classification that Home and Problems & Repair both use. It is pure: it
//! reads nothing, writes nothing, never marks anything verified, and it does
//! not change what the launch path requires.
//!
//! Classes, in the order they are decided:
//!
//! 1. a confirmed canonical identity - nothing to show;
//! 2. verified facts that disagree, or ambiguous evidence - the person must
//!    choose ([`IdentityAttention::NeedsChoice`]); ambiguity never downgrades;
//! 3. a game already matched against reference data (an arcade set audited
//!    against the MAME data) - information;
//! 4. homebrew, prototypes, hacks, translations, demos and similar special
//!    releases, which normal commercial reference data does not describe -
//!    information;
//! 5. no known system - the person can choose one;
//! 6. a system EmuWiz has no reference source for - information;
//! 7. a system with an expected reference source - either a setup step (the
//!    inventory proves none of that kind is installed) or an available action
//!    (match the games against the identification data).

use std::collections::HashSet;
use std::path::Path;

use crate::dat::coverage_expectations::{
    ExpectedAuthoritativeSource, PlatformCoverageExpectation, expected_authoritative_coverage,
};
use crate::dat::model::DatEcosystem;
use crate::game_identity::{GameIdentityReport, IdentityStatus};
use crate::launch::{CanonicalIdentityStatus, canonical_identity_from_game_report};

/// What the person has installed, as far as it could be established.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReferenceInventory {
    /// Lower-case canonical platform names with an enabled, usable catalogue.
    pub platforms: HashSet<String>,
    /// Ecosystems that have at least one enabled, usable catalogue.
    pub ecosystems: Vec<DatEcosystem>,
    /// At least one enabled catalogue whose platform or ecosystem could not be
    /// determined. While true, "nothing is installed" can never be proven.
    pub has_unattributed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChoiceReason {
    /// Verified facts disagree about what this game is.
    Conflict,
    /// The evidence points at more than one possible game.
    Ambiguous,
    /// The game has no system, so nothing can be identified or launched.
    SystemUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InformationalReason {
    /// Matched against reference data by an audit (not a recorded identity).
    MatchedByReferenceData,
    /// Homebrew, prototype, hack, translation, demo or similar.
    SpecialRelease,
    /// EmuWiz knows no reference database for this system.
    NoReferenceSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityAttention {
    /// A confirmed canonical identity exists.
    Identified,
    /// A person has to decide.
    NeedsChoice(ChoiceReason),
    /// A reference database is expected for this system and none is installed.
    SetupRequired(DatEcosystem),
    /// EmuWiz can match these games against identification data now.
    ActionAvailable,
    /// Nothing is wrong and nothing needs doing.
    Informational(InformationalReason),
}

impl IdentityAttention {
    /// Whether a person is asked to do something (information is not).
    pub fn is_actionable(self) -> bool {
        matches!(
            self,
            Self::NeedsChoice(_) | Self::SetupRequired(_) | Self::ActionAvailable
        )
    }
}

/// Filename tags of releases that ordinary commercial reference data does not
/// describe. Used only to explain an unmatched game, never to verify one.
const SPECIAL_RELEASE_TAGS: &[&str] = &[
    "(homebrew",
    "homebrew",
    "(proto",
    "(beta",
    "(demo",
    "(sample",
    "(unl",
    "(hack",
    "[hack",
    "(translated",
    "(aftermarket",
    "(pirate",
    "(bootleg",
    "[cr ",
    "[t+",
    "(pd)",
    "public domain",
];

pub fn is_special_release(relative_path: &Path) -> bool {
    let name = relative_path.to_string_lossy().to_lowercase();
    SPECIAL_RELEASE_TAGS.iter().any(|tag| name.contains(tag))
}

/// Everything the classification looks at for one game.
#[derive(Clone, Copy, Debug)]
pub struct IdentityFacts<'a> {
    /// Canonical platform name, or `None` when the game has no system.
    pub platform: Option<&'a str>,
    pub relative_path: &'a Path,
    pub report: Option<&'a GameIdentityReport>,
    /// A completed audit against reference data exists for this game.
    pub matched_by_reference_data: bool,
}

/// `inventory` is `None` when the installed catalogues could not be listed.
pub fn classify_identity(
    facts: &IdentityFacts<'_>,
    inventory: Option<&ReferenceInventory>,
) -> IdentityAttention {
    if let Some(report) = facts.report {
        match canonical_identity_from_game_report(report).0 {
            CanonicalIdentityStatus::Resolved(_) => return IdentityAttention::Identified,
            CanonicalIdentityStatus::Conflicting => {
                return IdentityAttention::NeedsChoice(ChoiceReason::Conflict);
            }
            CanonicalIdentityStatus::Unknown => {}
        }
        if report
            .evidence
            .iter()
            .any(|evidence| evidence.status == IdentityStatus::Ambiguous)
        {
            return IdentityAttention::NeedsChoice(ChoiceReason::Ambiguous);
        }
    }
    if facts.matched_by_reference_data {
        return IdentityAttention::Informational(InformationalReason::MatchedByReferenceData);
    }
    if is_special_release(facts.relative_path) {
        return IdentityAttention::Informational(InformationalReason::SpecialRelease);
    }
    let Some(platform) = facts
        .platform
        .filter(|platform| !platform.trim().is_empty())
    else {
        return IdentityAttention::NeedsChoice(ChoiceReason::SystemUnknown);
    };
    let ecosystem = match expected_authoritative_coverage(Some(platform)) {
        PlatformCoverageExpectation::UnsupportedOrUnknown { platform: None, .. } => {
            return IdentityAttention::NeedsChoice(ChoiceReason::SystemUnknown);
        }
        PlatformCoverageExpectation::UnsupportedOrUnknown { .. }
        | PlatformCoverageExpectation::NoKnownAuthoritativeSource { .. } => {
            return IdentityAttention::Informational(InformationalReason::NoReferenceSource);
        }
        PlatformCoverageExpectation::ExpectedAuthoritativeSource { source, .. } => {
            vec![source.source]
        }
        PlatformCoverageExpectation::MultipleCandidateSources { sources, .. } => {
            sources.into_iter().map(|source| source.source).collect()
        }
    };
    // Only a provable absence is a setup step; anything unclear is an action.
    let Some(inventory) = inventory else {
        return IdentityAttention::ActionAvailable;
    };
    if inventory.has_unattributed || inventory.platforms.contains(&platform.to_lowercase()) {
        return IdentityAttention::ActionAvailable;
    }
    let installed = |source: &ExpectedAuthoritativeSource| match source {
        ExpectedAuthoritativeSource::Dat(ecosystem) => inventory.ecosystems.contains(ecosystem),
        ExpectedAuthoritativeSource::OfficialScummVmDetector => true,
    };
    if ecosystem.iter().any(installed) {
        return IdentityAttention::ActionAvailable;
    }
    match ecosystem.first() {
        Some(ExpectedAuthoritativeSource::Dat(kind)) => IdentityAttention::SetupRequired(*kind),
        _ => IdentityAttention::ActionAvailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_identity::{
        IdentityConfidence, IdentityEvidence, IdentityImageFormat, IdentityKind, IdentityPlatform,
        IdentityProvenance,
    };
    use std::path::PathBuf;

    fn evidence(
        kind: IdentityKind,
        status: IdentityStatus,
        value: &str,
        confidence: IdentityConfidence,
    ) -> IdentityEvidence {
        IdentityEvidence {
            kind,
            status,
            value: Some(value.into()),
            confidence,
            provenance: IdentityProvenance {
                archive_path: PathBuf::from("/library/game.iso"),
                member_path: None,
                member_index: None,
                method: "test".into(),
            },
            diagnostic: "test".into(),
        }
    }

    fn report(evidence: Vec<IdentityEvidence>) -> GameIdentityReport {
        GameIdentityReport {
            archive_path: PathBuf::from("/library/game.iso"),
            platform: IdentityPlatform::PlayStation,
            format: IdentityImageFormat::Iso,
            evidence,
            warnings: Vec::new(),
            bytes_read: 1,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        }
    }

    fn serial(value: &str) -> IdentityEvidence {
        evidence(
            IdentityKind::Ps1Serial,
            IdentityStatus::Verified,
            value,
            IdentityConfidence::ExactBytes,
        )
    }

    fn facts<'a>(platform: Option<&'a str>, path: &'a str) -> IdentityFacts<'a> {
        IdentityFacts {
            platform,
            relative_path: Path::new(path),
            report: None,
            matched_by_reference_data: false,
        }
    }

    #[test]
    fn a_confirmed_identity_is_never_reported() {
        let report = report(vec![serial("SLUS-00001")]);
        let mut input = facts(Some("PSX"), "psx/Game.iso");
        input.report = Some(&report);
        assert_eq!(
            classify_identity(&input, None),
            IdentityAttention::Identified
        );
    }

    #[test]
    fn disagreeing_verified_facts_need_the_persons_choice() {
        let report = report(vec![serial("SLUS-00001"), serial("SLUS-00002")]);
        let mut input = facts(Some("PSX"), "psx/Game.iso");
        input.report = Some(&report);
        assert_eq!(
            classify_identity(&input, None),
            IdentityAttention::NeedsChoice(ChoiceReason::Conflict)
        );
    }

    #[test]
    fn ambiguous_evidence_needs_a_choice_and_is_never_downgraded() {
        let report = report(vec![evidence(
            IdentityKind::LooseRomTitle,
            IdentityStatus::Ambiguous,
            "Two Games",
            IdentityConfidence::CatalogueContext,
        )]);
        let mut input = facts(Some("Game Boy"), "gb/Game (Homebrew).gb");
        input.report = Some(&report);
        // Neither a reference match, a special-release tag nor missing data
        // can hide an ambiguity.
        input.matched_by_reference_data = true;
        assert_eq!(
            classify_identity(&input, None),
            IdentityAttention::NeedsChoice(ChoiceReason::Ambiguous)
        );
    }

    #[test]
    fn filename_only_evidence_is_never_promoted_to_identified() {
        let report = report(vec![evidence(
            IdentityKind::LooseRomTitle,
            IdentityStatus::Candidate,
            "Some Game",
            IdentityConfidence::FilenameOnly,
        )]);
        let mut input = facts(Some("Game Boy"), "gb/Some Game.gb");
        input.report = Some(&report);
        assert_ne!(
            classify_identity(&input, None),
            IdentityAttention::Identified
        );
        assert_eq!(
            classify_identity(&input, None),
            IdentityAttention::ActionAvailable
        );
    }

    #[test]
    fn a_reference_match_and_special_releases_are_information() {
        let mut matched = facts(Some("Arcade"), "arcade/pacman");
        matched.matched_by_reference_data = true;
        assert_eq!(
            classify_identity(&matched, None),
            IdentityAttention::Informational(InformationalReason::MatchedByReferenceData)
        );
        for name in [
            "gb/Cool Game (Homebrew).gb",
            "snes/Game (Proto).sfc",
            "nes/Game (Hack).nes",
            "gba/Game (Translated En).gba",
        ] {
            assert_eq!(
                classify_identity(&facts(Some("Game Boy"), name), None),
                IdentityAttention::Informational(InformationalReason::SpecialRelease),
                "{name}"
            );
        }
    }

    #[test]
    fn a_game_with_no_system_can_be_given_one() {
        for platform in [None, Some(""), Some("  ")] {
            assert_eq!(
                classify_identity(&facts(platform, "mystery/Game.bin"), None),
                IdentityAttention::NeedsChoice(ChoiceReason::SystemUnknown)
            );
        }
    }

    #[test]
    fn a_system_without_a_reference_source_is_information() {
        assert_eq!(
            classify_identity(&facts(Some("ZX Spectrum"), "zxs/Game.tap"), None),
            IdentityAttention::Informational(InformationalReason::NoReferenceSource)
        );
    }

    #[test]
    fn an_expected_reference_source_is_an_action_unless_provably_not_installed() {
        let game = facts(Some("Game Boy Advance"), "gba/Game.gba");
        // Installed catalogues unknown: never claim something is missing.
        assert_eq!(
            classify_identity(&game, None),
            IdentityAttention::ActionAvailable
        );
        // Provably nothing installed: a setup step.
        let empty = ReferenceInventory::default();
        assert_eq!(
            classify_identity(&game, Some(&empty)),
            IdentityAttention::SetupRequired(DatEcosystem::NoIntro)
        );
        // The right ecosystem is installed.
        let installed = ReferenceInventory {
            ecosystems: vec![DatEcosystem::NoIntro],
            ..ReferenceInventory::default()
        };
        assert_eq!(
            classify_identity(&game, Some(&installed)),
            IdentityAttention::ActionAvailable
        );
        // A catalogue whose system is unknown could be the right one.
        let unclear = ReferenceInventory {
            has_unattributed: true,
            ..ReferenceInventory::default()
        };
        assert_eq!(
            classify_identity(&game, Some(&unclear)),
            IdentityAttention::ActionAvailable
        );
        // Another ecosystem does not count.
        let other = ReferenceInventory {
            ecosystems: vec![DatEcosystem::Redump],
            ..ReferenceInventory::default()
        };
        assert_eq!(
            classify_identity(&game, Some(&other)),
            IdentityAttention::SetupRequired(DatEcosystem::NoIntro)
        );
    }

    #[test]
    fn only_choices_setup_and_actions_ask_the_person_to_do_something() {
        assert!(IdentityAttention::NeedsChoice(ChoiceReason::Conflict).is_actionable());
        assert!(IdentityAttention::SetupRequired(DatEcosystem::NoIntro).is_actionable());
        assert!(IdentityAttention::ActionAvailable.is_actionable());
        assert!(!IdentityAttention::Identified.is_actionable());
        assert!(
            !IdentityAttention::Informational(InformationalReason::NoReferenceSource)
                .is_actionable()
        );
    }
}
