//! GitHub Releases provider for the homebrew artifact foundation.
//!
//! This provider enumerates the releases and assets of a repository EmuWiz
//! **already knows** (`owner/repository` or a repository id). It is not a
//! discovery crawler: there is no search, no topic query and no listing of
//! repositories. It returns metadata only and never downloads an asset; the
//! `browser_download_url` is carried as an acquisition handoff that a user or
//! workflow must separately choose to follow under the existing download
//! policy.
//!
//! What it does and does not claim:
//!
//! - **Identity** is the numeric repository id, release id and asset id. Tags,
//!   names and URLs are metadata and may change without changing identity.
//! - **Platform claims** are bounded heuristics (a recognised ROM extension, or
//!   a `<name>.<rom-ext>.<archive-ext>` filename) and each carries its evidence
//!   kind. They are never verified platform identity; structural inspection
//!   happens after acquisition, elsewhere.
//! - **Rights**: the repository's SPDX id fills only `software_licence`. It does
//!   not imply an asset licence, redistribution permission, or anything about
//!   ROM, artwork or music content. Everything else stays `Unknown`. A
//!   downloadable asset is never treated as redistributable.
//! - **Digests**: a GitHub-reported asset digest is `ProviderComputed`
//!   byte-integrity evidence only.
//!
//! Network policy follows the other official-metadata clients: HTTPS only to
//! `api.github.com`, no process-environment proxy, no redirects, a global
//! timeout, bounded response size, bounded pagination, and a descriptive
//! `User-Agent`. There is no credential seam in the existing infrastructure, so
//! requests are unauthenticated.

use std::collections::BTreeSet;
use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::homebrew_artifact::{
    ArtifactAcquisition, ArtifactAcquisitionMode, ArtifactChecksum, ArtifactChecksumAlgorithm,
    ArtifactReleaseProvider, HomebrewArtifact, HomebrewArtifactListing, HomebrewProjectId,
    HomebrewProviderId, HomebrewRelease, PlatformClaim, PlatformClaimEvidence, RightsEvidence,
    RightsFact,
};

pub const GITHUB_PROVIDER_ID: &str = "github";
const API_HOST: &str = "api.github.com";
const DOWNLOAD_HOSTS: &[&str] = &["github.com"];
pub const GITHUB_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest single API response accepted.
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// Largest release body kept as metadata; longer bodies are truncated.
pub const MAX_BODY_METADATA_BYTES: usize = 16 * 1024;
pub const MAX_ASSETS_PER_RELEASE: usize = 200;
pub const MAX_PAGES_LIMIT: u8 = 10;
pub const MAX_PER_PAGE: u8 = 100;
pub const MAX_RELEASES_LIMIT: usize = 500;

/// Caller policy for one enumeration. Every bound has a hard ceiling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubReleasePolicy {
    pub include_prereleases: bool,
    pub include_drafts: bool,
    pub per_page: u8,
    pub max_pages: u8,
    pub max_releases: usize,
}

impl Default for GithubReleasePolicy {
    fn default() -> Self {
        Self {
            include_prereleases: false,
            include_drafts: false,
            per_page: 50,
            max_pages: 3,
            max_releases: 100,
        }
    }
}

impl GithubReleasePolicy {
    fn clamped(&self) -> Self {
        Self {
            include_prereleases: self.include_prereleases,
            include_drafts: self.include_drafts,
            per_page: self.per_page.clamp(1, MAX_PER_PAGE),
            max_pages: self.max_pages.clamp(1, MAX_PAGES_LIMIT),
            max_releases: self.max_releases.clamp(1, MAX_RELEASES_LIMIT),
        }
    }
}

/// A repository EmuWiz already knows. Parsed from a project id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GithubRepositoryRef {
    /// `owner/repository`.
    Named { owner: String, repository: String },
    /// `id:<numeric repository id>`.
    Id(u64),
}

impl GithubRepositoryRef {
    pub fn parse(text: &str) -> Result<Self, GithubProviderError> {
        let invalid = || GithubProviderError::InvalidRepository(text.to_string());
        if let Some(digits) = text.strip_prefix("id:") {
            if digits.is_empty() || digits.len() > 18 || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(invalid());
            }
            return digits.parse().map(Self::Id).map_err(|_| invalid());
        }
        let (owner, repository) = text.split_once('/').ok_or_else(invalid)?;
        let part_ok = |part: &str, max: usize| {
            !part.is_empty()
                && part.len() <= max
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        if !part_ok(owner, 39) || !part_ok(repository, 100) || repository.contains('/') {
            return Err(invalid());
        }
        Ok(Self::Named {
            owner: owner.into(),
            repository: repository.into(),
        })
    }

