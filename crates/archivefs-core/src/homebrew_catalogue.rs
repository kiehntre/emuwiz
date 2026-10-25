//! Offline homebrew catalogue discovery. Metadata is evidence, never game identity.
//!
//! # Rights authority
//!
//! Every rights field in a local catalogue, including a `Known` redistribution
//! value of `Permitted`, is an unverified assertion by whoever wrote the file.
//! It is not permission for EmuWiz to download, install, distribute or
//! republish anything. This module has no acquisition path and never consults
//! these fields to decide an action; the existing download and acquisition
//! policies stay authoritative and do not read them from here.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use caseless::Caseless;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::homebrew_artifact::{
    ArtifactReleaseProvider, HomebrewArtifactListing, HomebrewProject, HomebrewProjectId,
    HomebrewProviderId, ProjectDiscoveryProvider,
};

pub const HOMEBREW_CATALOGUE_SCHEMA_VERSION: u32 = 1;
pub const MAX_HOMEBREW_CATALOGUE_BYTES: usize = 4 * 1024 * 1024;

#[path = "homebrew_catalogue/validation.rs"]
mod validation;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalHomebrewCatalogue {
    pub schema_version: u32,
    pub provider: HomebrewProviderId,
    pub records: Vec<LocalHomebrewRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalHomebrewRecord {
    pub project: HomebrewProject,
    #[serde(default)]
    pub listings: Vec<HomebrewArtifactListing>,
    pub observations: Vec<CatalogueObservation>,
}

/// Dated source evidence, including the scope/limitations of a recorded claim.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogueObservation {
    pub source_url: String,
    /// Calendar date in YYYY-MM-DD form, not an assertion of ongoing freshness.
    pub observed_on: String,
    pub note: String,
}

#[derive(Debug)]
pub enum LocalCatalogueError {
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    NotRegularFile(std::path::PathBuf),
    TooLarge,
    Malformed(String),
    UnsupportedSchema(u32),
    Invalid {
        field: String,
        reason: String,
    },
    Conflict {
        field: String,
        id: String,
    },
    UnsupportedPlatform(String),
    WrongProvider(String),
    ProjectNotFound(String),
}

impl std::fmt::Display for LocalCatalogueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(
                f,
                "cannot read local catalogue {}: {source}",
                path.display()
            ),
            Self::NotRegularFile(path) => write!(
                f,
                "local catalogue is not a regular file: {}",
                path.display()
            ),
            Self::TooLarge => write!(
                f,
                "local catalogue exceeds {MAX_HOMEBREW_CATALOGUE_BYTES} bytes"
            ),
            Self::Malformed(reason) => write!(f, "malformed local catalogue: {reason}"),
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported local catalogue schema {version}")
            }
            Self::Invalid { field, reason } => {
                write!(f, "invalid local catalogue {field}: {reason}")
            }
            Self::Conflict { field, id } => write!(f, "conflicting duplicate in {field}: {id:?}"),
            Self::UnsupportedPlatform(id) => write!(
                f,
                "unsupported platform filter: {id:?}; use a canonical platform id"
            ),
            Self::WrongProvider(id) => write!(f, "project belongs to a different provider: {id:?}"),
            Self::ProjectNotFound(id) => write!(f, "project not found in local catalogue: {id:?}"),
        }
    }
}

impl std::error::Error for LocalCatalogueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A platform claim whose value is not a registered platform ID. It is kept as
/// evidence and never satisfies a platform filter.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnregisteredPlatformClaim {
    pub project: HomebrewProjectId,
    pub provider_asset_id: String,
    pub platform: String,
}

/// Immutable snapshot. Reloading is explicit; discovery never reads a cache,
/// opens a URL, refreshes metadata, or acquires an artifact.
pub struct LocalCatalogueProvider {
    provider: HomebrewProviderId,
    records: BTreeMap<HomebrewProjectId, LocalHomebrewRecord>,
}

