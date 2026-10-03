//! Live, read-only browsing through the existing approved RomM client.
//!
//! All projections are external hints. Nothing here imports a catalogue,
//! verifies local identity, fetches artwork bytes, or publishes mutations.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::capability::{MINIMUM_SUPPORTED_MAJOR, RommApiCapability, RommHeartbeat};
use super::client::{MAX_PAGE_SIZE, REQUEST_TIMEOUT, RommClient, RommRequestError, RommTransport};
use super::config::ValidatedRommSource;
use super::connectivity::RommConnectivity;
use super::normalise::{
    MAX_RELATED_FILES, NormalisationReport, canonical_platform_for_romm_slug, normalise_platform,
    normalise_rom,
};
use crate::identity_source::model::{
    ArtworkReference, ExternalHash, ExternalIdentityRecord, HashAlgorithm, IdentityProvider,
    MediaReference,
};

pub const MAX_BROWSE_STRING_BYTES: usize = 8192;
pub const MAX_SEARCH_BYTES: usize = 512;
pub const MAX_BROWSE_PLATFORMS: usize = 1024;
pub const MAX_BROWSE_DEPTH: usize = 32;
const MAX_DOCUMENT_NODES: usize = 100_000;
const MAX_STUDIED_MAJOR: u32 = 5;

/// No error carries a response body, credential, URL, or transport message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RommBrowseError {
    Unreachable,
    Timeout,
    Tls,
    AuthRequired,
    AuthFailed,
    EndpointRefused,
    Cancelled,
    HttpClient { status: u16 },
    HttpServer { status: u16 },
    RateLimited,
    ResponseTooLarge { limit: usize },
    MalformedResponse,
    UnsupportedVersion,
    UnsupportedCapability,
    UnsupportedFilter,
    DiscoveryRequired,
    PaginationInconsistency,
    SchemaIncompatibility,
    InvalidRequest,
    LimitExceeded,
}

impl std::fmt::Display for RommBrowseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RomM browse request refused: {self:?}")
    }
}

impl std::error::Error for RommBrowseError {}

