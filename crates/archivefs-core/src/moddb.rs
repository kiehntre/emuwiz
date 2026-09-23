//! Conservative ModDB metadata and browser-handoff provider.
//!
//! This module deliberately stops at parsed metadata. It never follows a
//! ModDB download/start route, downloads a payload, executes an installer, or
//! turns a title/tag into verified game identity. Local packages continue
//! through the existing hash, inspection, preview, and transaction pipeline.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::game_identity::{IdentityKind, IdentityPlatform};
use crate::identity_source::net_policy::{SystemResolver, validate_public_https_url};
use crate::mod_catalogue::{ModCatalogueHash, ModCatalogueHashAlgorithm};
use crate::mod_provider::{
    MAX_PROVIDER_CACHE_ENTRIES, MAX_PROVIDER_FILES, MAX_PROVIDER_RELEASES, MAX_PROVIDER_TEXT_BYTES,
    ModAcquisitionMode, ModDownloadCandidate, ModProvider, ModProviderCapability, ModProviderError,
    ModProviderEvidenceSource, ModProviderGameEvidence, ModProviderId, ModProviderIdentityFact,
    ModRelease, ModReleaseFile, ModSearchQuery, ModSearchResult,
};

pub const MODDB_PROVIDER_ID: &str = "moddb";
pub const MODDB_CACHE_SCHEMA_VERSION: u32 = 1;
pub const MODDB_CACHE_TTL: u64 = 7 * 24 * 60 * 60;
pub const MODDB_MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MODDB_MAX_REDIRECTS: usize = 3;
pub const MODDB_MAX_TAGS: usize = 64;
pub const MODDB_MAX_GAME_FACTS: usize = 16;
pub const MODDB_MAX_RELEASES: usize = MAX_PROVIDER_RELEASES;
pub const MODDB_MAX_FILES: usize = MAX_PROVIDER_FILES;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(45);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModDbCacheState {
    #[default]
    Fresh,
    Stale,
    Offline,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModDbPageRecord {
    pub result: ModSearchResult,
    pub releases: Vec<ModRelease>,
    pub original_url: String,
    pub canonical_url: String,
    pub retrieved_url: String,
    pub retrieved_at_unix_secs: u64,
    pub source_fingerprint_sha256: String,
    #[serde(default)]
    pub cache_state: ModDbCacheState,
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModDbCacheEntry {
    pub provider: ModProviderId,
    pub canonical_url: String,
    pub fetched_at_unix_secs: u64,
    pub expires_at_unix_secs: u64,
    pub source_fingerprint_sha256: String,
    pub result: ModSearchResult,
    pub releases: Vec<ModRelease>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModDbCacheDocument {
    pub schema_version: u32,
    pub provider: ModProviderId,
    pub max_entries: usize,
    pub entries: BTreeMap<String, ModDbCacheEntry>,
}

impl Default for ModDbCacheDocument {
    fn default() -> Self {
        Self {
            schema_version: MODDB_CACHE_SCHEMA_VERSION,
            provider: ModProviderId::new(MODDB_PROVIDER_ID).expect("constant provider id"),
            max_entries: MAX_PROVIDER_CACHE_ENTRIES,
            entries: BTreeMap::new(),
        }
    }
}

impl ModDbCacheDocument {
    pub fn validate(&self) -> Result<(), ModProviderError> {
        if self.schema_version != MODDB_CACHE_SCHEMA_VERSION {
            return Err(ModProviderError::UnsupportedSchema(self.schema_version));
        }
        if self.provider.as_str() != MODDB_PROVIDER_ID {
            return Err(ModProviderError::CorruptCache(
                "cache provider is not ModDB".into(),
            ));
        }
        if self.max_entries == 0 || self.max_entries > MAX_PROVIDER_CACHE_ENTRIES {
            return Err(ModProviderError::InvalidCacheBound);
        }
        if self.entries.len() > self.max_entries {
            return Err(ModProviderError::CorruptCache(
                "cache exceeds its entry bound".into(),
            ));
        }
        for (key, entry) in &self.entries {
            if key != &entry.canonical_url
                || entry.provider.as_str() != MODDB_PROVIDER_ID
                || entry.result.provider != entry.provider
                || entry.result.canonical_source_url != entry.canonical_url
                || canonicalize_moddb_url(&entry.canonical_url).ok().as_deref()
                    != Some(entry.canonical_url.as_str())
                || entry.releases.len() > MODDB_MAX_RELEASES
            {
                return Err(ModProviderError::CorruptCache(
                    "cache entry identity or bounds are inconsistent".into(),
                ));
            }
            if entry
                .releases
                .iter()
                .any(|release| release.files.len() > MODDB_MAX_FILES)
            {
                return Err(ModProviderError::CorruptCache(
                    "cache release contains too many files".into(),
                ));
            }
        }
        Ok(())
    }

    fn insert(&mut self, page: &ModDbPageRecord) -> Result<(), ModProviderError> {
        self.validate()?;
        if !self.entries.contains_key(&page.canonical_url) && self.entries.len() >= self.max_entries
        {
            return Err(ModProviderError::CacheFull);
        }
        self.entries.insert(
            page.canonical_url.clone(),
            ModDbCacheEntry {
                provider: page.result.provider.clone(),
                canonical_url: page.canonical_url.clone(),
                fetched_at_unix_secs: page.retrieved_at_unix_secs,
                expires_at_unix_secs: page.retrieved_at_unix_secs.saturating_add(MODDB_CACHE_TTL),
                source_fingerprint_sha256: page.source_fingerprint_sha256.clone(),
                result: page.result.clone(),
                releases: page.releases.clone(),
            },
        );
        Ok(())
    }

    fn page(&self, canonical_url: &str, now: u64, offline: bool) -> Option<ModDbPageRecord> {
        let entry = self.entries.get(canonical_url)?;
        let state = if offline {
            ModDbCacheState::Offline
        } else if now < entry.expires_at_unix_secs {
            ModDbCacheState::Fresh
        } else {
            ModDbCacheState::Stale
        };
        Some(ModDbPageRecord {
            result: entry.result.clone(),
            releases: entry.releases.clone(),
            original_url: entry.canonical_url.clone(),
            canonical_url: entry.canonical_url.clone(),
            retrieved_url: entry.canonical_url.clone(),
            retrieved_at_unix_secs: entry.fetched_at_unix_secs,
            source_fingerprint_sha256: entry.source_fingerprint_sha256.clone(),
            cache_state: state,
            warning: (state != ModDbCacheState::Fresh)
                .then(|| "metadata is cached and may not reflect the current ModDB page".into()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModDbHttpResponse {
    pub status: u16,
    pub location: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub retrieved_url: String,
}

pub trait ModDbTransport: Send + Sync {
    fn get(&self, url: &str) -> Result<ModDbHttpResponse, ModProviderError>;
}

#[derive(Debug, Clone)]
pub struct UreqModDbTransport {
    agent: ureq::Agent,
}

impl Default for UreqModDbTransport {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(GLOBAL_TIMEOUT))
            .timeout_recv_body(Some(IDLE_TIMEOUT))
            .build();
        Self {
            agent: config.new_agent(),
        }
    }
}

impl ModDbTransport for UreqModDbTransport {
    fn get(&self, url: &str) -> Result<ModDbHttpResponse, ModProviderError> {
        validate_moddb_url(url)?;
        validate_public_https_url(url, &SystemResolver)
            .map_err(|error| ModProviderError::ProviderUnavailable(error.to_string()))?;
        let response = self
            .agent
            .get(url)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9",
            )
            .header("Accept-Encoding", "identity")
            .header(
                "User-Agent",
                concat!(
                    "EmuWiz/",
                    env!("CARGO_PKG_VERSION"),
                    " (metadata; +https://www.moddb.com)"
                ),
            )
            .call()
            .map_err(|error| ModProviderError::ProviderUnavailable(error.to_string()))?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let mut body = Vec::new();
        response
            .into_body()
            .into_reader()
            .take((MODDB_MAX_PAGE_BYTES + 1) as u64)
            .read_to_end(&mut body)
            .map_err(|error| ModProviderError::ProviderUnavailable(error.to_string()))?;
        if body.len() > MODDB_MAX_PAGE_BYTES {
            return Err(ModProviderError::ProviderUnavailable(
                "ModDB page exceeds the bounded response size".into(),
            ));
        }
        Ok(ModDbHttpResponse {
            status,
            location,
            content_type,
            body,
            retrieved_url: url.to_string(),
        })
    }
}

#[derive(Debug)]
pub struct ModDbProvider<T = UreqModDbTransport> {
    id: ModProviderId,
    transport: T,
    cache_path: Option<PathBuf>,
    cache: Mutex<ModDbCacheDocument>,
}

impl ModDbProvider<UreqModDbTransport> {
    pub fn new() -> Self {
        Self::with_transport(UreqModDbTransport::default())
    }
}

impl Default for ModDbProvider<UreqModDbTransport> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ModDbTransport> ModDbProvider<T> {
    pub fn with_transport(transport: T) -> Self {
        Self {
            id: ModProviderId::new(MODDB_PROVIDER_ID).expect("constant provider id"),
            transport,
            cache_path: None,
            cache: Mutex::new(ModDbCacheDocument::default()),
        }
    }

    pub fn with_cache_path(mut self, path: PathBuf) -> Result<Self, ModProviderError> {
        validate_cache_path(&path)?;
        let cache = if path.exists() {
            let bytes = fs::read(&path)
                .map_err(|error| ModProviderError::CorruptCache(error.to_string()))?;
            let value: ModDbCacheDocument = serde_json::from_slice(&bytes)
                .map_err(|error| ModProviderError::CorruptCache(error.to_string()))?;
            value.validate()?;
            value
        } else {
            ModDbCacheDocument::default()
        };
        self.cache_path = Some(path);
        self.cache = Mutex::new(cache);
        Ok(self)
    }

    pub fn cache(&self) -> Result<ModDbCacheDocument, ModProviderError> {
        self.cache
            .lock()
            .map(|cache| cache.clone())
            .map_err(|_| ModProviderError::ProviderUnavailable("cache lock poisoned".into()))
    }

    pub fn browser_handoff_url(&self, url: &str) -> Result<String, ModProviderError> {
        canonicalize_moddb_url(url)
    }

    pub fn inspect_url(&self, url: &str) -> Result<ModDbPageRecord, ModProviderError> {
        let canonical = canonicalize_moddb_url(url)?;
        match self.fetch_page(&canonical) {
            Ok(mut page) => {
                page.original_url = url.to_string();
                self.persist_page(&page)?;
                Ok(page)
            }
            Err(error) => self.cached_fallback(&canonical, error, true),
        }
    }

    pub fn inspect_cached(&self, url: &str) -> Result<ModDbPageRecord, ModProviderError> {
        let canonical = canonicalize_moddb_url(url)?;
        self.cached_fallback(
            &canonical,
            ModProviderError::ProviderUnavailable("offline lookup requested".into()),
            false,
        )
    }

    fn cached_fallback(
        &self,
        canonical: &str,
        error: ModProviderError,
        offline: bool,
    ) -> Result<ModDbPageRecord, ModProviderError> {
        let now = unix_now();
        let cached = self
            .cache
            .lock()
            .map_err(|_| ModProviderError::ProviderUnavailable("cache lock poisoned".into()))?
            .page(canonical, now, offline);
        cached
            .map(|mut page| {
                page.warning = Some(error.to_string());
                page
            })
            .ok_or(error)
    }

    fn fetch_page(&self, canonical: &str) -> Result<ModDbPageRecord, ModProviderError> {
        let mut current = canonical.to_string();
        let mut visited = BTreeSet::from([current.clone()]);
        for _ in 0..=MODDB_MAX_REDIRECTS {
            let response = self.transport.get(&current)?;
            if (300..400).contains(&response.status) {
                let location = response.location.ok_or_else(|| {
                    ModProviderError::BrowserRequired(
                        "ModDB redirect did not provide a usable location".into(),
                    )
                })?;
                let next = Url::parse(&current)
                    .and_then(|base| base.join(&location))
                    .map_err(|error| ModProviderError::InvalidUrl(error.to_string()))?;
                let next = canonicalize_moddb_url(next.as_str())?;
                if !visited.insert(next.clone()) {
                    return Err(ModProviderError::ProviderUnavailable(
                        "ModDB redirect loop".into(),
                    ));
                }
                current = next;
                continue;
            }
            if response.status == 403 || response.status == 401 {
                return Err(ModProviderError::BrowserRequired(
                    "ModDB requires browser access".into(),
                ));
            }
            if response.status == 429 {
                return Err(ModProviderError::RateLimited);
            }
            if response.status == 404 {
                return Err(ModProviderError::NotFound(current));
            }
            if !(200..300).contains(&response.status) {
                return Err(ModProviderError::HttpStatus(response.status));
            }
            if challenge_page(&response.body) {
                return Err(ModProviderError::Challenge(
                    "challenge, CAPTCHA, or login wall detected".into(),
                ));
            }
            let record = parse_moddb_page(
                canonical,
                &response.body,
                &response.retrieved_url,
                unix_now(),
            )?;
            return Ok(record);
        }
        Err(ModProviderError::ProviderUnavailable(
            "ModDB redirect limit exceeded".into(),
        ))
    }

    fn persist_page(&self, page: &ModDbPageRecord) -> Result<(), ModProviderError> {
        let Some(path) = &self.cache_path else {
            return Ok(());
        };
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| ModProviderError::ProviderUnavailable("cache lock poisoned".into()))?;
        cache.insert(page)?;
        cache.validate()?;
        write_cache_atomically(path, &cache)
    }
}

impl<T: ModDbTransport> ModProvider for ModDbProvider<T> {
    fn id(&self) -> &ModProviderId {
        &self.id
    }

    fn capabilities(&self) -> BTreeSet<ModProviderCapability> {
        [
            ModProviderCapability::MetadataLookup,
            ModProviderCapability::ReleaseListing,
            ModProviderCapability::BrowserHandoff,
        ]
        .into_iter()
        .collect()
    }

    fn search(&self, _query: &ModSearchQuery) -> Result<Vec<ModSearchResult>, ModProviderError> {
        Err(ModProviderError::UnsupportedCapability(
            ModProviderCapability::BrowseSearch,
        ))
    }

    fn releases(&self, item_id: &str) -> Result<Vec<ModRelease>, ModProviderError> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| ModProviderError::ProviderUnavailable("cache lock poisoned".into()))?;
        cache
            .entries
            .values()
            .find(|entry| entry.result.provider_item_id == item_id)
            .map(|entry| entry.releases.clone())
            .ok_or_else(|| ModProviderError::NotFound(item_id.into()))
    }

    fn acquisition_candidate(
        &self,
        file_id: &str,
    ) -> Result<ModDownloadCandidate, ModProviderError> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| ModProviderError::ProviderUnavailable("cache lock poisoned".into()))?;
        for entry in cache.entries.values() {
            for release in &entry.releases {
                if let Some(file) = release
                    .files
                    .iter()
                    .find(|file| file.provider_file_id == file_id)
                {
                    return Ok(file.download.clone());
                }
            }
        }
        Err(ModProviderError::NotFound(file_id.into()))
    }
}

