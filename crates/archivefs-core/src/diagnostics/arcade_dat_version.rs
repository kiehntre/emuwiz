//! Truthful MAME / FinalBurn Neo version discovery and DAT-version
//! compatibility reporting for Doctor / Emulator Setup.
//!
//! # Advisory only
//!
//! Nothing here changes ROM-completeness semantics. It answers one
//! narrower question - "does the arcade DAT I audited against come from the
//! same emulator build I have installed?" - and does so honestly, defaulting
//! to `Unknown` whenever the two versions cannot be *safely* compared.
//!
//! # Nothing is executed here
//!
//! This module parses version strings the caller already captured (the
//! stdout of `mame -version`, say); it never spawns a process, exactly as
//! [`crate::patch_manager::dolphin_local::parse_dolphin_version`] parses
//! supplied output and never launches Dolphin. In a session where no
//! version string was captured the emulator version is reported as
//! `Unknown` (a truthful state), not guessed.
//!
//! # Version formats - researched, not assumed
//!
//! - **MAME** (`mame -version` / `-help`: "Displays current MAME version
//!   and copyright notice", MAME docs
//!   <https://docs.mamedev.org/commandline/commandline-all.html>). Modern
//!   MAME versions are `0.NNN` - always `0.` followed by an integer - and
//!   the internal build string is `0.NNN (mameNNNN-...)` (docs example
//!   `0.216 (mame0216-154-gabddfb0404c-dirty)`). Historically, up to
//!   MAME 0.106, releases carried an incremental `uN` suffix (`0.106u2`),
//!   and pre-1.0 test releases in 1997-98 carried a `bN` beta suffix
//!   (`0.37b16`); both are parsed and ordered (`0.37b16` < `0.37` <
//!   `0.106u2` < `0.107`).
//! - **FBNeo** (FinalBurn Neo). Its own version string is a 2-to-4 part
//!   dotted number, optionally `v`-prefixed and optionally followed by a
//!   date and a git hash - e.g. `v1.0.0.02`, or `FBNeo 1.0.0.3 260723
//!   GIT7a28a7d`; FBNeo `.dat` `<version>` headers use the same `v1.0.0.02`
//!   shape (neo-source.com "FinalBurn Neo v1.0.0.02 dat file"). Only a
//!   clean dotted token is accepted; anything else -> `Unknown`, per the
//!   task's "FBNeo must fail to Unknown if version formats cannot be safely
//!   compared".

use serde::Serialize;

use super::{DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding};
use crate::dat::model::DatEcosystem;
use crate::diagnostics::profiles::LinuxEmulatorInstallationEvidence;

// ---------------------------------------------------------------------------
// MAME version
// ---------------------------------------------------------------------------

/// The legacy sub-revision phase of a pre-0.107 MAME version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MamePhase {
    /// `0.37b16` - a 1997/98 beta, before the corresponding `0.NN` release.
    Beta(u32),
    /// `0.NNN` - a normal release.
    Release,
    /// `0.106u2` - an incremental update after the corresponding `0.NNN`.
    Update(u32),
}

impl MamePhase {
    /// `(rank, n)` where `rank` orders Beta < Release < Update.
    fn key(self) -> (u8, u32) {
        match self {
            Self::Beta(n) => (0, n),
            Self::Release => (1, 0),
            Self::Update(n) => (2, n),
        }
    }
}

/// A parsed MAME version - `0.NNN`, plus any legacy `uN` / `bN` suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MameVersion {
    /// The `NNN` in `0.NNN`.
    release: u32,
    phase: MamePhase,
}

impl PartialOrd for MameVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MameVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.release, self.phase.key()).cmp(&(other.release, other.phase.key()))
    }
}

impl MameVersion {
    /// Parses a MAME version out of a version string / command output.
    ///
    /// Scans whitespace- and `(`-delimited tokens for the first that is
    /// `0.NNN`, `0.NNNuM`, or `0.NNNbM` (an optional leading `v`/`V` is
    /// tolerated, as in `MAME v0.270`). `NNN` is 1-4 digits; `M` is 1-2.
    /// Returns `None` for anything else - a changed or unexpected output
    /// shape fails soft, never guessed.
    pub fn parse(text: &str) -> Option<Self> {
        text.split([' ', '\t', '\r', '\n', '(', ')', ','])
            .filter_map(Self::parse_token)
            .next()
    }