impl From<RommRequestError> for RommBrowseError {
    fn from(error: RommRequestError) -> Self {
        match error {
            RommRequestError::Endpoint(_) => Self::EndpointRefused,
            RommRequestError::Unauthorised { .. } => Self::AuthFailed,
            RommRequestError::HttpStatus { status } | RommRequestError::RateLimited { status } => {
                match status {
                    429 => Self::RateLimited,
                    500..=599 => Self::HttpServer { status },
                    _ => Self::HttpClient { status },
                }
            }
            RommRequestError::ResponseTooLarge { limit } => Self::ResponseTooLarge { limit },
            RommRequestError::MalformedResponse { .. } => Self::MalformedResponse,
            RommRequestError::Timeout => Self::Timeout,
            RommRequestError::Cancelled => Self::Cancelled,
            error @ RommRequestError::Transport { .. } => {
                match RommConnectivity::from_request_error(&error) {
                    Some(RommConnectivity::Timeout) => Self::Timeout,
                    Some(RommConnectivity::TlsFailure) => Self::Tls,
                    _ => Self::Unreachable,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RommServerStatus {
    Supported,
    PartiallySupported,
    UnsupportedVersion,
    Unreachable,
    AuthRequired,
    AuthFailed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommBrowseCapabilities {
    pub platforms: bool,
    pub games: bool,
    pub game_detail: bool,
    pub text_search: bool,
    pub platform_filter: bool,
    pub omit_files: bool,
    pub omit_char_index: bool,
    pub omit_filter_values: bool,
    pub omit_rom_id_index: bool,
    pub disable_grouping: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommServerInfo {
    pub server_id: String,
    pub version: Option<String>,
    pub status: RommServerStatus,
    pub capabilities: RommBrowseCapabilities,
    pub declared_read_scopes: Vec<String>,
    pub error: Option<RommBrowseError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RommPlatformMapping {
    Exact { canonical: String },
    KnownAlias { canonical: String },
    Ambiguous { candidates: Vec<String> },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommPlatformSummary {
    pub id: u64,
    pub name: Option<String>,
    pub slug: String,
    pub filesystem_slug: Option<String>,
    pub game_count: Option<u64>,
    pub system_identifiers: BTreeMap<String, String>,
    pub mapping: RommPlatformMapping,
    pub provenance: RommBrowseProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommBrowseProvenance {
    pub provider: IdentityProvider,
    pub server_id: String,
    /// One fixed read endpoint, without credentials or search text.
    pub endpoint: String,
    pub observed_at_unix_seconds: i64,
}

/// The existing canonical external-identity model, still Unmatched.
pub type RommIdentityHint = ExternalIdentityRecord;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommArtworkRef {
    pub cover: Option<ArtworkReference>,
    pub screenshots: Vec<MediaReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommGameSummary {
    pub id: u64,
    pub filename: Option<String>,
    pub created_at: Option<String>,
    pub platform_mapping: RommPlatformMapping,
    pub identity: RommIdentityHint,
    pub artwork: RommArtworkRef,
    pub normalisation: NormalisationReport,
    /// Pages omit file lists; a selected detail includes them.
    pub includes_file_detail: bool,
    pub provenance: RommBrowseProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommRelatedFile {
    pub id: u64,
    pub filename: Option<String>,
    pub provider_path: Option<String>,
    pub size_bytes: Option<u64>,
    pub hashes: Vec<ExternalHash>,
    pub rejected_hashes: Vec<HashAlgorithm>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommGameDetail {
    pub game: RommGameSummary,
    pub files: Vec<RommRelatedFile>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommBrowseFilter {
    pub text: Option<String>,
    pub platform_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommBrowsePage {
    pub games: Vec<RommGameSummary>,
    pub offset: u32,
    pub page_size: u32,
    pub total: Option<u64>,
    pub next_offset: Option<u32>,
    pub previous_offset: Option<u32>,
}

/// Holds only the established source/client and the last explicit discovery.
pub struct RommBrowser<'a, T: RommTransport> {
    source: &'a ValidatedRommSource,
    client: RommClient<'a, T>,
    observed_at: i64,
    info: Option<RommServerInfo>,
}

impl<T: RommTransport> std::fmt::Debug for RommBrowser<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RommBrowser")
            .field("info", &self.info)
            .finish()
    }
}

impl<'a, T: RommTransport> RommBrowser<'a, T> {
    /// Construction performs no I/O. Discovery is explicit and can be cancelled.
    pub fn new(source: &'a ValidatedRommSource, transport: &'a T, observed_at: i64) -> Self {
        Self {
            source,
            client: RommClient::new(source, transport),
            observed_at,
            info: None,
        }
    }

    pub fn server_info(&self) -> Option<&RommServerInfo> {
        self.info.as_ref()
    }

    /// Two public documents plus at most one authenticated, one-item page.
    /// No retries, whole-library import, or artwork/ROM fetch.
    pub fn discover(&mut self, cancel: Option<&AtomicBool>) -> RommServerInfo {
        let mut info = RommServerInfo {
            server_id: self.source.server_id().to_owned(),
            version: None,
            status: RommServerStatus::PartiallySupported,
            capabilities: RommBrowseCapabilities::default(),
            declared_read_scopes: Vec::new(),
            error: None,
        };
        let result = self.discover_inner(&mut info, cancel);
        if let Err(error) = result {
            info.status = match error {
                RommBrowseError::UnsupportedVersion => RommServerStatus::UnsupportedVersion,
                RommBrowseError::AuthRequired => RommServerStatus::AuthRequired,
                RommBrowseError::AuthFailed => RommServerStatus::AuthFailed,
                RommBrowseError::Unreachable | RommBrowseError::Timeout | RommBrowseError::Tls => {
                    RommServerStatus::Unreachable
                }
                _ => RommServerStatus::PartiallySupported,
            };
            info.error = Some(error);
        }
        self.info = Some(info.clone());
        info
    }

    fn discover_inner(
        &self,
        info: &mut RommServerInfo,
        cancel: Option<&AtomicBool>,
    ) -> Result<(), RommBrowseError> {
        // Some installations hide heartbeat; only a 404 permits OpenAPI fallback.
        match self.read("/api/heartbeat", false, cancel) {
            Ok(document) => {
                let heartbeat = RommHeartbeat::parse(&document)
                    .ok_or(RommBrowseError::SchemaIncompatibility)?;
                match check_version(&heartbeat.version) {
                    Ok(()) => info.version = Some(heartbeat.version),
                    Err(error @ RommBrowseError::UnsupportedVersion) => {
                        info.version = Some(heartbeat.version);
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(RommBrowseError::HttpClient { status: 404 }) => {}
            Err(error) => return Err(error),
        }
        let document = self.read("/openapi.json", false, cancel)?;
        if !document.get("paths").is_some_and(Value::is_object)
            || !document
                .get("openapi")
                .and_then(Value::as_str)
                .is_some_and(|v| v.starts_with("3."))
        {
            return Err(RommBrowseError::SchemaIncompatibility);
        }
        let api = RommApiCapability::from_openapi(&document);
        if let Some(version) = &api.api_version {
            check_version(version)?;
            if info
                .version
                .as_ref()
                .is_some_and(|v| major(v) != major(version))
            {
                return Err(RommBrowseError::SchemaIncompatibility);
            }
            info.version.get_or_insert_with(|| version.clone());
        }
        if info.version.is_none() {
            return Err(RommBrowseError::SchemaIncompatibility);
        }
        info.declared_read_scopes = api.declared_read_scopes;
        info.capabilities = capabilities(&document)?;
        let caps = &info.capabilities;
        info.status = if caps.platforms
            && caps.games
            && caps.game_detail
            && caps.text_search
            && caps.platform_filter
        {
            RommServerStatus::Supported
        } else {
            RommServerStatus::PartiallySupported
        };
        if caps.games {
            let path = page_path(caps, 1, 0, &RommBrowseFilter::default())?;
            let page = self.read(&path, true, cancel)?;
            self.project_page(&page, 1, 0, &RommBrowseFilter::default())?;
        } else if caps.platforms {
            self.project_platforms(&self.read("/api/platforms", true, cancel)?)?;
        }
        Ok(())
    }

    pub fn platforms(
        &self,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<RommPlatformSummary>, RommBrowseError> {
        if !self.ready()?.capabilities.platforms {
            return Err(RommBrowseError::UnsupportedCapability);
        }
        self.project_platforms(&self.read("/api/platforms", true, cancel)?)
    }

    pub fn games(
        &self,
        offset: u32,
        page_size: u32,
        filter: &RommBrowseFilter,
        cancel: Option<&AtomicBool>,
    ) -> Result<RommBrowsePage, RommBrowseError> {
        let caps = &self.ready()?.capabilities;
        if !caps.games {
            return Err(RommBrowseError::UnsupportedCapability);
        }
        if !(1..=MAX_PAGE_SIZE).contains(&page_size) {
            return Err(RommBrowseError::InvalidRequest);
        }
        let path = page_path(caps, page_size, offset, filter)?;
        self.project_page(&self.read(&path, true, cancel)?, page_size, offset, filter)
    }

    pub fn game_detail(
        &self,
        id: u64,
        cancel: Option<&AtomicBool>,
    ) -> Result<RommGameDetail, RommBrowseError> {
        check_id(id)?;
        if !self.ready()?.capabilities.game_detail {
            return Err(RommBrowseError::UnsupportedCapability);
        }
        let path = format!("/api/roms/{id}");
        let value = self.read(&path, true, cancel)?;
        let game = self.project_game(&value, &path, true)?;
        if game.id != id {
            return Err(RommBrowseError::SchemaIncompatibility);
        }
        let files = value
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(project_file)
            .collect::<Result<Vec<_>, _>>()?;
        if files.iter().map(|f| f.id).collect::<BTreeSet<_>>().len() != files.len() {
            return Err(RommBrowseError::SchemaIncompatibility);
        }
        Ok(RommGameDetail { game, files })
    }

    fn ready(&self) -> Result<&RommServerInfo, RommBrowseError> {
        let info = self
            .info
            .as_ref()
            .ok_or(RommBrowseError::DiscoveryRequired)?;
        if let Some(error) = &info.error {
            return Err(error.clone());
        }
        Ok(info)
    }

    fn read(
        &self,
        path: &str,
        authenticated: bool,
        cancel: Option<&AtomicBool>,
    ) -> Result<Value, RommBrowseError> {
        let value = self
            .client
            .get_json(path, authenticated, REQUEST_TIMEOUT, cancel)
            .map_err(|e| {
                if !authenticated && matches!(e, RommRequestError::Unauthorised { .. }) {
                    RommBrowseError::AuthRequired
                } else {
                    e.into()
                }
            })?;
        let mut nodes = 0;
        check_document(&value, 0, MAX_BROWSE_PLATFORMS, &mut nodes)?;
        Ok(value)
    }

    fn provenance(&self, endpoint: &str) -> RommBrowseProvenance {
        RommBrowseProvenance {
            provider: IdentityProvider::Romm,
            server_id: self.source.server_id().to_owned(),
            endpoint: endpoint.to_owned(),
            observed_at_unix_seconds: self.observed_at,
        }
    }

    fn project_platforms(
        &self,
        document: &Value,
    ) -> Result<Vec<RommPlatformSummary>, RommBrowseError> {
        let items = document
            .as_array()
            .ok_or(RommBrowseError::SchemaIncompatibility)?;
        let mut ids = BTreeSet::new();
        items
            .iter()
            .map(|value| {
                let id = required_id(value, "id")?;
                if !ids.insert(id) {
                    return Err(RommBrowseError::SchemaIncompatibility);
                }
                for field in ["slug", "fs_slug", "name", "display_name"] {
                    optional_string(value, field)?;
                }
                let normal =
                    normalise_platform(value).ok_or(RommBrowseError::SchemaIncompatibility)?;
                let fs_slug = optional_string(value, "fs_slug")?;
                let mapping = platform_mapping(
                    normal.canonical.as_deref(),
                    &normal.provider_slug,
                    fs_slug.as_deref(),
                );
                let mut system_identifiers = BTreeMap::new();
                for field in [
                    "igdb_id",
                    "moby_id",
                    "ss_id",
                    "ra_id",
                    "sgdb_id",
                    "launchbox_id",
                    "hasheous_id",
                    "tgdb_id",
                    "igdb_slug",
                    "moby_slug",
                    "libretro_slug",
                ] {
                    if let Some(v) = value.get(field).filter(|v| !v.is_null()) {
                        let text = if let Some(text) = v.as_str() {
                            text.to_owned()
                        } else if let Some(id) = v.as_u64() {
                            id.to_string()
                        } else {
                            return Err(RommBrowseError::SchemaIncompatibility);
                        };
                        system_identifiers.insert(field.to_owned(), text);
                    }
                }
                Ok(RommPlatformSummary {
                    id,
                    name: optional_string(value, "display_name")?.or(normal.provider_name),
                    slug: normal.provider_slug,
                    filesystem_slug: fs_slug,
                    game_count: optional_u64(value, "rom_count")?
                        .or(optional_u64(value, "roms_count")?),
                    system_identifiers,
                    mapping,
                    provenance: self.provenance("/api/platforms"),
                })
            })
            .collect()
    }

    fn project_game(
        &self,
        value: &Value,
        endpoint: &str,
        detail: bool,
    ) -> Result<RommGameSummary, RommBrowseError> {
        let mut nodes = 0;
        check_document(value, 0, MAX_RELATED_FILES, &mut nodes)?;
        let id = required_id(value, "id")?;
        if let Some(platform) = optional_u64(value, "platform_id")? {
            check_id(platform)?;
        }
        optional_u64(value, "fs_size_bytes")?;
        for field in [
            "name",
            "platform_slug",
            "platform_fs_slug",
            "platform_display_name",
            "fs_name",
            "fs_path",
            "full_path",
            "revision",
            "crc_hash",
            "md5_hash",
            "sha1_hash",
            "updated_at",
            "created_at",
            "path_cover_small",
            "path_cover_large",
            "url_cover",
            "screenshot_path",
        ] {
            optional_string(value, field)?;
        }
        for field in ["regions", "merged_screenshots"] {
            check_string_list(value, field)?;
        }
        for field in ["files", "sibling_roms"] {
            if value
                .get(field)
                .is_some_and(|v| !v.is_null() && !v.is_array())
            {
                return Err(RommBrowseError::SchemaIncompatibility);
            }
        }
        let mut normalisation = NormalisationReport::default();
        let identity = normalise_rom(
            value,
            self.source.server_id(),
            self.source.mappings(),
            self.observed_at,
            &mut normalisation,
        )
        .ok_or(RommBrowseError::SchemaIncompatibility)?;
        let slug = optional_string(value, "platform_slug")?.unwrap_or_default();
        let fs_slug = optional_string(value, "platform_fs_slug")?;
        let platform_mapping = platform_mapping(
            identity.platform_candidate.as_deref(),
            &slug,
            fs_slug.as_deref(),
        );
        let mut artwork = RommArtworkRef {
            cover: identity.artwork.clone(),
            screenshots: identity
                .artwork
                .as_ref()
                .map(|a| a.screenshots.clone())
                .unwrap_or_default(),
        };
        let references = value
            .get("merged_screenshots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain(value.get("screenshot_path").and_then(Value::as_str));
        for reference in references.filter(|r| !r.is_empty()) {
            let media = if reference.starts_with("https://") || reference.starts_with("http://") {
                MediaReference {
                    hosted_reference: None,
                    public_reference: Some(reference.to_owned()),
                }
            } else {
                MediaReference {
                    hosted_reference: Some(reference.to_owned()),
                    public_reference: None,
                }
            };
            if !artwork.screenshots.contains(&media) {
                artwork.screenshots.push(media);
            }
        }
        if artwork.screenshots.len() > MAX_RELATED_FILES {
            return Err(RommBrowseError::LimitExceeded);
        }
        Ok(RommGameSummary {
            id,
            filename: optional_string(value, "fs_name")?,
            created_at: optional_string(value, "created_at")?,
            platform_mapping,
            identity,
            artwork,
            normalisation,
            includes_file_detail: detail,
            provenance: self.provenance(endpoint),
        })
    }

    fn project_page(
        &self,
        value: &Value,
        limit: u32,
        offset: u32,
        filter: &RommBrowseFilter,
    ) -> Result<RommBrowsePage, RommBrowseError> {
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .ok_or(RommBrowseError::SchemaIncompatibility)?;
        for (field, expected) in [("limit", limit), ("offset", offset)] {
            if let Some(reported) = value.get(field)
                && reported.as_u64() != Some(u64::from(expected))
            {
                return Err(RommBrowseError::PaginationInconsistency);
            }
        }
        let total = optional_u64(value, "total")?;
        let count =
            u32::try_from(items.len()).map_err(|_| RommBrowseError::PaginationInconsistency)?;
        let end = offset
            .checked_add(count)
            .ok_or(RommBrowseError::PaginationInconsistency)?;
        if count > limit
            || total.is_some_and(|total| {
                (count > 0 && u64::from(end) > total) || (count < limit && u64::from(end) < total)
            })
        {
            return Err(RommBrowseError::PaginationInconsistency);
        }
        let mut ids = BTreeSet::new();
        let games = items
            .iter()
            .map(|v| {
                let game = self.project_game(v, "/api/roms", false)?;
                if !ids.insert(game.id)
                    || filter.platform_id.is_some_and(|id| {
                        game.identity.provider_platform_id.as_deref()
                            != Some(id.to_string().as_str())
                    })
                {
                    return Err(RommBrowseError::PaginationInconsistency);
                }
                Ok(game)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_next = total.map(|t| u64::from(end) < t).unwrap_or(count == limit);
        let next_offset = if has_next {
            Some(
                offset
                    .checked_add(limit)
                    .ok_or(RommBrowseError::PaginationInconsistency)?,
            )
        } else {
            None
        };
        Ok(RommBrowsePage {
            games,
            offset,
            page_size: limit,
            total,
            next_offset,
            previous_offset: (offset > 0).then(|| offset.saturating_sub(limit)),
        })
    }
}

fn major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

fn check_version(version: &str) -> Result<(), RommBrowseError> {
    if version.len() > 128
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    {
        return Err(RommBrowseError::SchemaIncompatibility);
    }
    if !major(version).is_some_and(|v| (MINIMUM_SUPPORTED_MAJOR..=MAX_STUDIED_MAJOR).contains(&v)) {
        return Err(RommBrowseError::UnsupportedVersion);
    }
    Ok(())
}

fn capabilities(document: &Value) -> Result<RommBrowseCapabilities, RommBrowseError> {
    let paths = document
        .get("paths")
        .and_then(Value::as_object)
        .ok_or(RommBrowseError::SchemaIncompatibility)?;
    let get = |path: &str| {
        paths
            .get(path)
            .and_then(|v| v.get("get"))
            .filter(|v| v.is_object())
    };
    let mut parameters = BTreeSet::new();
    if let Some(operation) = get("/api/roms") {
        for entry in [paths.get("/api/roms"), Some(operation)]
            .into_iter()
            .flatten()
        {
            if let Some(list) = entry.get("parameters") {
                for parameter in list
                    .as_array()
                    .ok_or(RommBrowseError::SchemaIncompatibility)?
                {
                    let parameter = resolve_parameter(document, parameter)?;
                    if parameter.get("in").and_then(Value::as_str) == Some("query") {
                        parameters.insert(
                            parameter
                                .get("name")
                                .and_then(Value::as_str)
                                .ok_or(RommBrowseError::SchemaIncompatibility)?,
                        );
                    }
                }
            }
        }
    }
    Ok(RommBrowseCapabilities {
        platforms: get("/api/platforms").is_some(),
        games: get("/api/roms").is_some()
            && parameters.contains("limit")
            && parameters.contains("offset"),
        game_detail: get("/api/roms/{id}").is_some(),
        text_search: parameters.contains("search_term"),
        platform_filter: parameters.contains("platform_ids"),
        omit_files: parameters.contains("with_files"),
        omit_char_index: parameters.contains("with_char_index"),
        omit_filter_values: parameters.contains("with_filter_values"),
        omit_rom_id_index: parameters.contains("with_rom_id_index"),
        disable_grouping: parameters.contains("group_by_meta_id"),
    })
}

fn resolve_parameter<'a>(
    document: &'a Value,
    mut parameter: &'a Value,
) -> Result<&'a Value, RommBrowseError> {
    for _ in 0..MAX_BROWSE_DEPTH {
        let Some(reference) = parameter.get("$ref") else {
            return Ok(parameter);
        };
        let reference = reference
            .as_str()
            .filter(|r| r.starts_with("#/components/parameters/"))
            .ok_or(RommBrowseError::SchemaIncompatibility)?;
        parameter = document
            .pointer(&reference[1..])
            .ok_or(RommBrowseError::SchemaIncompatibility)?;
    }
    Err(RommBrowseError::LimitExceeded)
}

fn page_path(
    caps: &RommBrowseCapabilities,
    limit: u32,
    offset: u32,
    filter: &RommBrowseFilter,
) -> Result<String, RommBrowseError> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("limit", &limit.to_string())
        .append_pair("offset", &offset.to_string());
    for (supported, name) in [
        (caps.omit_files, "with_files"),
        (caps.omit_char_index, "with_char_index"),
        (caps.omit_filter_values, "with_filter_values"),
        (caps.omit_rom_id_index, "with_rom_id_index"),
        (caps.disable_grouping, "group_by_meta_id"),
    ] {
        if supported {
            query.append_pair(name, "false");
        }
    }
    if let Some(text) = &filter.text {
        if !caps.text_search {
            return Err(RommBrowseError::UnsupportedFilter);
        }
        if text.len() > MAX_SEARCH_BYTES || text.chars().any(char::is_control) {
            return Err(RommBrowseError::InvalidRequest);
        }
        query.append_pair("search_term", text);
    }
    if let Some(id) = filter.platform_id {
        if !caps.platform_filter {
            return Err(RommBrowseError::UnsupportedFilter);
        }
        check_id(id)?;
        query.append_pair("platform_ids", &id.to_string());
    }
    Ok(format!("/api/roms?{}", query.finish()))
}

fn platform_mapping(
    canonical: Option<&str>,
    slug: &str,
    filesystem_slug: Option<&str>,
) -> RommPlatformMapping {
    let Some(canonical) = canonical else {
        return RommPlatformMapping::Unknown;
    };
    if let Some(other) = filesystem_slug.and_then(canonical_platform_for_romm_slug)
        && other != canonical
    {
        let mut candidates = vec![canonical.to_owned(), other.to_owned()];
        candidates.sort();
        return RommPlatformMapping::Ambiguous { candidates };
    }
    if slug == canonical {
        RommPlatformMapping::Exact {
            canonical: canonical.to_owned(),
        }
    } else {
        RommPlatformMapping::KnownAlias {
            canonical: canonical.to_owned(),
        }
    }
}

fn check_id(id: u64) -> Result<(), RommBrowseError> {
    if id == 0 || id > i64::MAX as u64 {
        Err(RommBrowseError::SchemaIncompatibility)
    } else {
        Ok(())
    }
}

fn required_id(value: &Value, field: &str) -> Result<u64, RommBrowseError> {
    let id = optional_u64(value, field)?.ok_or(RommBrowseError::SchemaIncompatibility)?;
    check_id(id)?;
    Ok(id)
}

fn optional_u64(value: &Value, field: &str) -> Result<Option<u64>, RommBrowseError> {
    value
        .get(field)
        .filter(|v| !v.is_null())
        .map(|v| v.as_u64().ok_or(RommBrowseError::SchemaIncompatibility))
        .transpose()
}

fn optional_string(value: &Value, field: &str) -> Result<Option<String>, RommBrowseError> {
    value
        .get(field)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or(RommBrowseError::SchemaIncompatibility)
        })
        .transpose()
}

fn check_string_list(value: &Value, field: &str) -> Result<(), RommBrowseError> {
    if let Some(list) = value.get(field).filter(|v| !v.is_null())
        && !list
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err(RommBrowseError::SchemaIncompatibility);
    }
    Ok(())
}

fn check_document(
    value: &Value,
    depth: usize,
    list_limit: usize,
    nodes: &mut usize,
) -> Result<(), RommBrowseError> {
    *nodes += 1;
    if depth > MAX_BROWSE_DEPTH || *nodes > MAX_DOCUMENT_NODES {
        return Err(RommBrowseError::LimitExceeded);
    }
    match value {
        Value::String(s) if s.len() > MAX_BROWSE_STRING_BYTES => {
            return Err(RommBrowseError::LimitExceeded);
        }
        Value::Array(values) => {
            if values.len() > list_limit {
                return Err(RommBrowseError::LimitExceeded);
            }
            for v in values {
                check_document(v, depth + 1, list_limit, nodes)?;
            }
        }
        Value::Object(values) => {
            if values.len() > MAX_BROWSE_PLATFORMS
                || values.keys().any(|s| s.len() > MAX_BROWSE_STRING_BYTES)
            {
                return Err(RommBrowseError::LimitExceeded);
            }
            for v in values.values() {
                check_document(v, depth + 1, list_limit, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn project_file(value: &Value) -> Result<RommRelatedFile, RommBrowseError> {
    let id = required_id(value, "id")?;
    let mut hashes = Vec::new();
    let mut rejected_hashes = Vec::new();
    for (field, algorithm) in [
        ("crc_hash", HashAlgorithm::Crc32),
        ("md5_hash", HashAlgorithm::Md5),
        ("sha1_hash", HashAlgorithm::Sha1),
    ] {
        if let Some(raw) = optional_string(value, field)? {
            if let Some(hash) = ExternalHash::parse(algorithm, &raw) {
                hashes.push(hash);
            } else {
                rejected_hashes.push(algorithm);
            }
        }
    }
    Ok(RommRelatedFile {
        id,
        filename: optional_string(value, "file_name")?,
        provider_path: optional_string(value, "full_path")?
            .or(optional_string(value, "file_path")?),
        size_bytes: optional_u64(value, "file_size_bytes")?,
        hashes,
        rejected_hashes,
    })
}

#[cfg(test)]
mod tests;
