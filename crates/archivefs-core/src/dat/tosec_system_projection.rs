//! Exact, reviewed projection between canonical EmuWiz platforms and TOSEC
//! `System` names (the leading segment of a TOSEC catalogue name, as produced
//! by [`super::tosec_release_pack::classify_tosec_catalogue_name`]).
//!
//! This is *organisation*, not identity: it says which TOSEC system groups a
//! release pack would normally offer for a platform that has already been
//! resolved from independent evidence.  It never resolves a platform from a
//! TOSEC name for a game, never fuzzy-matches, and never enables anything -
//! the user still has to approve any group.  Verification of a game remains
//! exact content matching against DAT entries.
//!
//! Matching is exact on the trimmed system text.  Unknown or unlisted systems
//! (for example `Commodore C64DTV`, `Sinclair ZX81`, `Acorn Atom`) are not
//! mapped to a neighbouring platform.

use crate::dat::coverage_expectations::{
    ExpectedAuthoritativeSource, PlatformCoverageExpectation, expected_authoritative_coverage,
};
use crate::dat::model::DatEcosystem;

/// One reviewed `canonical platform -> TOSEC systems` row.
struct Row {
    platform: &'static str,
    systems: &'static [&'static str],
}

/// The table is deliberately explicit.  `Apple IIGS` is listed under
/// `Apple II` only because the platform registry itself folds the IIGS
/// aliases into `Apple II`.  BBC Micro and Acorn Electron are separate rows:
/// a TOSEC name cannot decide between them.
const ROWS: &[Row] = &[
    Row {
        platform: "Acorn Electron",
        systems: &["Acorn Electron"],
    },
    Row {
        platform: "Amiga",
        systems: &["Commodore Amiga"],
    },
    Row {
        platform: "Amstrad CPC",
        systems: &["Amstrad CPC"],
    },
    Row {
        platform: "Apple II",
        systems: &["Apple II", "Apple IIGS"],
    },
    Row {
        platform: "Atari 8-bit",
        systems: &["Atari 8bit"],
    },
    Row {
        platform: "AtariST",
        systems: &["Atari ST"],
    },
    Row {
        platform: "BBC Micro",
        systems: &["Acorn BBC"],
    },
    Row {
        platform: "Commodore 128",
        systems: &["Commodore C128"],
    },
    Row {
        platform: "Commodore 64",
        systems: &["Commodore C64"],
    },
    Row {
        platform: "VIC-20",
        systems: &["Commodore VIC20"],
    },
    Row {
        platform: "ZX Spectrum",
        systems: &["Sinclair ZX Spectrum"],
    },
];

/// Result of projecting a platform onto TOSEC system names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TosecSystemsForPlatform {
    /// The exact TOSEC system names reviewed for this canonical platform.
    Systems(&'static [&'static str]),
    /// No reviewed TOSEC system projection (unknown, unmapped, or not a
    /// TOSEC-expected platform).
    NotMapped,
}

/// Result of projecting one TOSEC system name onto a canonical platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformForTosecSystem {
    Platform(&'static str),
    /// Not an exactly reviewed system name; nothing is guessed.
    Unknown,
}

/// The reviewed TOSEC systems for a canonical platform id or exact registry
/// alias.  Requires TOSEC to actually be an expected source for the platform.
pub fn tosec_systems_for_platform(platform_hint: Option<&str>) -> TosecSystemsForPlatform {
    let expects_tosec = match expected_authoritative_coverage(platform_hint) {
        PlatformCoverageExpectation::ExpectedAuthoritativeSource { source, .. } => {
            source.source == ExpectedAuthoritativeSource::Dat(DatEcosystem::Tosec)
        }
        PlatformCoverageExpectation::MultipleCandidateSources { sources, .. } => sources
            .iter()
            .any(|source| source.source == ExpectedAuthoritativeSource::Dat(DatEcosystem::Tosec)),
        _ => false,
    };
    let Some(platform) = platform_hint.and_then(crate::canonical_platform_for_alias) else {
        return TosecSystemsForPlatform::NotMapped;
    };
    match ROWS.iter().find(|row| row.platform == platform) {
        Some(row) if expects_tosec => TosecSystemsForPlatform::Systems(row.systems),
        _ => TosecSystemsForPlatform::NotMapped,
    }
}

/// The canonical platform for one exact TOSEC system name.
pub fn platform_for_tosec_system(system: &str) -> PlatformForTosecSystem {
    let system = system.trim();
    ROWS.iter()
        .find(|row| row.systems.contains(&system))
        .map_or(PlatformForTosecSystem::Unknown, |row| {
            PlatformForTosecSystem::Platform(row.platform)
        })
}

#[cfg(test)]
mod tests;

/// Every reviewed row, for lockstep tests.
#[cfg(test)]
fn rows() -> impl Iterator<Item = (&'static str, &'static [&'static str])> {
    ROWS.iter().map(|row| (row.platform, row.systems))
}