    fn api_base(&self) -> String {
        match self {
            Self::Named { owner, repository } => {
                format!("https://{API_HOST}/repos/{owner}/{repository}")
            }
            Self::Id(id) => format!("https://{API_HOST}/repositories/{id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubProviderError {
    /// The project id belongs to a different provider.
    WrongProvider(String),
    InvalidRepository(String),
    Transport(String),
    /// The repository or its releases do not exist or are not visible.
    NotFound,
    /// GitHub refused for rate-limit reasons; `reset_unix` when it said when.
    RateLimited {
        reset_unix: Option<u64>,
    },
    HttpStatus(u16),
    Malformed(String),
    ResponseTooLarge,
    DuplicateReleaseId(String),
    DuplicateAssetId {
        release: String,
        asset: String,
    },
    UrlNotAllowed(String),
}

impl std::fmt::Display for GithubProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongProvider(provider) => {
                write!(
                    formatter,
                    "the project belongs to provider {provider:?}, not GitHub"
                )
            }
            Self::InvalidRepository(text) => {
                write!(formatter, "not a GitHub repository id: {text:?}")
            }
            Self::Transport(detail) => write!(formatter, "GitHub could not be reached: {detail}"),
            Self::NotFound => write!(formatter, "the repository or its releases were not found"),
            Self::RateLimited { .. } => write!(formatter, "GitHub rate limit reached; try later"),
            Self::HttpStatus(status) => write!(formatter, "GitHub answered HTTP {status}"),
            Self::Malformed(detail) => write!(formatter, "GitHub returned unusable data: {detail}"),
            Self::ResponseTooLarge => {
                write!(formatter, "a GitHub response exceeded the size limit")
            }
            Self::DuplicateReleaseId(id) => write!(formatter, "GitHub listed release {id} twice"),
            Self::DuplicateAssetId { release, asset } => {
                write!(formatter, "release {release} lists asset id {asset} twice")
            }
            Self::UrlNotAllowed(url) => {
                write!(formatter, "refused URL outside the GitHub allowlist: {url}")
            }
        }
    }
}

impl std::error::Error for GithubProviderError {}

/// HTTP validators for incremental polling of the first releases page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubValidators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GithubRequest {
    pub url: String,
    pub if_none_match: Option<String>,
    pub if_modified_since: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GithubResponse {
    pub status: u16,
    pub validators: GithubValidators,
    pub rate_limit_remaining: Option<u64>,
    pub rate_limit_reset_unix: Option<u64>,
    pub body: Vec<u8>,
}

/// One bounded GET. Implementations must refuse any URL that is not HTTPS to
/// `api.github.com`, bound the body to `max_body`, and never follow redirects.
pub trait GithubTransport {
    fn get(
        &self,
        request: &GithubRequest,
        max_body: usize,
    ) -> Result<GithubResponse, GithubProviderError>;
}

/// The production transport, configured like the other official-metadata
/// clients: HTTPS only, no environment proxy, no redirects, a global timeout.
#[derive(Debug, Clone)]
pub struct UreqGithubTransport {
    agent: ureq::Agent,
}

impl UreqGithubTransport {
    #[must_use]
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(GITHUB_REQUEST_TIMEOUT))
            .build();
        Self {
            agent: config.new_agent(),
        }
    }
}

impl Default for UreqGithubTransport {
    fn default() -> Self {
        Self::new()
    }
}

fn api_url_allowed(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|host| host == API_HOST)
}