impl LocalCatalogueProvider {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, LocalCatalogueError> {
        let path = path.as_ref();
        let io_error = |source| LocalCatalogueError::Io {
            path: path.to_path_buf(),
            source,
        };
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        // A user-supplied FIFO must not block discovery at open(). Verify the
        // opened handle, not a pre-open path that could have been replaced.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path).map_err(io_error)?;
        let metadata = file.metadata().map_err(io_error)?;
        if !metadata.is_file() {
            return Err(LocalCatalogueError::NotRegularFile(path.to_path_buf()));
        }
        if metadata.len() > MAX_HOMEBREW_CATALOGUE_BYTES as u64 {
            return Err(LocalCatalogueError::TooLarge);
        }
        let mut bytes = Vec::new();
        file.take(MAX_HOMEBREW_CATALOGUE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        Self::from_json(&bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, LocalCatalogueError> {
        if bytes.len() > MAX_HOMEBREW_CATALOGUE_BYTES {
            return Err(LocalCatalogueError::TooLarge);
        }
        let malformed =
            |error: serde_json::Error| LocalCatalogueError::Malformed(error.to_string());
        // Deserialize the structs directly first: serde rejects duplicate JSON
        // fields instead of Value's last-key-wins behaviour for known fields.
        let mut catalogue: LocalHomebrewCatalogue =
            serde_json::from_slice(bytes).map_err(malformed)?;
        if catalogue.schema_version != HOMEBREW_CATALOGUE_SCHEMA_VERSION {
            return Err(LocalCatalogueError::UnsupportedSchema(
                catalogue.schema_version,
            ));
        }
        let raw: serde_json::Value = serde_json::from_slice(bytes).map_err(malformed)?;
        let known = serde_json::to_value(&catalogue).map_err(malformed)?;
        validation::known_fields(&raw, &known, "catalogue")?;
        // Specific character diagnostics must precede generic NUL/size checks.
        validation::display_fields(&catalogue.records)?;
        validation::bounded_strings(&raw, "catalogue")?;
        validation::provider_id(&catalogue.provider, "provider")?;
        let mut records = BTreeMap::new();
        for (index, record) in catalogue.records.iter_mut().enumerate() {
            let field = format!("records[{index}]");
            validation::record(record, &catalogue.provider, &field)?;
            canonicalize(record, &field)?;
            insert_unique(
                &mut records,
                record.project.id.clone(),
                record.clone(),
                &field,
            )?;
        }
        Ok(Self {
            provider: catalogue.provider,
            records,
        })
    }

    /// Claims naming a platform that is not in the registry, in stable order.
    /// They stay visible as evidence; this lets a curator find ones that can
    /// never match a platform filter. Case variants of registered IDs are
    /// refused at load time, so what remains is a genuinely unknown name.
    pub fn unregistered_platform_claims(&self) -> Vec<UnregisteredPlatformClaim> {
        let mut claims = Vec::new();
        for (project, record) in &self.records {
            for artifact in record.listings.iter().flat_map(|l| &l.artifacts) {
                for claim in &artifact.platform_claims {
                    if claim.registered_platform().is_none() {
                        claims.push(UnregisteredPlatformClaim {
                            project: project.clone(),
                            provider_asset_id: artifact.provider_asset_id.clone(),
                            platform: claim.platform.clone(),
                        });
                    }
                }
            }
        }
        claims.sort();
        claims.dedup();
        claims
    }

    /// Filter by recorded claims for an exact registered platform, without
    /// inspecting filenames or presenting claims as verified compatibility.
    pub fn discover_for_platform(
        &self,
        query: &str,
        platform: &str,
    ) -> Result<Vec<HomebrewProject>, LocalCatalogueError> {
        if crate::platform::platform_by_id(platform).is_none() {
            return Err(LocalCatalogueError::UnsupportedPlatform(platform.into()));
        }
        self.search(query, Some(platform))
    }

    pub fn record(
        &self,
        id: &HomebrewProjectId,
    ) -> Result<&LocalHomebrewRecord, LocalCatalogueError> {
        if id.provider != self.provider {
            return Err(LocalCatalogueError::WrongProvider(id.provider.to_string()));
        }
        self.records
            .get(id)
            .ok_or_else(|| LocalCatalogueError::ProjectNotFound(id.provider_id.clone()))
    }

    fn search(
        &self,
        query: &str,
        platform: Option<&str>,
    ) -> Result<Vec<HomebrewProject>, LocalCatalogueError> {
        validation::text(query, "query", false)?;
        let query = search_key(query.trim());
        let mut matches: Vec<_> = self
            .records
            .values()
            .filter(|record| {
                let text_matches = search_key(&record.project.title).contains(&query)
                    || record
                        .project
                        .summary
                        .as_deref()
                        .is_some_and(|text| search_key(text).contains(&query));
                let platform_matches = platform.is_none_or(|platform| {
                    record.listings.iter().any(|listing| {
                        listing.artifacts.iter().any(|artifact| {
                            artifact
                                .platform_claims
                                .iter()
                                .any(|claim| claim.platform == platform)
                        })
                    })
                });
                text_matches && platform_matches
            })
            .map(|record| record.project.clone())
            .collect();
        matches.sort_by_cached_key(|project| (search_key(&project.title), project.id.clone()));
        Ok(matches)
    }
}

fn search_key(text: &str) -> String {
    // Unicode canonical caseless matching; does not alter stored metadata/IDs.
    text.chars().nfd().default_case_fold().nfd().collect()
}

fn insert_unique<K: Ord + std::fmt::Debug, V: PartialEq>(
    map: &mut BTreeMap<K, V>,
    id: K,
    value: V,
    field: &str,
) -> Result<(), LocalCatalogueError> {
    if let Some(existing) = map.get(&id) {
        if existing != &value {
            return Err(LocalCatalogueError::Conflict {
                field: field.into(),
                id: format!("{id:?}"),
            });
        }
    } else {
        map.insert(id, value);
    }
    Ok(())
}

fn canonicalize(record: &mut LocalHomebrewRecord, field: &str) -> Result<(), LocalCatalogueError> {
    record
        .project
        .links
        .sort_by(|a, b| (&a.project, &a.evidence).cmp(&(&b.project, &b.evidence)));
    record.project.links.dedup();
    record.observations.sort();
    record.observations.dedup();
    let mut listings = BTreeMap::new();
    for (index, mut listing) in std::mem::take(&mut record.listings).into_iter().enumerate() {
        let field = format!("{field}.listings[{index}]");
        let mut artifacts = BTreeMap::new();
        for mut artifact in listing.artifacts {
            artifact.platform_claims.sort_by_cached_key(|claim| {
                (claim.platform.clone(), format!("{:?}", claim.evidence))
            });
            artifact.platform_claims.dedup();
            insert_unique(
                &mut artifacts,
                artifact.provider_asset_id.clone(),
                artifact,
                &format!("{field}.artifacts"),
            )?;
        }
        listing.artifacts = artifacts.into_values().collect();
        let release_id = listing
            .release
            .as_ref()
            .and_then(|release| release.provider_release_id.clone());
        insert_unique(&mut listings, release_id, listing, &field)?;
    }
    record.listings = listings.into_values().collect();
    Ok(())
}

impl ProjectDiscoveryProvider for LocalCatalogueProvider {
    type Error = LocalCatalogueError;
    fn provider(&self) -> &HomebrewProviderId {
        &self.provider
    }
    fn discover_projects(&self, query: &str) -> Result<Vec<HomebrewProject>, Self::Error> {
        self.search(query, None)
    }
}

impl ArtifactReleaseProvider for LocalCatalogueProvider {
    type Error = LocalCatalogueError;
    fn provider(&self) -> &HomebrewProviderId {
        &self.provider
    }
    fn artifact_releases(
        &self,
        project: &HomebrewProjectId,
    ) -> Result<Vec<HomebrewArtifactListing>, Self::Error> {
        Ok(self.record(project)?.listings.clone())
    }
}

#[cfg(test)]
#[path = "homebrew_catalogue/tests.rs"]
mod tests;
