//! Two explicit official-tool providers, not a marketplace or a replacement DAT stack.
pub mod discovery;
pub mod mame;
pub mod scummvm;
mod tool;

use super::model::IdentityProvider;
use crate::dat::model::ParsedDat;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub type ProviderResult<T> = Result<T, String>;
pub const PARSER_VERSION: u32 = 1;
pub const MAX_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum ProviderRecords {
    DatLike {
        catalogue: ParsedDat,
        listxml: String,
    },
    NativeDetection(Vec<scummvm::DetectionRecord>),
}

/// Immutable payload. Import time is stored by the catalogue independently of
/// content identity, so checking the same tool twice does not invent an update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub provider: IdentityProvider,
    pub version: String,
    pub source_identifier: String,
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub source_sha256: String,
    pub parser_version: u32,
    pub records: ProviderRecords,
    pub warnings: Vec<String>,
}

impl ProviderSnapshot {
    pub fn sha256(&self) -> ProviderResult<String> {
        serde_json::to_vec(self)
            .map(|b| sha256(&b))
            .map_err(|e| e.to_string())
    }
    pub fn record_count(&self) -> usize {
        match &self.records {
            ProviderRecords::DatLike { catalogue, .. } => catalogue.games.len(),
            ProviderRecords::NativeDetection(records) => records.len(),
        }
    }
    pub fn validate(&self) -> ProviderResult<()> {
        if self.parser_version != PARSER_VERSION
            || self.version.trim().is_empty()
            || self.record_count() == 0
        {
            return Err("Unsupported or empty provider snapshot".into());
        }
        match (&self.provider, &self.records) {
            (IdentityProvider::Mame, ProviderRecords::DatLike { catalogue: dat, .. })
                if dat.source.ecosystem == crate::dat::model::DatEcosystem::MAMEArcade => {}
            (IdentityProvider::ScummVm, ProviderRecords::NativeDetection(_)) => {}
            _ => return Err("Provider and evidence model disagree".into()),
        }
        Ok(())
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchStatus {
    Exact,
    Probable,
    Ambiguous,
    NoMatch,
    NeedsRecheck,
}

/// ScummVM result semantics are intentionally finer than a boolean match.
/// A runtime ID, a table signature, and EmuWiz filename clues are different
/// kinds of evidence and must remain distinguishable to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionClass {
    OfficialExact,
    OfficialFallback,
    OfficialDetectionCoverageGap,
    EmuwizDerivedProbable,
    Unknown,
}
impl DetectionClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::OfficialExact => "Official exact",
            Self::OfficialFallback => "Official fallback",
            Self::OfficialDetectionCoverageGap => "Official detection coverage gap",
            Self::EmuwizDerivedProbable => "EmuWiz-derived probable",
            Self::Unknown => "Unknown",
        }
    }
}
impl MatchStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Exact => "Exact",
            Self::Probable => "Probable",
            Self::Ambiguous => "Ambiguous",
            Self::NoMatch => "No match",
            Self::NeedsRecheck => "Needs re-check",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchOrigin {
    OfficialMame,
    OfficialScummVm,
    EmuwizDiscovery,
}
impl MatchOrigin {
    pub fn label(self) -> &'static str {
        match self {
            Self::OfficialMame => "MAME official",
            Self::OfficialScummVm => "Official ScummVM match",
            Self::EmuwizDiscovery => "EmuWiz discovery — not an official match",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderIdentityResult {
    pub provider: IdentityProvider,
    pub snapshot_sha256: String,
    pub path: PathBuf,
    pub status: MatchStatus,
    pub detection_class: DetectionClass,
    pub origin: MatchOrigin,
    pub detector_method: Option<String>,
    pub official_game_id: Option<String>,
    pub configured_target_id: Option<String>,
    pub configured_target_evidence: Option<String>,
    pub possible_base_game_id: Option<String>,
    pub cleaned_local_title: Option<String>,
    pub candidates: Vec<String>,
    pub details: Vec<String>,
    pub discovery: Option<discovery::DiscoveryResult>,
}
impl ProviderIdentityResult {
    /// Presentation only. The original evidence and source attribution survive.
    pub fn status_against(&self, active_snapshot: Option<&str>) -> MatchStatus {
        if active_snapshot != Some(self.snapshot_sha256.as_str()) {
            MatchStatus::NeedsRecheck
        } else {
            self.status
        }
    }
}

/// Explicit check only: captures metadata into memory. It does not publish a
/// snapshot, update the user's emulator, run a game or touch its configuration.
pub fn check_provider(
    provider: IdentityProvider,
    executable: &Path,
) -> ProviderResult<ProviderSnapshot> {
    match provider {
        IdentityProvider::Mame => mame::capture(executable),
        IdentityProvider::ScummVm => scummvm::capture(executable),
        _ => Err("Only MAME and ScummVM belong to this proof of concept".into()),
    }
}

pub fn verify(snapshot: &ProviderSnapshot, path: &Path) -> ProviderResult<ProviderIdentityResult> {
    snapshot.validate()?;
    match snapshot.provider {
        IdentityProvider::Mame => mame::verify(snapshot, path),
        IdentityProvider::ScummVm => scummvm::verify(snapshot, path),
        _ => Err("Unsupported provider".into()),
    }
}