impl GithubTransport for UreqGithubTransport {
    fn get(
        &self,
        request: &GithubRequest,
        max_body: usize,
    ) -> Result<GithubResponse, GithubProviderError> {
        if !api_url_allowed(&request.url) {
            return Err(GithubProviderError::UrlNotAllowed(request.url.clone()));
        }
        let mut builder = self
            .agent
            .get(&request.url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header(
                "User-Agent",
                concat!("archivefs/", env!("CARGO_PKG_VERSION")),
            );
        if let Some(etag) = &request.if_none_match {
            builder = builder.header("If-None-Match", etag);
        }
        if let Some(since) = &request.if_modified_since {
            builder = builder.header("If-Modified-Since", since);
        }
        let mut response = builder
            .call()
            .map_err(|error| GithubProviderError::Transport(error.to_string()))?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let validators = GithubValidators {
            etag: header("etag"),
            last_modified: header("last-modified"),
        };
        let rate_limit_remaining = header("x-ratelimit-remaining").and_then(|v| v.parse().ok());
        let rate_limit_reset_unix = header("x-ratelimit-reset").and_then(|v| v.parse().ok());
        let status = response.status().as_u16();
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(max_body as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|error| GithubProviderError::Transport(error.to_string()))?;
        if body.len() > max_body {
            return Err(GithubProviderError::ResponseTooLarge);
        }
        Ok(GithubResponse {
            status,
            validators,
            rate_limit_remaining,
            rate_limit_reset_unix,
            body,
        })
    }
}

/// GitHub-specific release facts the provider-neutral model has no field for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubReleaseDetails {
    pub release_id: String,
    pub name: Option<String>,
    pub draft: bool,
    /// `Some` only when GitHub exposed the field.
    pub immutable: Option<bool>,
    pub created_at_unix: Option<u64>,
    pub html_url: Option<String>,
    /// The release notes as metadata, truncated to a bound. Never interpreted.
    pub body: Option<String>,
    pub body_truncated: bool,
    pub assets: Vec<GithubAssetDetails>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubAssetDetails {
    pub asset_id: String,
    pub content_type: Option<String>,
    pub created_at_unix: Option<u64>,
    pub updated_at_unix: Option<u64>,
}

/// The repository facts needed for identity and software-licence evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubRepositoryFacts {
    pub repository_id: u64,
    pub full_name: String,
    pub html_url: String,
    pub spdx_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubEnumeration {
    pub repository: GithubRepositoryFacts,
    /// One listing per kept release, newest first as GitHub returned them.
    pub listings: Vec<HomebrewArtifactListing>,
    /// GitHub-specific details, parallel to `listings`.
    pub details: Vec<GithubReleaseDetails>,
    /// Validators of the first releases page, for a later conditional poll.
    pub validators: GithubValidators,
    pub rate_limit_remaining: Option<u64>,
    pub rate_limit_reset_unix: Option<u64>,
    /// True when more releases existed than the policy bounds allowed.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GithubEnumerationOutcome {
    Releases(Box<GithubEnumeration>),
    /// The first releases page is unchanged since the supplied validators.
    NotModified,
}

pub struct GithubReleasesProvider<T: GithubTransport> {
    provider: HomebrewProviderId,
    transport: T,
    policy: GithubReleasePolicy,
}

impl GithubReleasesProvider<UreqGithubTransport> {
    #[must_use]
    pub fn new() -> Self {
        Self::with_transport(UreqGithubTransport::new(), GithubReleasePolicy::default())
    }
}

impl Default for GithubReleasesProvider<UreqGithubTransport> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: GithubTransport> GithubReleasesProvider<T> {
    /// One bounded GET; the size bound is enforced here as well as in the
    /// transport so no transport can hand back an oversized body.
    fn get(&self, request: &GithubRequest) -> Result<GithubResponse, GithubProviderError> {
        if !api_url_allowed(&request.url) {
            return Err(GithubProviderError::UrlNotAllowed(request.url.clone()));
        }
        let response = self.transport.get(request, MAX_RESPONSE_BYTES)?;
        if response.body.len() > MAX_RESPONSE_BYTES {
            return Err(GithubProviderError::ResponseTooLarge);
        }
        Ok(response)
    }

    pub fn with_transport(transport: T, policy: GithubReleasePolicy) -> Self {
        Self {
            provider: HomebrewProviderId::new(GITHUB_PROVIDER_ID)
                .expect("the GitHub provider id is valid"),
            transport,
            policy: policy.clamped(),
        }
    }