    fn parse_token(token: &str) -> Option<Self> {
        let token = token.trim();
        let token = token.strip_prefix(['v', 'V']).unwrap_or(token);
        let rest = token.strip_prefix("0.")?;
        // `NNN` then an optional `u`/`b` + `M`.
        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits_end == 0 || digits_end > 4 {
            return None;
        }
        let release: u32 = rest[..digits_end].parse().ok()?;
        let suffix = &rest[digits_end..];
        let phase = if suffix.is_empty() {
            MamePhase::Release
        } else {
            let letter = suffix.chars().next()?;
            let number: u32 = suffix[1..].parse().ok()?;
            if suffix.len() - 1 == 0 || suffix.len() - 1 > 2 {
                return None;
            }
            match letter {
                'u' | 'U' => MamePhase::Update(number),
                'b' | 'B' => MamePhase::Beta(number),
                _ => return None,
            }
        };
        Some(Self { release, phase })
    }

    /// The canonical display form: `0.270`, `0.106u2`, `0.37b16`.
    pub fn display(&self) -> String {
        match self.phase {
            MamePhase::Release => format!("0.{}", self.release),
            MamePhase::Update(n) => format!("0.{}u{n}", self.release),
            MamePhase::Beta(n) => format!("0.{}b{n}", self.release),
        }
    }
}

// ---------------------------------------------------------------------------
// FBNeo version
// ---------------------------------------------------------------------------

/// A parsed FinalBurn Neo version - a 2-to-4 part dotted number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbneoVersion {
    parts: Vec<u32>,
}

impl PartialOrd for FbneoVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FbneoVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let len = self.parts.len().max(other.parts.len());
        for index in 0..len {
            let left = self.parts.get(index).copied().unwrap_or(0);
            let right = other.parts.get(index).copied().unwrap_or(0);
            match left.cmp(&right) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            }
        }
        std::cmp::Ordering::Equal
    }
}

impl FbneoVersion {
    /// Parses an FBNeo version out of a version string / DAT `<version>`
    /// header. Accepts the first token that is a clean 2-to-4 part dotted
    /// number, optionally `v`-prefixed (`v1.0.0.02`, `1.0.0.3`). A token
    /// with any non-digit-non-dot character, or fewer than two parts, is
    /// rejected - so `FBNeo`, `260723`, and `GIT7a28a7d` are all ignored,
    /// and a whole line that cannot be reduced to a clean dotted number
    /// yields `None`.
    pub fn parse(text: &str) -> Option<Self> {
        text.split([' ', '\t', '\r', '\n', '(', ')', ','])
            .filter_map(Self::parse_token)
            .next()
    }

    fn parse_token(token: &str) -> Option<Self> {
        let token = token.trim();
        let token = token.strip_prefix(['v', 'V']).unwrap_or(token);
        if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        let parts: Vec<u32> = token
            .split('.')
            .map(|part| part.parse::<u32>().ok())
            .collect::<Option<_>>()?;
        if parts.len() < 2 || parts.len() > 4 {
            return None;
        }
        Some(Self { parts })
    }

    /// The canonical display form, e.g. `1.0.0.2`.
    pub fn display(&self) -> String {
        self.parts
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }
}

// ---------------------------------------------------------------------------
// Compatibility
// ---------------------------------------------------------------------------

/// How an arcade DAT's catalogue version relates to the installed emulator
/// version. Advisory: a difference is never a claim the ROM set is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcadeDatVersionCompatibility {
    /// The DAT was produced by the same emulator release that is installed.
    Matching,
    /// The DAT is from an older emulator release than the one installed.
    DatOlderThanEmulator,
    /// The DAT is from a newer emulator release than the one installed.
    DatNewerThanEmulator,
    /// One or both versions were missing or could not be safely parsed /
    /// compared.
    Unknown,
    /// The DAT is not a MAME / FBNeo catalogue, so an emulator-version
    /// comparison does not apply.
    NotApplicable,
}