fn parse_moddb_page(
    original_url: &str,
    body: &[u8],
    retrieved_url: &str,
    now: u64,
) -> Result<ModDbPageRecord, ModProviderError> {
    if body.len() > MODDB_MAX_PAGE_BYTES {
        return Err(ModProviderError::ProviderUnavailable(
            "ModDB page exceeds the bounded response size".into(),
        ));
    }
    let text = std::str::from_utf8(body).map_err(|error| {
        ModProviderError::ProviderUnavailable(format!("ModDB page is not UTF-8: {error}"))
    })?;
    if challenge_page(body) {
        return Err(ModProviderError::Challenge(
            "challenge, CAPTCHA, or login wall detected".into(),
        ));
    }
    let document = Html::parse_document(text);
    let canonical = canonical_link(&document).map_or_else(
        || canonicalize_moddb_url(original_url),
        |value| canonicalize_moddb_url(&value),
    )?;
    let title = first_text(&document, "meta[property='og:title']", "content")
        .or_else(|| first_text(&document, "h1", "text"))
        .or_else(|| first_text(&document, "title", "text"))
        .ok_or_else(|| ModProviderError::ProviderUnavailable("ModDB page has no title".into()))?;
    let description = first_text(&document, "meta[property='og:description']", "content")
        .or_else(|| first_text(&document, "meta[name='description']", "content"));
    let author = first_text(&document, "meta[name='author']", "content");
    let project_id = first_attr(
        &document,
        "[data-moddb-project-id]",
        "data-moddb-project-id",
    );
    let canonical_url =
        Url::parse(&canonical).map_err(|error| ModProviderError::InvalidUrl(error.to_string()))?;
    let slug = project_slug(&canonical_url);
    let item_id = project_id.clone().or_else(|| slug.clone()).ok_or_else(|| {
        ModProviderError::ProviderUnavailable(
            "ModDB page has no bounded project or page identity".into(),
        )
    })?;
    let platforms = platform_claims(&document);
    let tags = tag_values(&document);
    let page_text = document.root_element().text().collect::<Vec<_>>().join(" ");
    let filename = labelled_token(
        &page_text,
        "Filename",
        &["Category", "Licence", "Uploader", "Added", "Size"],
    );
    let reported_size = labelled_bytes(&page_text, "Size");
    let reported_md5 = labelled_token(&page_text, "MD5 Hash", &["Embed Button", "Description"])
        .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let mut platforms = platforms;
    for tag in &tags {
        let platform = IdentityPlatform::from_catalogue(Some(tag));
        if platform != IdentityPlatform::Other && !platforms.contains(&platform) {
            platforms.push(platform);
        }
    }
    let game_claim = first_attr(&document, "[data-moddb-game]", "data-moddb-game")
        .or_else(|| first_attr(&document, "[data-game-title]", "data-game-title"))
        .or_else(|| first_link_text(&document, "a[href*='/games/']"));
    let author = author.or_else(|| first_link_text(&document, "a[href*='/members/']"));
    let identity_facts = identity_facts(&document);
    let result = ModSearchResult {
        provider: ModProviderId::new(MODDB_PROVIDER_ID).expect("constant provider id"),
        provider_item_id: item_id,
        canonical_source_url: canonical.clone(),
        title: bounded(title.clone()),
        summary: description.map(bounded),
        author_or_team: author.map(bounded),
        game_claim: game_claim.clone().map(bounded),
        platform_claims: platforms.clone(),
        release: first_attr(&document, "[data-moddb-release]", "data-moddb-release"),
        version: first_attr(&document, "[data-moddb-version]", "data-moddb-version"),
        published_at_unix_secs: None,
        updated_at_unix_secs: None,
        tags,
        game_evidence: ModProviderGameEvidence {
            title_claim: game_claim,
            platform_claims: platforms,
            identity_facts,
            installation_text: first_attr(
                &document,
                "[data-moddb-installation]",
                "data-moddb-installation",
            )
            .map(bounded),
        },
    };
    let mut releases = parse_releases(&document, &canonical)?;
    if releases.is_empty()
        && (filename.is_some() || reported_md5.is_some() || reported_size.is_some())
    {
        let page_slug = project_slug(&canonical_url).unwrap_or_else(|| "release".into());
        let checksum = reported_md5.map(|value| ModCatalogueHash {
            algorithm: ModCatalogueHashAlgorithm::Md5,
            value: value.to_ascii_lowercase(),
        });
        let candidate = ModDownloadCandidate {
            canonical_source_url: canonical.clone(),
            acquisition_url: None,
            mode: ModAcquisitionMode::BrowserRequired,
            external_host: false,
            reported_filename: filename.clone(),
            reported_checksum: checksum.clone(),
        };
        releases.push(ModRelease {
            provider_release_id: page_slug.clone(),
            title: bounded(title.clone()),
            version: result.version.clone(),
            published_at_unix_secs: None,
            updated_at_unix_secs: None,
            installation_evidence: Vec::new(),
            files: vec![ModReleaseFile {
                provider: result.provider.clone(),
                provider_file_id: page_slug,
                reported_filename: filename,
                size_bytes: reported_size,
                reported_checksum: checksum,
                download: candidate,
            }],
        });
    }
    let source_fingerprint_sha256 = sha256_hex(body);
    Ok(ModDbPageRecord {
        result,
        releases,
        original_url: original_url.into(),
        canonical_url: canonical,
        retrieved_url: retrieved_url.into(),
        retrieved_at_unix_secs: now,
        source_fingerprint_sha256,
        cache_state: ModDbCacheState::Fresh,
        warning: None,
    })
}

