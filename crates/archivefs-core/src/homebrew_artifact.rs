//! Metadata contracts for homebrew projects and their artifacts.
//!
//! No discovery, transport, inspection, identity resolution or rights inference
//! is performed here. Provider claims and byte digests remain evidence only.

use serde::{Deserialize, Serialize};

// Reuse provider namespace validation and the checksum/acquisition primitives
// already consumed by the mod download policy; do not create another policy.
pub use crate::mod_catalogue::{
    ModCatalogueHash as ArtifactChecksum, ModCatalogueHashAlgorithm as ArtifactChecksumAlgorithm,
};
pub use crate::mod_provider::{
    ModAcquisitionMode as ArtifactAcquisitionMode, ModProviderId as HomebrewProviderId,
};

/// Exact provider-scoped identity. Neither names nor URLs imply equivalence.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HomebrewProjectId {
    pub provider: HomebrewProviderId,
    pub provider_id: String,
}

/// An explicit asserted relationship; it never merges the two identities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomebrewProjectLink {
    pub project: HomebrewProjectId,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomebrewProject {
    pub id: HomebrewProjectId,
    pub title: String,
    pub summary: Option<String>,
    pub source_page_url: String,
    #[serde(default)]
    pub links: Vec<HomebrewProjectLink>,
    #[serde(default)]
    pub rights: RightsEvidence,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomebrewRelease {
    pub provider_release_id: Option<String>,
    pub version: Option<String>,
    pub tag: Option<String>,
    pub published_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
    #[serde(default)]
    pub prerelease: bool,
}

/// One artifact batch for a known project. Unversioned projects can return
/// artifacts with `release: None` without manufacturing a release or tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomebrewArtifactListing {
    pub project: HomebrewProjectId,
    pub release: Option<HomebrewRelease>,
    pub artifacts: Vec<HomebrewArtifact>,
}

/// URLs use the same string representation as existing release/download
/// metadata. A link/mode is a handoff, not permission to transport bytes:
/// future acquisition must still apply the existing download/network policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactAcquisition {
    pub source_page_url: String,
    pub link_url: Option<String>,
    pub mode: ArtifactAcquisitionMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomebrewArtifact {
    /// Scoped to the listing's project provider, never a global asset ID.
    pub provider_asset_id: String,
    pub filename: String,
    pub size_bytes: Option<u64>,
    pub acquisition: ArtifactAcquisition,
    pub provider_digest: Option<ArtifactChecksum>,
    /// Empty for manuals, source bundles, tools or unknown platform payloads.
    #[serde(default)]
    pub platform_claims: Vec<PlatformClaim>,
    #[serde(default)]
    pub rights: RightsEvidence,
}

impl HomebrewArtifact {
    /// Provenance is determined by where this field came from, not by digest
    /// strength. Matching a provider digest never establishes game identity.
    pub fn provider_digest_evidence(&self) -> Option<ArtifactDigest> {
        self.provider_digest.clone().map(|checksum| ArtifactDigest {
            checksum,
            provenance: DigestProvenance::ProviderComputed,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestProvenance {
    ProviderComputed,
    LocalComputed,
}

/// Byte-integrity evidence only, with no DAT/game-identity authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDigest {
    pub checksum: ArtifactChecksum,
    pub provenance: DigestProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformClaim {
    /// A canonical `crate::platform::Platform::id` when registered; unknown
    /// provider values may be retained for review without alias inference.
    pub platform: String,
    pub evidence: PlatformClaimEvidence,
}

impl PlatformClaim {
    /// Exact registry lookup only. A recognized claim is still not verified
    /// platform identity; structural evidence must be assessed separately.
    pub fn registered_platform(&self) -> Option<&'static crate::platform::Platform> {
        crate::platform::platform_by_id(&self.platform)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformClaimEvidence {
    Extension(String),
    Filename(String),
    ReleaseMetadata(String),
    StructuralInspection(String),
}

/// A recorded assertion and its basis, not a permission inferred elsewhere.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RightsFact<T> {
    #[default]
    Unknown,
    Known {
        value: T,
        evidence: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RightsEvidence {
    pub software_licence: RightsFact<String>,
    pub asset_licence: RightsFact<String>,
    pub redistribution: RightsFact<RedistributionPermission>,
    pub commercial_status: RightsFact<CommercialStatus>,
    pub source_availability: RightsFact<SourceAvailability>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedistributionPermission {
    Permitted,
    Prohibited,
}

/// Price alone says nothing about commercial status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommercialStatus {
    Commercial,
    NonCommercial,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAvailability {
    Available { source_url: Option<String> },
    Unavailable,
}

/// Returns candidates/metadata only. Discovery is independent of whether a
/// provider offers release or artifact listing for already-known projects.
pub trait ProjectDiscoveryProvider {
    type Error;

    fn provider(&self) -> &HomebrewProviderId;
    fn discover_projects(&self, query: &str) -> Result<Vec<HomebrewProject>, Self::Error>;
}

/// Returns releases/artifacts for an already-known provider-scoped project.
/// Implementations must reject IDs belonging to a different provider.
pub trait ArtifactReleaseProvider {
    type Error;

    fn provider(&self) -> &HomebrewProviderId;
    fn artifact_releases(
        &self,
        project: &HomebrewProjectId,
    ) -> Result<Vec<HomebrewArtifactListing>, Self::Error>;
}

#[cfg(test)]
#[path = "homebrew_artifact/tests.rs"]
mod tests;