impl ArcadeDatVersionCompatibility {
    pub fn label(self) -> &'static str {
        match self {
            Self::Matching => "Current",
            Self::DatOlderThanEmulator => "Older DAT",
            Self::DatNewerThanEmulator => "Newer DAT",
            Self::Unknown => "Unknown",
            Self::NotApplicable => "Not applicable",
        }
    }

    fn from_ordering(dat_vs_emulator: std::cmp::Ordering) -> Self {
        match dat_vs_emulator {
            std::cmp::Ordering::Equal => Self::Matching,
            std::cmp::Ordering::Less => Self::DatOlderThanEmulator,
            std::cmp::Ordering::Greater => Self::DatNewerThanEmulator,
        }
    }
}

/// Which arcade emulator a readiness fact is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcadeEmulator {
    Mame,
    Fbneo,
}

impl ArcadeEmulator {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mame => "MAME",
            Self::Fbneo => "FinalBurn Neo",
        }
    }

    /// The `LinuxEmulatorInstallationEvidence::emulator` string this
    /// emulator is discovered under.
    fn installation_name(self) -> &'static str {
        match self {
            Self::Mame => "MAME",
            Self::Fbneo => "FinalBurn Neo",
        }
    }

    fn for_ecosystem(ecosystem: DatEcosystem) -> Option<Self> {
        match ecosystem {
            DatEcosystem::MAMEArcade | DatEcosystem::MAMESoftwareList => Some(Self::Mame),
            DatEcosystem::FBNeo => Some(Self::Fbneo),
            _ => None,
        }
    }
}

/// One configured arcade DAT catalogue's version header, as retained by
/// [`crate::dat::model::DatSource`] at parse time. No filename parsing: the
/// caller passes the structured `<version>` field verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArcadeDatCatalogueVersion {
    pub ecosystem: DatEcosystem,
    /// The DAT's own `<version>` header text, when it declared one.
    pub version_header: Option<String>,
}

/// Builds the arcade DAT catalogue-version inputs from the DAT-source health
/// records persisted the last time each source was validated
/// ([`crate::dat::sources::ArcadeCatalogueRevision`]).
///
/// Pure: it consumes health records that are already in memory, opens no
/// file, and parses no DAT - the "no DAT XML is reopened during a Doctor
/// scan" rule is satisfied by construction. One catalogue per distinct
/// arcade ecosystem (the first occurrence wins), so a MAME revision and an
/// FBNeo revision are never merged into one comparison.
pub fn arcade_dat_catalogues_from_source_health<'a>(
    revisions: impl IntoIterator<Item = &'a crate::dat::sources::ArcadeCatalogueRevision>,
) -> Vec<ArcadeDatCatalogueVersion> {
    let mut catalogues: Vec<ArcadeDatCatalogueVersion> = Vec::new();
    for revision in revisions {
        if catalogues
            .iter()
            .any(|catalogue| catalogue.ecosystem == revision.ecosystem)
        {
            continue;
        }
        catalogues.push(ArcadeDatCatalogueVersion {
            ecosystem: revision.ecosystem,
            version_header: revision.version.clone(),
        });
    }
    catalogues
}

/// A truthful readiness fact for one arcade emulator + its DAT catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArcadeEmulatorDatReadiness {
    pub emulator: ArcadeEmulator,
    /// The DAT ecosystem this fact is scoped to, so MAME and FBNeo results
    /// never merge.
    pub dat_ecosystem: DatEcosystem,
    /// Whether an executable for this emulator was discovered at all.
    pub emulator_detected: bool,
    /// The emulator's own reported version, canonicalised, when a version
    /// string was supplied and parsed. `None` = detected-but-version-unknown
    /// or not detected.
    pub emulator_version: Option<String>,
    /// The DAT catalogue's `<version>` header text, verbatim, when present.
    pub dat_revision: Option<String>,
    pub compatibility: ArcadeDatVersionCompatibility,
}

