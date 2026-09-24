//! Provider-neutral third-party mod discovery and acquisition handoff.
//!
//! This module deliberately stops before downloading, extracting, inspecting,
//! or installing a package. Providers return bounded metadata and an honest
//! acquisition state; the existing local package inspectors and installers
//! remain the only authorities allowed to act on bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::game_identity::{GameIdentityReport, IdentityKind, IdentityPlatform};
use crate::mod_catalogue::{ModCatalogueHash, ModCatalogueHashAlgorithm};

pub const MOD_PROVIDER_SCHEMA_VERSION: u32 = 1;
pub const MAX_PROVIDER_RESULTS: usize = 64;
pub const MAX_PROVIDER_RELEASES: usize = 64;
pub const MAX_PROVIDER_FILES: usize = 64;
pub const MAX_PROVIDER_CACHE_ENTRIES: usize = 256;
pub const MAX_PROVIDER_TEXT_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModProviderId(String);

impl ModProviderId {
    pub fn new(value: impl Into<String>) -> Result<Self, ModProviderError> {
        let value = value.into();
        validate_text("provider id", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModProviderCapability {
    MetadataLookup,
    BrowseSearch,
    ReleaseListing,
    BrowserHandoff,
    MachineAcquisition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModSearchQuery {
    pub text: Option<String>,
    pub platform: Option<IdentityPlatform>,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSearchResult {
    pub provider: ModProviderId,
    pub provider_item_id: String,
    pub canonical_source_url: String,
    pub title: String,
    pub summary: Option<String>,
    pub author_or_team: Option<String>,
    pub game_claim: Option<String>,
    pub platform_claims: Vec<IdentityPlatform>,
    pub release: Option<String>,
    pub version: Option<String>,
    pub published_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
    pub tags: Vec<String>,
    pub game_evidence: ModProviderGameEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModProviderEvidenceSource {
    Package,
    ProviderMetadata,
    InstallationText,
    Tag,
    LocalInspection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModProviderIdentityFact {
    pub kind: IdentityKind,
    pub value: String,
    pub source: ModProviderEvidenceSource,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModProviderGameEvidence {
    pub title_claim: Option<String>,
    pub platform_claims: Vec<IdentityPlatform>,
    pub identity_facts: Vec<ModProviderIdentityFact>,
    pub installation_text: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCompatibilityLevel {
    Verified,
    Likely,
    TitleOnly,
    Unknown,
    Conflicting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModCompatibilityAssessment {
    pub level: ModCompatibilityLevel,
    pub matched_identity: Vec<ModProviderIdentityFact>,
    pub reasons: Vec<String>,
}

impl ModCompatibilityAssessment {
    pub fn is_verified(&self) -> bool {
        self.level == ModCompatibilityLevel::Verified
    }
}

impl ModProviderGameEvidence {
    /// Compares provider evidence to the already-verified local game report.
    /// Platform and title claims can improve explanation, but never authorize
    /// `Verified`; a verified identity fact must match an existing verified
    /// [`GameIdentityReport`] fact exactly.
    pub fn assess_against(
        &self,
        selected_game: &GameIdentityReport,
        selected_title: Option<&str>,
    ) -> ModCompatibilityAssessment {
        let mut reasons = Vec::new();
        let mut matched_identity = Vec::new();
        let mut facts_by_kind: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for fact in &self.identity_facts {
            facts_by_kind
                .entry(fact.kind.to_string())
                .or_default()
                .insert(fact.value.trim().to_ascii_uppercase());
        }

        if facts_by_kind.values().any(|values| values.len() > 1) {
            return ModCompatibilityAssessment {
                level: ModCompatibilityLevel::Conflicting,
                matched_identity,
                reasons: vec!["provider supplied conflicting identity values".into()],
            };
        }

        for fact in &self.identity_facts {
            match selected_game.verified_value(fact.kind) {
                Some(selected) if selected.eq_ignore_ascii_case(fact.value.trim()) => {
                    matched_identity.push(fact.clone());
                }
                Some(selected) => {
                    return ModCompatibilityAssessment {
                        level: ModCompatibilityLevel::Conflicting,
                        matched_identity,
                        reasons: vec![format!(
                            "{} conflicts with selected verified value {selected}",
                            fact.kind
                        )],
                    };
                }
                None => reasons.push(format!(
                    "provider supplied {} but the selected game has no verified value of that kind",
                    fact.kind
                )),
            }
        }

        if !matched_identity.is_empty() {
            reasons.push("provider identity exactly matches selected verified identity".into());
            return ModCompatibilityAssessment {
                level: ModCompatibilityLevel::Verified,
                matched_identity,
                reasons,
            };
        }

        let title_matches =
            self.title_claim
                .as_deref()
                .zip(selected_title)
                .is_some_and(|(candidate, selected)| {
                    normalize_title(candidate) == normalize_title(selected)
                });
        if title_matches {
            reasons.push("title matches, but no verified provider identity was supplied".into());
            return ModCompatibilityAssessment {
                level: ModCompatibilityLevel::TitleOnly,
                matched_identity,
                reasons,
            };
        }
        if self.platform_claims.contains(&selected_game.platform) {
            reasons
                .push("platform claim matches, but title/identity evidence is insufficient".into());
            return ModCompatibilityAssessment {
                level: ModCompatibilityLevel::Likely,
                matched_identity,
                reasons,
            };
        }

        reasons.push("no compatible identity or reliable title evidence was found".into());
        ModCompatibilityAssessment {
            level: ModCompatibilityLevel::Unknown,
            matched_identity,
            reasons,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModReleaseFile {
    pub provider: ModProviderId,
    pub provider_file_id: String,
    pub reported_filename: Option<String>,
    pub size_bytes: Option<u64>,
    pub reported_checksum: Option<ModCatalogueHash>,
    pub download: ModDownloadCandidate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModRelease {
    pub provider_release_id: String,
    pub title: String,
    pub version: Option<String>,
    pub published_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
    pub installation_evidence: Vec<String>,
    pub files: Vec<ModReleaseFile>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModAcquisitionMode {
    DirectPermitted,
    BrowserRequired,
    ExternalHost,
    Unavailable,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModDownloadCandidate {
    pub canonical_source_url: String,
    pub acquisition_url: Option<String>,
    pub mode: ModAcquisitionMode,
    pub external_host: bool,
    pub reported_filename: Option<String>,
    pub reported_checksum: Option<ModCatalogueHash>,
}

impl ModDownloadCandidate {
    pub fn requires_browser(&self) -> bool {
        self.mode == ModAcquisitionMode::BrowserRequired
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModProviderProvenance {
    pub schema_version: u32,
    pub provider: ModProviderId,
    pub provider_item_id: String,
    pub provider_release_id: Option<String>,
    pub provider_file_id: Option<String>,
    pub canonical_url: String,
    pub acquisition_mode: ModAcquisitionMode,
    pub reported_filename: Option<String>,
    pub reported_checksum: Option<ModCatalogueHash>,
    pub locally_calculated_checksum: Option<ModCatalogueHash>,
    pub acquired_at_unix_secs: Option<u64>,
    pub compatibility: ModCompatibilityAssessment,
    pub external_host: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalModAcquisition {
    pub provider: ModProviderId,
    pub provider_item_id: Option<String>,
    pub provider_release_id: Option<String>,
    pub provider_file_id: Option<String>,
    pub reported_filename: Option<String>,
    pub locally_calculated_checksum: ModCatalogueHash,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModPackageJoin {
    pub provider: ModProviderId,
    pub provider_file_id: String,
    pub local_checksum: ModCatalogueHash,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModPackageJoinFailure {
    ProviderMismatch,
    ChecksumMismatch { expected: String, actual: String },
    NoStrongEvidence,
}

impl fmt::Display for ModPackageJoinFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderMismatch => formatter.write_str("provider identity does not match"),
            Self::ChecksumMismatch { expected, actual } => {
                write!(
                    formatter,
                    "provider checksum {expected} does not match local {actual}"
                )
            }
            Self::NoStrongEvidence => {
                formatter.write_str("no strong provider/package join evidence")
            }
        }
    }
}

impl std::error::Error for ModPackageJoinFailure {}

pub fn join_acquired_package(
    file: &ModReleaseFile,
    acquisition: &LocalModAcquisition,
) -> Result<ModPackageJoin, ModPackageJoinFailure> {
    if acquisition.provider != file.provider {
        return Err(ModPackageJoinFailure::ProviderMismatch);
    }
    if let Some(expected) = &file.reported_checksum {
        let actual = &acquisition.locally_calculated_checksum;
        if expected.algorithm != actual.algorithm
            || !expected.value.eq_ignore_ascii_case(&actual.value)
        {
            return Err(ModPackageJoinFailure::ChecksumMismatch {
                expected: expected.value.clone(),
                actual: actual.value.clone(),
            });
        }
        return Ok(ModPackageJoin {
            provider: acquisition.provider.clone(),
            provider_file_id: file.provider_file_id.clone(),
            local_checksum: actual.clone(),
            evidence: "reported provider checksum matches locally calculated checksum".into(),
        });
    }
    if acquisition.provider_file_id.as_deref() == Some(file.provider_file_id.as_str()) {
        return Ok(ModPackageJoin {
            provider: acquisition.provider.clone(),
            provider_file_id: file.provider_file_id.clone(),
            local_checksum: acquisition.locally_calculated_checksum.clone(),
            evidence: "persisted provider file ID matches".into(),
        });
    }
    Err(ModPackageJoinFailure::NoStrongEvidence)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModMetadataCacheEntry {
    pub provider_namespace: ModProviderId,
    pub item_id: String,
    pub release_id: Option<String>,
    pub fetched_at_unix_secs: u64,
    pub expires_at_unix_secs: u64,
    pub source_url: String,
    pub schema_version: u32,
    pub result: ModSearchResult,
}

impl ModMetadataCacheEntry {
    pub fn is_stale(&self, now_unix_secs: u64) -> bool {
        now_unix_secs >= self.expires_at_unix_secs
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModMetadataCache {
    pub schema_version: u32,
    pub max_entries: usize,
    pub entries: BTreeMap<String, ModMetadataCacheEntry>,
}

impl Default for ModMetadataCache {
    fn default() -> Self {
        Self {
            schema_version: MOD_PROVIDER_SCHEMA_VERSION,
            max_entries: MAX_PROVIDER_CACHE_ENTRIES,
            entries: BTreeMap::new(),
        }
    }
}

impl ModMetadataCache {
    pub fn insert(&mut self, entry: ModMetadataCacheEntry) -> Result<(), ModProviderError> {
        if self.max_entries == 0 || self.max_entries > MAX_PROVIDER_CACHE_ENTRIES {
            return Err(ModProviderError::InvalidCacheBound);
        }
        let key = format!("{}/{}", entry.provider_namespace, entry.item_id);
        if !self.entries.contains_key(&key) && self.entries.len() >= self.max_entries {
            return Err(ModProviderError::CacheFull);
        }
        self.entries.insert(key, entry);
        Ok(())
    }

    pub fn get_fresh(
        &self,
        provider: &ModProviderId,
        item_id: &str,
        now: u64,
    ) -> Option<&ModMetadataCacheEntry> {
        let entry = self.entries.get(&format!("{provider}/{item_id}"))?;
        (!entry.is_stale(now)).then_some(entry)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModProviderError {
    InvalidText(String),
    UnsupportedCapability(ModProviderCapability),
    InvalidQuery(String),
    NotFound(String),
    CacheFull,
    InvalidCacheBound,
    InvalidUrl(String),
    ProviderUnavailable(String),
    RateLimited,
    BrowserRequired(String),
    Challenge(String),
    CorruptCache(String),
    UnsupportedSchema(u32),
    HttpStatus(u16),
}

impl fmt::Display for ModProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidText(field) => write!(formatter, "{field} is empty or exceeds its bound"),
            Self::UnsupportedCapability(capability) => {
                write!(formatter, "provider does not support {capability:?}")
            }
            Self::InvalidQuery(detail) => formatter.write_str(detail),
            Self::NotFound(id) => write!(formatter, "provider item was not found: {id}"),
            Self::CacheFull => formatter.write_str("bounded provider metadata cache is full"),
            Self::InvalidCacheBound => formatter.write_str("invalid provider metadata cache bound"),
            Self::InvalidUrl(detail) => write!(formatter, "invalid ModDB URL: {detail}"),
            Self::ProviderUnavailable(detail) => {
                write!(formatter, "ModDB is unavailable: {detail}")
            }
            Self::RateLimited => formatter.write_str("ModDB rate-limited the metadata request"),
            Self::BrowserRequired(detail) => {
                write!(formatter, "ModDB requires browser handoff: {detail}")
            }
            Self::Challenge(detail) => {
                write!(formatter, "ModDB returned a challenge page: {detail}")
            }
            Self::CorruptCache(detail) => write!(formatter, "ModDB cache is corrupt: {detail}"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "ModDB cache schema {version} is unsupported")
            }
            Self::HttpStatus(status) => write!(formatter, "ModDB returned HTTP {status}"),
        }
    }
}

impl std::error::Error for ModProviderError {}

pub trait ModProvider {
    fn id(&self) -> &ModProviderId;
    fn capabilities(&self) -> BTreeSet<ModProviderCapability>;
    fn search(&self, query: &ModSearchQuery) -> Result<Vec<ModSearchResult>, ModProviderError>;
    fn releases(&self, item_id: &str) -> Result<Vec<ModRelease>, ModProviderError>;
    fn acquisition_candidate(
        &self,
        file_id: &str,
    ) -> Result<ModDownloadCandidate, ModProviderError>;
}

pub fn require_capability(
    provider: &dyn ModProvider,
    capability: ModProviderCapability,
) -> Result<(), ModProviderError> {
    provider
        .capabilities()
        .contains(&capability)
        .then_some(())
        .ok_or(ModProviderError::UnsupportedCapability(capability))
}

/// Deterministic fixtures model the researched ModDB metadata shape without
/// contacting ModDB. They are intentionally not a ModDB client.
#[derive(Clone, Debug)]
pub struct ModDbFixtureProvider {
    id: ModProviderId,
    results: Vec<ModSearchResult>,
    releases: BTreeMap<String, Vec<ModRelease>>,
    downloads: BTreeMap<String, ModDownloadCandidate>,
}

impl ModDbFixtureProvider {
    pub fn new() -> Self {
        let id = ModProviderId::new("moddb").expect("fixture provider id");
        let verified = fixture_result(
            "darkwatch",
            "Darkwatch Texture Pack",
            Some("SLES-53564"),
            None,
            Some(IdentityPlatform::PlayStation2),
        );
        let title_only = fixture_result(
            "gta-title-only",
            "Grand Theft Auto: San Andreas",
            None,
            None,
            Some(IdentityPlatform::PlayStation2),
        );
        let external = fixture_result(
            "external-pack",
            "External PS2 Texture Pack",
            Some("SLUS-21139"),
            Some("external.example"),
            Some(IdentityPlatform::PlayStation2),
        );
        let browser = fixture_result(
            "browser-pack",
            "Browser Required PS2 Patch",
            Some("SLUS-20864"),
            None,
            Some(IdentityPlatform::PlayStation2),
        );
        let conflicting = fixture_result_with_facts(
            "conflicting",
            "Conflicting Serial Pack",
            vec![
                fixture_fact("SLES-53564", ModProviderEvidenceSource::Package),
                fixture_fact("SLUS-21139", ModProviderEvidenceSource::InstallationText),
            ],
            Some(IdentityPlatform::PlayStation2),
        );

        let mut provider = Self {
            id,
            results: vec![verified, title_only, external, browser, conflicting],
            releases: BTreeMap::new(),
            downloads: BTreeMap::new(),
        };
        provider.add_fixture_release(
            "darkwatch",
            ModAcquisitionMode::DirectPermitted,
            false,
            true,
        );
        provider.add_fixture_release(
            "gta-title-only",
            ModAcquisitionMode::Unavailable,
            false,
            false,
        );
        provider.add_fixture_release(
            "external-pack",
            ModAcquisitionMode::ExternalHost,
            true,
            false,
        );
        provider.add_fixture_release(
            "browser-pack",
            ModAcquisitionMode::BrowserRequired,
            false,
            false,
        );
        provider.add_fixture_release("conflicting", ModAcquisitionMode::Unsupported, false, false);
        provider
    }

    fn add_fixture_release(
        &mut self,
        item_id: &str,
        mode: ModAcquisitionMode,
        external: bool,
        checksum: bool,
    ) {
        let file_id = format!("{item_id}-file");
        let candidate = ModDownloadCandidate {
            canonical_source_url: format!("https://www.moddb.com/addons/{item_id}"),
            acquisition_url: (mode == ModAcquisitionMode::DirectPermitted)
                .then(|| format!("https://files.moddb.com/{file_id}.rar")),
            mode,
            external_host: external,
            reported_filename: Some(format!("{item_id}.rar")),
            reported_checksum: checksum.then(|| ModCatalogueHash {
                algorithm: ModCatalogueHashAlgorithm::Md5,
                value: "0123456789abcdef0123456789abcdef".into(),
            }),
        };
        self.downloads.insert(file_id.clone(), candidate.clone());
        self.releases.insert(
            item_id.into(),
            vec![ModRelease {
                provider_release_id: format!("{item_id}-release"),
                title: format!("{item_id} release"),
                version: Some("1.0".into()),
                published_at_unix_secs: Some(1_700_000_000),
                updated_at_unix_secs: Some(1_700_000_100),
                installation_evidence: vec![
                    "fixture metadata only; local inspection remains required".into(),
                ],
                files: vec![ModReleaseFile {
                    provider: self.id.clone(),
                    provider_file_id: file_id,
                    reported_filename: candidate.reported_filename.clone(),
                    size_bytes: Some(1024),
                    reported_checksum: candidate.reported_checksum.clone(),
                    download: candidate,
                }],
            }],
        );
    }
}

impl Default for ModDbFixtureProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ModProvider for ModDbFixtureProvider {
    fn id(&self) -> &ModProviderId {
        &self.id
    }

    fn capabilities(&self) -> BTreeSet<ModProviderCapability> {
        [
            ModProviderCapability::MetadataLookup,
            ModProviderCapability::BrowseSearch,
            ModProviderCapability::ReleaseListing,
            ModProviderCapability::BrowserHandoff,
        ]
        .into_iter()
        .collect()
    }

    fn search(&self, query: &ModSearchQuery) -> Result<Vec<ModSearchResult>, ModProviderError> {
        require_capability(self, ModProviderCapability::BrowseSearch)?;
        let limit = query.limit.min(MAX_PROVIDER_RESULTS);
        if query.limit > MAX_PROVIDER_RESULTS {
            return Err(ModProviderError::InvalidQuery(
                "query result limit exceeds provider bound".into(),
            ));
        }
        let text = query.text.as_deref().map(normalize_title);
        Ok(self
            .results
            .iter()
            .filter(|result| {
                let text_match = text
                    .as_deref()
                    .is_none_or(|needle| normalize_title(&result.title).contains(needle));
                let platform_match = query
                    .platform
                    .is_none_or(|platform| result.platform_claims.contains(&platform));
                text_match && platform_match
            })
            .take(limit.max(1))
            .cloned()
            .collect())
    }

    fn releases(&self, item_id: &str) -> Result<Vec<ModRelease>, ModProviderError> {
        require_capability(self, ModProviderCapability::ReleaseListing)?;
        self.releases
            .get(item_id)
            .cloned()
            .ok_or_else(|| ModProviderError::NotFound(item_id.into()))
    }

    fn acquisition_candidate(
        &self,
        file_id: &str,
    ) -> Result<ModDownloadCandidate, ModProviderError> {
        let candidate = self
            .downloads
            .get(file_id)
            .cloned()
            .ok_or_else(|| ModProviderError::NotFound(file_id.into()))?;
        match candidate.mode {
            ModAcquisitionMode::BrowserRequired => {
                require_capability(self, ModProviderCapability::BrowserHandoff)?;
            }
            ModAcquisitionMode::DirectPermitted => {
                require_capability(self, ModProviderCapability::MachineAcquisition)?;
            }
            ModAcquisitionMode::ExternalHost
            | ModAcquisitionMode::Unavailable
            | ModAcquisitionMode::Unsupported => {}
        }
        Ok(candidate)
    }
}

fn fixture_result(
    item_id: &str,
    title: &str,
    serial: Option<&str>,
    _external_host: Option<&str>,
    platform: Option<IdentityPlatform>,
) -> ModSearchResult {
    fixture_result_with_facts(
        item_id,
        title,
        serial
            .map(|value| vec![fixture_fact(value, ModProviderEvidenceSource::Package)])
            .unwrap_or_default(),
        platform,
    )
}

fn fixture_result_with_facts(
    item_id: &str,
    title: &str,
    identity_facts: Vec<ModProviderIdentityFact>,
    platform: Option<IdentityPlatform>,
) -> ModSearchResult {
    ModSearchResult {
        provider: ModProviderId::new("moddb").expect("fixture provider id"),
        provider_item_id: item_id.into(),
        canonical_source_url: format!("https://www.moddb.com/mods/{item_id}"),
        title: title.into(),
        summary: Some("representative ModDB-style fixture metadata".into()),
        author_or_team: Some("fixture uploader".into()),
        game_claim: Some(title.into()),
        platform_claims: platform.into_iter().collect(),
        release: Some("Released".into()),
        version: Some("1.0".into()),
        published_at_unix_secs: Some(1_700_000_000),
        updated_at_unix_secs: Some(1_700_000_100),
        tags: vec!["ps2".into(), "texture".into()],
        game_evidence: ModProviderGameEvidence {
            title_claim: Some(title.into()),
            platform_claims: platform.into_iter().collect(),
            identity_facts,
            installation_text: Some("inspect locally before applying".into()),
        },
    }
}

fn fixture_fact(value: &str, source: ModProviderEvidenceSource) -> ModProviderIdentityFact {
    ModProviderIdentityFact {
        kind: IdentityKind::Ps2Serial,
        value: value.into(),
        source,
        detail: "fixture evidence".into(),
    }
}

fn normalize_title(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn validate_text(field: &str, value: &str) -> Result<(), ModProviderError> {
    (!value.trim().is_empty() && value.len() <= MAX_PROVIDER_TEXT_BYTES)
        .then_some(())
        .ok_or_else(|| ModProviderError::InvalidText(field.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_identity::{
        IdentityConfidence, IdentityImageFormat, IdentityProvenance, IdentityStatus,
    };
    use std::path::PathBuf;

    fn selected_ps2(serial: &str) -> GameIdentityReport {
        GameIdentityReport {
            archive_path: PathBuf::from("/games/darkwatch.iso"),
            platform: IdentityPlatform::PlayStation2,
            format: IdentityImageFormat::Iso,
            evidence: vec![crate::game_identity::IdentityEvidence {
                kind: IdentityKind::Ps2Serial,
                status: IdentityStatus::Verified,
                value: Some(serial.into()),
                confidence: IdentityConfidence::StructuredMetadata,
                provenance: IdentityProvenance {
                    archive_path: PathBuf::from("/games/darkwatch.iso"),
                    member_path: None,
                    member_index: None,
                    method: "fixture verified disc structure".into(),
                },
                diagnostic: "fixture".into(),
            }],
            warnings: Vec::new(),
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        }
    }

    #[test]
    fn fixture_exposes_capabilities_without_machine_download() {
        let provider = ModDbFixtureProvider::new();
        assert!(
            provider
                .capabilities()
                .contains(&ModProviderCapability::BrowserHandoff)
        );
        assert!(
            !provider
                .capabilities()
                .contains(&ModProviderCapability::MachineAcquisition)
        );
        let error = provider
            .acquisition_candidate("darkwatch-file")
            .expect_err("direct machine acquisition must be capability-gated");
        assert_eq!(
            error,
            ModProviderError::UnsupportedCapability(ModProviderCapability::MachineAcquisition)
        );
    }

    #[test]
    fn fixture_proves_verified_title_only_external_browser_and_conflict() {
        let provider = ModDbFixtureProvider::new();
        let results = provider
            .search(&ModSearchQuery {
                platform: Some(IdentityPlatform::PlayStation2),
                limit: MAX_PROVIDER_RESULTS,
                ..Default::default()
            })
            .unwrap();
        let selected = selected_ps2("SLES-53564");
        let verified = results
            .iter()
            .find(|r| r.provider_item_id == "darkwatch")
            .unwrap();
        assert_eq!(
            verified
                .game_evidence
                .assess_against(&selected, Some(&verified.title))
                .level,
            ModCompatibilityLevel::Verified
        );
        let title_only = results
            .iter()
            .find(|r| r.provider_item_id == "gta-title-only")
            .unwrap();
        assert_eq!(
            title_only
                .game_evidence
                .assess_against(&selected, Some("Grand Theft Auto: San Andreas"))
                .level,
            ModCompatibilityLevel::TitleOnly
        );
        let conflicting = results
            .iter()
            .find(|r| r.provider_item_id == "conflicting")
            .unwrap();
        assert_eq!(
            conflicting
                .game_evidence
                .assess_against(&selected, Some(&conflicting.title))
                .level,
            ModCompatibilityLevel::Conflicting
        );
        assert_eq!(
            provider
                .acquisition_candidate("external-pack-file")
                .unwrap()
                .mode,
            ModAcquisitionMode::ExternalHost
        );
        assert_eq!(
            provider
                .acquisition_candidate("browser-pack-file")
                .unwrap()
                .mode,
            ModAcquisitionMode::BrowserRequired
        );
    }

    #[test]
    fn platform_claim_alone_is_likely_not_verified() {
        let evidence = ModProviderGameEvidence {
            platform_claims: vec![IdentityPlatform::PlayStation2],
            ..Default::default()
        };
        let assessment = evidence.assess_against(&selected_ps2("SLES-53564"), None);
        assert_eq!(assessment.level, ModCompatibilityLevel::Likely);
        assert!(!assessment.is_verified());
    }

    #[test]
    fn checksum_join_accepts_exact_and_refuses_mismatch() {
        let provider = ModDbFixtureProvider::new();
        let file = &provider.releases("darkwatch").unwrap()[0].files[0];
        let local = LocalModAcquisition {
            provider: provider.id().clone(),
            provider_item_id: Some("darkwatch".into()),
            provider_release_id: Some("darkwatch-release".into()),
            provider_file_id: Some("darkwatch-file".into()),
            reported_filename: Some("darkwatch.rar".into()),
            locally_calculated_checksum: file.reported_checksum.clone().unwrap(),
        };
        assert!(join_acquired_package(file, &local).is_ok());
        let mut mismatch = local.clone();
        mismatch.locally_calculated_checksum.value = "ffffffffffffffffffffffffffffffff".into();
        assert!(matches!(
            join_acquired_package(file, &mismatch),
            Err(ModPackageJoinFailure::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn provenance_round_trip_and_bounded_cache_staleness_work() {
        let provider = ModDbFixtureProvider::new();
        let result = provider
            .search(&ModSearchQuery {
                limit: 1,
                ..Default::default()
            })
            .unwrap()
            .pop()
            .unwrap();
        let assessment = result
            .game_evidence
            .assess_against(&selected_ps2("SLES-53564"), Some(&result.title));
        let provenance = ModProviderProvenance {
            schema_version: MOD_PROVIDER_SCHEMA_VERSION,
            provider: provider.id().clone(),
            provider_item_id: result.provider_item_id.clone(),
            provider_release_id: Some("darkwatch-release".into()),
            provider_file_id: Some("darkwatch-file".into()),
            canonical_url: result.canonical_source_url.clone(),
            acquisition_mode: ModAcquisitionMode::DirectPermitted,
            reported_filename: Some("darkwatch.rar".into()),
            reported_checksum: Some(ModCatalogueHash {
                algorithm: ModCatalogueHashAlgorithm::Md5,
                value: "0123456789abcdef0123456789abcdef".into(),
            }),
            locally_calculated_checksum: None,
            acquired_at_unix_secs: Some(1_700_000_000),
            compatibility: assessment,
            external_host: false,
        };
        let encoded = serde_json::to_vec(&provenance).unwrap();
        let decoded: ModProviderProvenance = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, provenance);

        let mut cache = ModMetadataCache {
            max_entries: 1,
            ..Default::default()
        };
        cache
            .insert(ModMetadataCacheEntry {
                provider_namespace: provider.id().clone(),
                item_id: result.provider_item_id.clone(),
                release_id: None,
                fetched_at_unix_secs: 10,
                expires_at_unix_secs: 20,
                source_url: result.canonical_source_url.clone(),
                schema_version: MOD_PROVIDER_SCHEMA_VERSION,
                result,
            })
            .unwrap();
        assert!(cache.get_fresh(provider.id(), "darkwatch", 19).is_some());
        assert!(cache.get_fresh(provider.id(), "darkwatch", 20).is_none());
        assert_eq!(
            cache
                .insert(ModMetadataCacheEntry {
                    provider_namespace: provider.id().clone(),
                    item_id: "second".into(),
                    release_id: None,
                    fetched_at_unix_secs: 0,
                    expires_at_unix_secs: 1,
                    source_url: "https://example.test".into(),
                    schema_version: MOD_PROVIDER_SCHEMA_VERSION,
                    result: ModSearchResult {
                        provider: provider.id().clone(),
                        provider_item_id: "second".into(),
                        canonical_source_url: "https://example.test".into(),
                        title: "second".into(),
                        summary: None,
                        author_or_team: None,
                        game_claim: None,
                        platform_claims: vec![],
                        release: None,
                        version: None,
                        published_at_unix_secs: None,
                        updated_at_unix_secs: None,
                        tags: vec![],
                        game_evidence: Default::default(),
                    }
                })
                .unwrap_err(),
            ModProviderError::CacheFull
        );
    }
}