fn parse_releases(document: &Html, canonical: &str) -> Result<Vec<ModRelease>, ModProviderError> {
    let selector = Selector::parse("[data-moddb-release-id]").expect("constant selector");
    let file_selector = Selector::parse("[data-moddb-file-id]").expect("constant selector");
    let mut releases = Vec::new();
    for release_node in document.select(&selector).take(MODDB_MAX_RELEASES) {
        let release_id = release_node
            .value()
            .attr("data-moddb-release-id")
            .unwrap_or("release")
            .to_string();
        let mut files = Vec::new();
        for file_node in release_node.select(&file_selector).take(MODDB_MAX_FILES) {
            files.push(parse_file(file_node, canonical));
        }
        releases.push(ModRelease {
            provider_release_id: release_id,
            title: attr_value(release_node, "data-moddb-release-title")
                .unwrap_or_else(|| "ModDB release".into()),
            version: attr_value(release_node, "data-moddb-version"),
            published_at_unix_secs: None,
            updated_at_unix_secs: None,
            installation_evidence: attr_value(release_node, "data-moddb-installation")
                .into_iter()
                .collect(),
            files,
        });
    }
    if releases.is_empty() {
        let file_nodes = document
            .select(&file_selector)
            .take(MODDB_MAX_FILES)
            .collect::<Vec<_>>();
        if !file_nodes.is_empty() {
            releases.push(ModRelease {
                provider_release_id: project_slug(&Url::parse(canonical).expect("canonical URL"))
                    .unwrap_or_else(|| "release".into()),
                title: "ModDB file listing".into(),
                version: None,
                published_at_unix_secs: None,
                updated_at_unix_secs: None,
                installation_evidence: Vec::new(),
                files: file_nodes
                    .into_iter()
                    .map(|node| parse_file(node, canonical))
                    .collect(),
            });
        }
    }
    Ok(releases)
}