/// Builds one readiness fact.
///
/// `detected` and `version_command_output` describe the *emulator*;
/// `dat` describes the *catalogue*. Neither side is executed or parsed from
/// a filename.
pub fn arcade_emulator_dat_readiness(
    emulator: ArcadeEmulator,
    detected: bool,
    version_command_output: Option<&str>,
    dat: &ArcadeDatCatalogueVersion,
) -> ArcadeEmulatorDatReadiness {
    let emulator_version_display = version_command_output.and_then(|text| match emulator {
        ArcadeEmulator::Mame => MameVersion::parse(text).map(|v| v.display()),
        ArcadeEmulator::Fbneo => FbneoVersion::parse(text).map(|v| v.display()),
    });

    let compatibility = if ArcadeEmulator::for_ecosystem(dat.ecosystem) != Some(emulator) {
        ArcadeDatVersionCompatibility::NotApplicable
    } else {
        match (
            emulator,
            version_command_output,
            dat.version_header.as_deref(),
        ) {
            (ArcadeEmulator::Mame, Some(emulator_text), Some(dat_text)) => {
                match (
                    MameVersion::parse(emulator_text),
                    MameVersion::parse(dat_text),
                ) {
                    (Some(installed), Some(dat_version)) => {
                        ArcadeDatVersionCompatibility::from_ordering(dat_version.cmp(&installed))
                    }
                    _ => ArcadeDatVersionCompatibility::Unknown,
                }
            }
            (ArcadeEmulator::Fbneo, Some(emulator_text), Some(dat_text)) => {
                match (
                    FbneoVersion::parse(emulator_text),
                    FbneoVersion::parse(dat_text),
                ) {
                    (Some(installed), Some(dat_version)) => {
                        ArcadeDatVersionCompatibility::from_ordering(dat_version.cmp(&installed))
                    }
                    _ => ArcadeDatVersionCompatibility::Unknown,
                }
            }
            _ => ArcadeDatVersionCompatibility::Unknown,
        }
    };

    ArcadeEmulatorDatReadiness {
        emulator,
        dat_ecosystem: dat.ecosystem,
        emulator_detected: detected,
        emulator_version: emulator_version_display,
        dat_revision: dat.version_header.clone(),
        compatibility,
    }
}

/// Assembles readiness facts for every configured arcade DAT catalogue,
/// pairing each with whatever is known about the matching emulator.
///
/// `installations` is the already-gathered installation evidence (from
/// [`crate::diagnostics::profiles::discover_linux_emulator_installations`]);
/// `version_outputs` optionally supplies the captured stdout of each
/// emulator's version command (`ArcadeEmulator` -> output). Nothing is run.
pub fn arcade_dat_version_readiness(
    installations: &[LinuxEmulatorInstallationEvidence],
    dat_catalogues: &[ArcadeDatCatalogueVersion],
    version_outputs: &[(ArcadeEmulator, String)],
) -> Vec<ArcadeEmulatorDatReadiness> {
    let detected = |emulator: ArcadeEmulator| {
        installations
            .iter()
            .any(|item| item.emulator == emulator.installation_name())
    };
    let version_output = |emulator: ArcadeEmulator| {
        version_outputs
            .iter()
            .find(|(candidate, _)| *candidate == emulator)
            .map(|(_, output)| output.as_str())
    };

    let mut readiness: Vec<ArcadeEmulatorDatReadiness> = dat_catalogues
        .iter()
        .filter_map(|catalogue| {
            let emulator = ArcadeEmulator::for_ecosystem(catalogue.ecosystem)?;
            Some(arcade_emulator_dat_readiness(
                emulator,
                detected(emulator),
                version_output(emulator),
                catalogue,
            ))
        })
        .collect();

    // A discovered arcade emulator with no configured DAT catalogue at all
    // still deserves an honest "DAT revision unknown" line.
    for emulator in [ArcadeEmulator::Mame, ArcadeEmulator::Fbneo] {
        if detected(emulator) && !readiness.iter().any(|item| item.emulator == emulator) {
            let ecosystem = match emulator {
                ArcadeEmulator::Mame => DatEcosystem::MAMEArcade,
                ArcadeEmulator::Fbneo => DatEcosystem::FBNeo,
            };
            readiness.push(arcade_emulator_dat_readiness(
                emulator,
                true,
                version_output(emulator),
                &ArcadeDatCatalogueVersion {
                    ecosystem,
                    version_header: None,
                },
            ));
        }
    }

    readiness
}