    /// Enumerates releases for a known repository. `previous` makes the first
    /// releases-page request conditional. Performs only GETs against the
    /// repository and its releases; never fetches an asset.
    pub fn enumerate(
        &self,
        repository: &GithubRepositoryRef,
        previous: Option<&GithubValidators>,
    ) -> Result<GithubEnumerationOutcome, GithubProviderError> {
        let base = repository.api_base();
        let repository_facts = self.fetch_repository(&base, repository)?;
        let releases_base = format!(
            "https://{API_HOST}/repositories/{}",
            repository_facts.repository_id
        );
        let mut collected: Vec<serde_json::Value> = Vec::new();
        let mut first_validators = GithubValidators::default();
        let mut remaining = None;
        let mut reset = None;
        let mut truncated = false;
        for page in 1..=u32::from(self.policy.max_pages) {
            let conditional = (page == 1).then_some(previous).flatten();
            let response = self.get(&GithubRequest {
                url: format!(
                    "{releases_base}/releases?per_page={}&page={page}",
                    self.policy.per_page
                ),
                if_none_match: conditional.and_then(|v| v.etag.clone()),
                if_modified_since: conditional.and_then(|v| v.last_modified.clone()),
            })?;
            remaining = response.rate_limit_remaining.or(remaining);
            reset = response.rate_limit_reset_unix.or(reset);
            if page == 1 && response.status == 304 && conditional.is_some() {
                return Ok(GithubEnumerationOutcome::NotModified);
            }
            check_status(&response)?;
            if page == 1 {
                first_validators = response.validators.clone();
            }
            let items: Vec<serde_json::Value> = serde_json::from_slice(&response.body)
                .map_err(|error| GithubProviderError::Malformed(error.to_string()))?;
            let full_page = items.len() >= usize::from(self.policy.per_page);
            collected.extend(items);
            if collected.len() > self.policy.max_releases {
                collected.truncate(self.policy.max_releases);
                truncated = true;
                break;
            }
            if !full_page {
                break;
            }
            if page == u32::from(self.policy.max_pages) {
                // A full final page: there may be more releases than we read.
                truncated = true;
            }
        }
        let mut seen_releases = BTreeSet::new();
        let mut listings = Vec::new();
        let mut details = Vec::new();
        for value in &collected {
            let Some((listing, detail)) =
                self.map_release(&repository_facts, value, &mut seen_releases)?
            else {
                continue;
            };
            listings.push(listing);
            details.push(detail);
        }
        Ok(GithubEnumerationOutcome::Releases(Box::new(
            GithubEnumeration {
                repository: repository_facts,
                listings,
                details,
                validators: first_validators,
                rate_limit_remaining: remaining,
                rate_limit_reset_unix: reset,
                truncated,
            },
        )))
    }

    fn fetch_repository(
        &self,
        base: &str,
        requested: &GithubRepositoryRef,
    ) -> Result<GithubRepositoryFacts, GithubProviderError> {
        let response = self.get(&GithubRequest {
            url: base.to_string(),
            ..GithubRequest::default()
        })?;
        check_status(&response)?;
        let value: serde_json::Value = serde_json::from_slice(&response.body)
            .map_err(|error| GithubProviderError::Malformed(error.to_string()))?;
        let malformed = |what: &str| GithubProviderError::Malformed(format!("repository {what}"));
        let repository_id = value
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| malformed("has no numeric id"))?;
        if let GithubRepositoryRef::Id(expected) = requested
            && *expected != repository_id
        {
            return Err(malformed("id does not match the requested id"));
        }
        let full_name = value
            .get("full_name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| malformed("has no full_name"))?
            .to_string();
        let html_url = value
            .get("html_url")
            .and_then(serde_json::Value::as_str)
            .filter(|url| url.starts_with("https://github.com/"))
            .ok_or_else(|| malformed("has no github.com html_url"))?
            .to_string();
        let spdx_id = value
            .get("license")
            .and_then(|license| license.get("spdx_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|spdx| !spdx.is_empty() && *spdx != "NOASSERTION")
            .map(str::to_string);
        Ok(GithubRepositoryFacts {
            repository_id,
            full_name,
            html_url,
            spdx_id,
        })
    }

    fn map_release(
        &self,
        repository: &GithubRepositoryFacts,
        value: &serde_json::Value,
        seen: &mut BTreeSet<String>,
    ) -> Result<Option<(HomebrewArtifactListing, GithubReleaseDetails)>, GithubProviderError> {
        let malformed = |what: &str| GithubProviderError::Malformed(format!("release {what}"));
        let release_id = value
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| malformed("has no numeric id"))?
            .to_string();
        if !seen.insert(release_id.clone()) {
            return Err(GithubProviderError::DuplicateReleaseId(release_id));
        }
        let draft = value
            .get("draft")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let prerelease = value
            .get("prerelease")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if (draft && !self.policy.include_drafts)
            || (prerelease && !self.policy.include_prereleases)
        {
            return Ok(None);
        }
        let text = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let published_at_unix_secs = text("published_at").and_then(|t| parse_rfc3339_utc(&t));
        let release_page = text("html_url")
            .filter(|url| url.starts_with("https://github.com/"))
            .unwrap_or_else(|| repository.html_url.clone());
        let (body, body_truncated) = match text("body") {
            Some(body) if body.len() > MAX_BODY_METADATA_BYTES => {
                let mut end = MAX_BODY_METADATA_BYTES;
                while !body.is_char_boundary(end) {
                    end -= 1;
                }
                (Some(body[..end].to_string()), true)
            }
            other => (other, false),
        };

        let assets = value
            .get("assets")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if assets.len() > MAX_ASSETS_PER_RELEASE {
            return Err(malformed("lists more assets than the bound allows"));
        }
        let mut seen_assets = BTreeSet::new();
        let mut artifacts = Vec::new();
        let mut asset_details = Vec::new();
        for asset in assets {
            let (artifact, detail) = map_asset(repository, &release_page, asset)?;
            if !seen_assets.insert(artifact.provider_asset_id.clone()) {
                return Err(GithubProviderError::DuplicateAssetId {
                    release: release_id,
                    asset: artifact.provider_asset_id,
                });
            }
            artifacts.push(artifact);
            asset_details.push(detail);
        }

        let release = HomebrewRelease {
            provider_release_id: Some(release_id.clone()),
            // A GitHub tag is not asserted to be a version number.
            version: None,
            tag: text("tag_name"),
            published_at_unix_secs,
            updated_at_unix_secs: None,
            prerelease,
        };
        let listing = HomebrewArtifactListing {
            project: HomebrewProjectId {
                provider: self.provider.clone(),
                provider_id: repository.repository_id.to_string(),
            },
            release: Some(release),
            artifacts,
        };
        let detail = GithubReleaseDetails {
            release_id,
            name: text("name"),
            draft,
            immutable: value.get("immutable").and_then(serde_json::Value::as_bool),
            created_at_unix: text("created_at").and_then(|t| parse_rfc3339_utc(&t)),
            html_url: Some(release_page),
            body,
            body_truncated,
            assets: asset_details,
        };
        Ok(Some((listing, detail)))
    }
}

impl<T: GithubTransport> ArtifactReleaseProvider for GithubReleasesProvider<T> {
    type Error = GithubProviderError;