fn parse_file(value: scraper::ElementRef<'_>, canonical: &str) -> ModReleaseFile {
    let file_id = attr_value(value, "data-moddb-file-id").unwrap_or_else(|| "file".into());
    let filename = attr_value(value, "data-moddb-filename");
    let size_bytes = attr_value(value, "data-moddb-size").and_then(|value| value.parse().ok());
    let checksum = attr_value(value, "data-moddb-md5")
        .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(|value| ModCatalogueHash {
            algorithm: ModCatalogueHashAlgorithm::Md5,
            value: value.to_ascii_lowercase(),
        });
    let page = attr_value(value, "data-moddb-url").unwrap_or_else(|| canonical.into());
    let candidate = ModDownloadCandidate {
        canonical_source_url: page,
        acquisition_url: None,
        mode: ModAcquisitionMode::BrowserRequired,
        external_host: false,
        reported_filename: filename.clone(),
        reported_checksum: checksum.clone(),
    };
    ModReleaseFile {
        provider: ModProviderId::new(MODDB_PROVIDER_ID).expect("constant provider id"),
        provider_file_id: file_id,
        reported_filename: filename,
        size_bytes,
        reported_checksum: checksum,
        download: candidate,
    }
}

fn first_text(document: &Html, selector: &str, attribute: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    let node = document.select(&selector).next()?;
    if attribute == "text" {
        return Some(node.text().collect::<String>().trim().to_string())
            .filter(|value| !value.is_empty());
    }
    node.value()
        .attr(attribute)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn first_attr(document: &Html, selector: &str, attribute: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document
        .select(&selector)
        .next()
        .and_then(|node| node.value().attr(attribute))
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn first_link_text(document: &Html, selector: &str) -> Option<String> {
    let selector = Selector::parse(selector).ok()?;
    document
        .select(&selector)
        .map(|node| node.text().collect::<String>().trim().to_string())
        .find(|value| !value.is_empty() && value.len() <= MAX_PROVIDER_TEXT_BYTES)
}

fn attr_value(value: scraper::ElementRef<'_>, attribute: &str) -> Option<String> {
    value
        .value()
        .attr(attribute)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn canonical_link(document: &Html) -> Option<String> {
    first_attr(document, "link[rel='canonical']", "href")
}

fn project_slug(url: &Url) -> Option<String> {
    let mut segments = url.path_segments()?;
    let kind = segments.next()?;
    if !matches!(kind, "mods" | "addons" | "downloads" | "games") {
        return None;
    }
    segments
        .next()
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

fn platform_claims(document: &Html) -> Vec<IdentityPlatform> {
    let mut values = Vec::new();
    for selector in ["[data-moddb-platform]", "[data-platform]"] {
        if let Ok(selector) = Selector::parse(selector) {
            for node in document.select(&selector) {
                let attribute = node
                    .value()
                    .attr("data-moddb-platform")
                    .or_else(|| node.value().attr("data-platform"))
                    .unwrap_or("");
                for value in attribute.split([',', ';']) {
                    let platform = IdentityPlatform::from_catalogue(Some(value.trim()));
                    if platform != IdentityPlatform::Other && !values.contains(&platform) {
                        values.push(platform);
                    }
                }
            }
        }
    }
    values
}

fn tag_values(document: &Html) -> Vec<String> {
    let selector =
        Selector::parse("[data-moddb-tag], a[href*='/tags/']").expect("constant selector");
    document
        .select(&selector)
        .take(MODDB_MAX_TAGS)
        .filter_map(|node| {
            node.value()
                .attr("data-moddb-tag")
                .map(str::to_string)
                .or_else(|| Some(node.text().collect::<String>()))
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn labelled_token(text: &str, label: &str, terminators: &[&str]) -> Option<String> {
    let start = text
        .to_ascii_lowercase()
        .find(&label.to_ascii_lowercase())?
        + label.len();
    let remainder = text[start..].trim();
    let end = terminators
        .iter()
        .filter_map(|terminator| {
            remainder
                .to_ascii_lowercase()
                .find(&terminator.to_ascii_lowercase())
        })
        .min()
        .unwrap_or(remainder.len());
    remainder[..end]
        .split_whitespace()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn labelled_bytes(text: &str, label: &str) -> Option<u64> {
    let start = text
        .to_ascii_lowercase()
        .find(&label.to_ascii_lowercase())?
        + label.len();
    let remainder = &text[start..];
    let marker = "bytes";
    let end = remainder.to_ascii_lowercase().find(marker)?;
    let mut digits = String::new();
    let mut started = false;
    for character in remainder[..end].chars().rev() {
        if character.is_ascii_digit() || (started && character == ',') {
            started = true;
            digits.push(character);
        } else if started {
            break;
        }
    }
    let digits = digits.chars().rev().collect::<String>();
    digits.replace(',', "").parse().ok()
}

fn identity_facts(document: &Html) -> Vec<ModProviderIdentityFact> {
    let selector = Selector::parse("[data-moddb-identity-kind][data-moddb-identity-value]")
        .expect("constant selector");
    document.select(&selector).take(MODDB_MAX_GAME_FACTS).filter_map(|node| {
        let kind = match node.value().attr("data-moddb-identity-kind")?.to_ascii_lowercase().as_str() {
            "ps1_serial" => IdentityKind::Ps1Serial,
            "ps2_serial" => IdentityKind::Ps2Serial,
            "psp_disc_id" => IdentityKind::PspDiscId,
            "ps3_title_id" => IdentityKind::Ps3TitleId,
            "dolphin_game_id" => IdentityKind::DolphinGameId,
            "dreamcast_product_code" => IdentityKind::DreamcastProductCode,
            "pcsx2_executable_crc" => IdentityKind::Pcsx2ExecutableCrc,
            _ => return None,
        };
        let value = node.value().attr("data-moddb-identity-value")?.trim();
        (!value.is_empty()).then(|| ModProviderIdentityFact { kind, value: value.into(), source: ModProviderEvidenceSource::ProviderMetadata, detail: "explicit ModDB page identity field; local package verification remains authoritative".into() })
    }).collect()
}

fn validate_moddb_url(value: &str) -> Result<(), ModProviderError> {
    let url = Url::parse(value).map_err(|error| ModProviderError::InvalidUrl(error.to_string()))?;
    if url.scheme() != "https" {
        return Err(ModProviderError::InvalidUrl(
            "ModDB metadata requires HTTPS".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ModProviderError::InvalidUrl(
            "credentials are not allowed in ModDB URLs".into(),
        ));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "moddb.com" | "www.moddb.com" | "files.moddb.com" | "media.moddb.com"
    ) {
        return Err(ModProviderError::InvalidUrl(
            "host is not an approved ModDB host".into(),
        ));
    }
    if url.path().is_empty() || url.path().split('/').any(|part| part == "..") {
        return Err(ModProviderError::InvalidUrl(
            "path is not a safe ModDB path".into(),
        ));
    }
    Ok(())
}

pub fn canonicalize_moddb_url(value: &str) -> Result<String, ModProviderError> {
    validate_moddb_url(value)?;
    let mut url =
        Url::parse(value).map_err(|error| ModProviderError::InvalidUrl(error.to_string()))?;
    url.set_fragment(None);
    url.set_query(None);
    if url.path().len() > MAX_PROVIDER_TEXT_BYTES {
        return Err(ModProviderError::InvalidUrl("path is too long".into()));
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(if path.is_empty() { "/" } else { &path });
    Ok(url.to_string())
}

fn challenge_page(body: &[u8]) -> bool {
    let sample = String::from_utf8_lossy(&body[..body.len().min(16 * 1024)]).to_ascii_lowercase();
    [
        "captcha",
        "cloudflare",
        "verify you are human",
        "access denied",
        "login required",
    ]
    .iter()
    .any(|needle| sample.contains(needle))
}

fn bounded(value: String) -> String {
    value.chars().take(MAX_PROVIDER_TEXT_BYTES).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn validate_cache_path(path: &Path) -> Result<(), ModProviderError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(ModProviderError::InvalidUrl(
            "cache path must be absolute and traversal-free".into(),
        ));
    }
    if path.exists()
        && fs::symlink_metadata(path)
            .map(|meta| meta.file_type().is_symlink() || !meta.is_file())
            .unwrap_or(true)
    {
        return Err(ModProviderError::CorruptCache(
            "cache path is not a regular file".into(),
        ));
    }
    if let Some(parent) = path.parent()
        && parent.exists()
        && fs::symlink_metadata(parent)
            .map(|meta| meta.file_type().is_symlink() || !meta.is_dir())
            .unwrap_or(true)
    {
        return Err(ModProviderError::CorruptCache(
            "cache parent is not a real directory".into(),
        ));
    }
    Ok(())
}

fn write_cache_atomically(path: &Path, cache: &ModDbCacheDocument) -> Result<(), ModProviderError> {
    validate_cache_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| ModProviderError::CorruptCache("cache has no parent".into()))?;
    fs::create_dir_all(parent)
        .map_err(|error| ModProviderError::CorruptCache(error.to_string()))?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(cache)
        .map_err(|error| ModProviderError::CorruptCache(error.to_string()))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| ModProviderError::CorruptCache(error.to_string()))?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(ModProviderError::CorruptCache(error.to_string()));
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(ModProviderError::CorruptCache(error.to_string()));
    }
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tempfile::tempdir;

    #[derive(Debug)]
    struct FakeTransport(Mutex<VecDeque<Result<ModDbHttpResponse, ModProviderError>>>);

    impl FakeTransport {
        fn page(body: &str) -> Self {
            Self(Mutex::new(VecDeque::from([Ok(ModDbHttpResponse {
                status: 200,
                location: None,
                content_type: Some("text/html".into()),
                body: body.as_bytes().to_vec(),
                retrieved_url: "https://www.moddb.com/mods/darkwatch".into(),
            })])))
        }
        fn response(response: ModDbHttpResponse) -> Self {
            Self(Mutex::new(VecDeque::from([Ok(response)])))
        }
    }

    impl ModDbTransport for FakeTransport {
        fn get(&self, _url: &str) -> Result<ModDbHttpResponse, ModProviderError> {
            self.0.lock().unwrap().pop_front().unwrap_or_else(|| {
                Err(ModProviderError::ProviderUnavailable(
                    "no fixture response".into(),
                ))
            })
        }
    }

    const FIXTURE: &str = r#"<!doctype html><html><head>
      <link rel="canonical" href="https://www.moddb.com/mods/darkwatch/">
      <meta property="og:title" content="Darkwatch Texture Pack"><meta name="description" content="Synthetic PCSX2 texture release">
      </head><body><h1>Darkwatch Texture Pack</h1>
      <div data-moddb-project-id="darkwatch" data-moddb-game="Darkwatch" data-moddb-platform="PS2" data-moddb-tag="texture" data-moddb-identity-kind="ps2_serial" data-moddb-identity-value="SLES-53564">
        <section data-moddb-release-id="sles-release" data-moddb-release-title="SLES release" data-moddb-version="1.0" data-moddb-installation="Use the existing local package inspector.">
          <a data-moddb-file-id="sles-file" data-moddb-filename="SLES-53564.rar" data-moddb-size="1234" data-moddb-md5="0123456789abcdef0123456789abcdef"></a>
        </section></div></body></html>"#;

    #[test]
    fn parses_metadata_release_file_identity_and_browser_handoff() {
        let provider = ModDbProvider::with_transport(FakeTransport::page(FIXTURE));
        let page = provider
            .inspect_url("https://www.moddb.com/mods/darkwatch/?utm_source=test")
            .unwrap();
        assert_eq!(page.canonical_url, "https://www.moddb.com/mods/darkwatch");
        assert_eq!(page.result.provider_item_id, "darkwatch");
        assert_eq!(
            page.result.platform_claims,
            vec![IdentityPlatform::PlayStation2]
        );
        assert_eq!(
            page.result.game_evidence.identity_facts[0].kind,
            IdentityKind::Ps2Serial
        );
        assert_eq!(
            page.releases[0].files[0].reported_filename.as_deref(),
            Some("SLES-53564.rar")
        );
        assert_eq!(
            page.releases[0].files[0].download.mode,
            ModAcquisitionMode::BrowserRequired
        );
        assert_eq!(
            provider
                .browser_handoff_url("https://www.moddb.com/mods/darkwatch/")
                .unwrap(),
            page.canonical_url
        );
        assert!(
            provider
                .search(&ModSearchQuery {
                    text: Some("darkwatch".into()),
                    limit: 1,
                    ..Default::default()
                })
                .is_err()
        );
    }

    #[test]
    fn url_validation_rejects_credentials_schemes_hosts_and_private_redirects() {
        for url in [
            "javascript:alert(1)",
            "file:///tmp/mod",
            "https://evil-moddb.com/mods/x",
            "https://user:pass@www.moddb.com/mods/x",
        ] {
            assert!(canonicalize_moddb_url(url).is_err(), "accepted {url}");
        }
        let provider = ModDbProvider::with_transport(FakeTransport::response(ModDbHttpResponse {
            status: 302,
            location: Some("https://evil.example/mods/x".into()),
            content_type: None,
            body: Vec::new(),
            retrieved_url: "https://www.moddb.com/mods/x".into(),
        }));
        assert!(matches!(
            provider.inspect_url("https://www.moddb.com/mods/x"),
            Err(ModProviderError::InvalidUrl(_))
        ));
    }

    #[test]
    fn challenge_rate_limit_and_not_found_are_structured() {
        let challenge = ModDbProvider::with_transport(FakeTransport::response(ModDbHttpResponse {
            status: 200,
            location: None,
            content_type: Some("text/html".into()),
            body: b"<title>Cloudflare challenge</title>".to_vec(),
            retrieved_url: "https://www.moddb.com/mods/x".into(),
        }));
        assert!(matches!(
            challenge.inspect_url("https://www.moddb.com/mods/x"),
            Err(ModProviderError::Challenge(_))
        ));
        let rate = ModDbProvider::with_transport(FakeTransport::response(ModDbHttpResponse {
            status: 429,
            location: None,
            content_type: None,
            body: Vec::new(),
            retrieved_url: "https://www.moddb.com/mods/x".into(),
        }));
        assert_eq!(
            rate.inspect_url("https://www.moddb.com/mods/x")
                .unwrap_err(),
            ModProviderError::RateLimited
        );
        let missing = ModDbProvider::with_transport(FakeTransport::response(ModDbHttpResponse {
            status: 404,
            location: None,
            content_type: None,
            body: Vec::new(),
            retrieved_url: "https://www.moddb.com/mods/x".into(),
        }));
        assert!(matches!(
            missing.inspect_url("https://www.moddb.com/mods/x"),
            Err(ModProviderError::NotFound(_))
        ));
    }

    #[test]
    fn cache_is_bounded_atomic_and_offline_usable() {
        let directory = tempdir().unwrap();
        let cache_path = directory.path().join("moddb.json");
        let provider = ModDbProvider::with_transport(FakeTransport::page(FIXTURE))
            .with_cache_path(cache_path.clone())
            .unwrap();
        let page = provider
            .inspect_url("https://www.moddb.com/mods/darkwatch")
            .unwrap();
        assert_eq!(page.cache_state, ModDbCacheState::Fresh);
        assert!(cache_path.is_file());
        let offline = ModDbProvider::with_transport(FakeTransport::response(ModDbHttpResponse {
            status: 503,
            location: None,
            content_type: None,
            body: Vec::new(),
            retrieved_url: "https://www.moddb.com/mods/darkwatch".into(),
        }))
        .with_cache_path(cache_path)
        .unwrap()
        .inspect_url("https://www.moddb.com/mods/darkwatch")
        .unwrap();
        assert_eq!(offline.cache_state, ModDbCacheState::Offline);
        assert!(offline.warning.is_some());
    }

    #[test]
    fn filename_only_and_ps2_tag_do_not_create_identity_fact() {
        let body = r#"<html><head><title>PS2 Texture</title></head><body><div data-moddb-platform="PS2" data-moddb-tag="ps2"></div><a data-moddb-file-id="f" data-moddb-filename="DARKWATCH.rar"></a></body></html>"#;
        let page = ModDbProvider::with_transport(FakeTransport::page(body))
            .inspect_url("https://www.moddb.com/addons/ps2-texture")
            .unwrap();
        assert!(page.result.game_evidence.identity_facts.is_empty());
        assert_eq!(
            page.result.game_evidence.platform_claims,
            vec![IdentityPlatform::PlayStation2]
        );
    }

    #[test]
    fn parses_bounded_human_labelled_addon_fields_without_download_route() {
        let body = r#"<html><head><title>SLES 53564 addon - Darkwatch Texture Pack</title></head>
          <body><h1>SLES 53564 addon - Darkwatch Texture Pack</h1>
          <a href="/games/darkwatch">Darkwatch</a><a href="/tags/ps2">ps2</a>
          <div>Filename SLES-53564.rar Category Texture Licence Proprietary Uploader fixture Added Jan 1st, 2026 Size 3.12gb (3,353,127,342 bytes) MD5 Hash a2789929816064d679306c62d53a3823 Embed Button Description</div>
          </body></html>"#;
        let page = ModDbProvider::with_transport(FakeTransport::page(body))
            .inspect_url("https://www.moddb.com/mods/darkwatch/addons/sles-53564")
            .unwrap();
        let file = &page.releases[0].files[0];
        assert_eq!(page.result.game_claim.as_deref(), Some("Darkwatch"));
        assert_eq!(file.reported_filename.as_deref(), Some("SLES-53564.rar"));
        assert_eq!(file.size_bytes, Some(3_353_127_342));
        assert_eq!(
            file.reported_checksum
                .as_ref()
                .map(|hash| hash.value.as_str()),
            Some("a2789929816064d679306c62d53a3823")
        );
        assert!(file.download.acquisition_url.is_none());
    }
}