// ---------------------------------------------------------------------------
// Doctor findings
// ---------------------------------------------------------------------------

/// Advisory Doctor findings for arcade emulator / DAT version compatibility.
///
/// Every finding is [`DoctorSeverity::Info`]: a version difference is
/// reported, never treated as a broken ROM set.
pub fn findings_from_arcade_dat_version(readiness: &[ArcadeEmulatorDatReadiness]) -> Vec<Finding> {
    readiness
        .iter()
        .map(|item| {
            let emulator = item.emulator.label();
            let ecosystem = item.dat_ecosystem.label();
            let (title, explanation) = match item.compatibility {
                ArcadeDatVersionCompatibility::Matching => (
                    format!("{ecosystem} DAT matches installed {emulator}"),
                    format!(
                        "The {ecosystem} catalogue revision ({}) is the same as the installed \
                         {emulator} version ({}).",
                        item.dat_revision.as_deref().unwrap_or("unknown"),
                        item.emulator_version.as_deref().unwrap_or("unknown"),
                    ),
                ),
                ArcadeDatVersionCompatibility::DatOlderThanEmulator => (
                    format!("{ecosystem} DAT is older than installed {emulator}"),
                    format!(
                        "The {ecosystem} catalogue ({}) predates the installed {emulator} \
                         version ({}). This is advisory only: a version difference does not by \
                         itself change ROM-set completeness.",
                        item.dat_revision.as_deref().unwrap_or("unknown"),
                        item.emulator_version.as_deref().unwrap_or("unknown"),
                    ),
                ),
                ArcadeDatVersionCompatibility::DatNewerThanEmulator => (
                    format!("{ecosystem} DAT is newer than installed {emulator}"),
                    format!(
                        "The {ecosystem} catalogue ({}) is from a newer {emulator} version than \
                         the one installed ({}). This is advisory only: a version difference \
                         does not by itself change ROM-set completeness.",
                        item.dat_revision.as_deref().unwrap_or("unknown"),
                        item.emulator_version.as_deref().unwrap_or("unknown"),
                    ),
                ),
                ArcadeDatVersionCompatibility::Unknown => {
                    let reason = if !item.emulator_detected {
                        format!("no {emulator} executable was discovered")
                    } else if item.emulator_version.is_none() {
                        format!("the installed {emulator} version could not be determined")
                    } else if item.dat_revision.is_none() {
                        format!("the {ecosystem} DAT declares no catalogue revision")
                    } else {
                        format!(
                            "the {emulator} and {ecosystem} DAT version strings could not be \
                             safely compared"
                        )
                    };
                    (
                        format!("{ecosystem} DAT / {emulator} version compatibility is unknown"),
                        format!("Version compatibility could not be determined: {reason}."),
                    )
                }
                ArcadeDatVersionCompatibility::NotApplicable => (
                    format!("{ecosystem} DAT version comparison does not apply to {emulator}"),
                    format!(
                        "The {ecosystem} catalogue is not a {emulator} catalogue, so no \
                         emulator-version comparison is made."
                    ),
                ),
            };

            let evidence = vec![
                format!(
                    "Installed {emulator}: {}",
                    match (item.emulator_detected, item.emulator_version.as_deref()) {
                        (false, _) => "not detected".to_string(),
                        (true, None) => "detected, version unknown".to_string(),
                        (true, Some(version)) => version.to_string(),
                    }
                ),
                format!(
                    "{ecosystem} DAT revision: {}",
                    item.dat_revision.as_deref().unwrap_or("unknown")
                ),
                format!("Compatibility: {}", item.compatibility.label()),
            ];

            Finding::new(
                format!(
                    "arcade_dat_version.{}",
                    match item.emulator {
                        ArcadeEmulator::Mame => "mame",
                        ArcadeEmulator::Fbneo => "fbneo",
                    }
                ),
                DoctorCategory::Emulators,
                DoctorSubsystem::EmulatorReadiness,
                DoctorSeverity::Info,
                title,
                explanation,
            )
            .with_evidence(evidence)
        })
        .collect()
}

#[cfg(test)]
mod tests;