    fn provider(&self) -> &HomebrewProviderId {
        &self.provider
    }

    fn artifact_releases(
        &self,
        project: &HomebrewProjectId,
    ) -> Result<Vec<HomebrewArtifactListing>, Self::Error> {
        if project.provider != self.provider {
            return Err(GithubProviderError::WrongProvider(
                project.provider.as_str().to_string(),
            ));
        }
        // A bare numeric id is the canonical form listings are returned with.
        let reference = if project.provider_id.bytes().all(|b| b.is_ascii_digit())
            && !project.provider_id.is_empty()
        {
            GithubRepositoryRef::parse(&format!("id:{}", project.provider_id))?
        } else {
            GithubRepositoryRef::parse(&project.provider_id)?
        };
        match self.enumerate(&reference, None)? {
            GithubEnumerationOutcome::Releases(enumeration) => Ok(enumeration.listings),
            GithubEnumerationOutcome::NotModified => Ok(Vec::new()),
        }
    }
}

fn check_status(response: &GithubResponse) -> Result<(), GithubProviderError> {
    match response.status {
        200 => Ok(()),
        404 => Err(GithubProviderError::NotFound),
        403 | 429 if response.rate_limit_remaining == Some(0) || response.status == 429 => {
            Err(GithubProviderError::RateLimited {
                reset_unix: response.rate_limit_reset_unix,
            })
        }
        other => Err(GithubProviderError::HttpStatus(other)),
    }
}

/// Maps one release asset. Platform claims and rights are evidence only.
fn map_asset(
    repository: &GithubRepositoryFacts,
    release_page: &str,
    asset: &serde_json::Value,
) -> Result<(HomebrewArtifact, GithubAssetDetails), GithubProviderError> {
    let malformed = |what: &str| GithubProviderError::Malformed(format!("asset {what}"));
    let asset_id = asset
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| malformed("has no numeric id"))?
        .to_string();
    let filename = asset
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| {
            !name.is_empty() && !name.contains(['/', '\\', '\0']) && *name != "." && *name != ".."
        })
        .ok_or_else(|| malformed("has no safe file name"))?
        .to_string();
    let size_bytes = asset.get("size").and_then(serde_json::Value::as_u64);
    let uploaded = asset
        .get("state")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|state| state == "uploaded");
    let download = asset
        .get("browser_download_url")
        .and_then(serde_json::Value::as_str)
        .filter(|url| download_url_allowed(url))
        .map(str::to_string);
    // The link is a handoff only. Rights stay Unknown, so the provider never
    // asserts that direct acquisition is permitted.
    let (link_url, mode) = match (&download, uploaded) {
        (Some(url), true) => (Some(url.clone()), ArtifactAcquisitionMode::BrowserRequired),
        (_, false) => (None, ArtifactAcquisitionMode::Unavailable),
        (None, true) => (None, ArtifactAcquisitionMode::Unsupported),
    };
    let provider_digest = asset
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_digest);
    let mut rights = RightsEvidence::default();
    if let Some(spdx) = &repository.spdx_id {
        rights.software_licence = RightsFact::Known {
            value: spdx.clone(),
            evidence: format!(
                "GitHub reports SPDX id {spdx} for repository {}; this says nothing about \
                 the asset's own licence or any right to redistribute it",
                repository.full_name
            ),
        };
    }
    let text = |key: &str| {
        asset
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
    };
    let artifact = HomebrewArtifact {
        provider_asset_id: asset_id.clone(),
        filename: filename.clone(),
        size_bytes,
        acquisition: ArtifactAcquisition {
            source_page_url: release_page.to_string(),
            link_url,
            mode,
        },
        provider_digest,
        platform_claims: platform_claims_for(&filename),
        rights,
    };
    let detail = GithubAssetDetails {
        asset_id,
        content_type: text("content_type").map(str::to_string),
        created_at_unix: text("created_at").and_then(parse_rfc3339_utc),
        updated_at_unix: text("updated_at").and_then(parse_rfc3339_utc),
    };
    Ok((artifact, detail))
}

fn download_url_allowed(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|host| DOWNLOAD_HOSTS.contains(&host))
        && !url.contains(char::is_whitespace)
}

/// `sha256:<64 hex>` or `sha1:<40 hex>`; anything else is not a digest we
/// record (and is never turned into an error).
fn parse_digest(text: &str) -> Option<ArtifactChecksum> {
    let (algorithm, value) = text.split_once(':')?;
    let (algorithm, length) = match algorithm.to_ascii_lowercase().as_str() {
        "sha256" => (ArtifactChecksumAlgorithm::Sha256, 64),
        "sha1" => (ArtifactChecksumAlgorithm::Sha1, 40),
        _ => return None,
    };
    (value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit())).then(|| {
        ArtifactChecksum {
            algorithm,
            value: value.to_ascii_lowercase(),
        }
    })
}

/// Recognised ROM extensions -> canonical registry platform ids. Deliberately
/// small and unambiguous; `.bin`, `.rom`, `.img` and similar are left out.
const ROM_EXTENSIONS: &[(&str, &str)] = &[
    ("gba", "Game Boy Advance"),
    ("gb", "Game Boy"),
    ("gbc", "Game Boy Color"),
    ("nes", "NES"),
    ("sfc", "SNES"),
    ("smc", "SNES"),
    ("sms", "MasterSystem"),
    ("gg", "GameGear"),
    ("nds", "Nintendo DS"),
    ("a26", "Atari2600"),
    ("a78", "Atari7800"),
    ("lnx", "Atari Lynx"),
];
const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "7z", "rar"];

/// Bounded filename heuristics. Each claim names its evidence kind; none is
/// verified platform identity.
fn platform_claims_for(filename: &str) -> Vec<PlatformClaim> {
    let lower = filename.to_ascii_lowercase();
    let mut parts = lower.rsplit('.');
    let last = parts.next().unwrap_or_default();
    let lookup = |extension: &str| {
        ROM_EXTENSIONS
            .iter()
            .find(|(known, _)| *known == extension)
            .map(|(_, platform)| (*platform).to_string())
    };
    if let Some(platform) = lookup(last) {
        return vec![PlatformClaim {
            platform,
            evidence: PlatformClaimEvidence::Extension(last.to_string()),
        }];
    }
    if ARCHIVE_EXTENSIONS.contains(&last)
        && let Some(inner) = parts.next()
        && let Some(platform) = lookup(inner)
        // A non-empty stem must precede ".<rom-ext>.<archive-ext>".
        && filename.len() > last.len() + inner.len() + 2
    {
        return vec![PlatformClaim {
            platform,
            evidence: PlatformClaimEvidence::Filename(format!(".{inner}.{last}")),
        }];
    }
    Vec::new()
}

/// Parses `YYYY-MM-DDTHH:MM:SSZ` (GitHub's only timestamp form) to Unix seconds.
fn parse_rfc3339_utc(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

#[cfg(test)]
#[path = "homebrew_github/tests.rs"]
mod tests;
