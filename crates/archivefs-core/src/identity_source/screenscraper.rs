//! Optional ScreenScraper metadata enrichment.
//!
//! ScreenScraper is deliberately not an identity source here.  This module
//! sends only a bounded, user-triggered metadata request and returns display
//! enrichment candidates.  [`IdentityContribution::None`] is part of the
//! returned model so a caller cannot accidentally feed a fuzzy provider match
//! into EmuWiz identity resolution.
//!
//! The API is v2 beta and its official documentation says that it may change
//! without notice.  The client therefore keeps the wire surface small,
//! tolerates additive response fields, bounds responses/retries, and never
//! persists provider state or artwork bytes.

use std::fmt;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use url::form_urlencoded;

pub const DEFAULT_BASE_URL: &str = "https://api.screenscraper.fr/api2";
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CANDIDATES: usize = 30;
pub const MAX_RETRIES: usize = 2;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// ScreenScraper contributes no authoritative identity, by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityContribution {
    None,
}

/// A secret kept only in memory. It cannot be formatted or serialized.
#[derive(Clone, PartialEq, Eq)]
pub struct ScreenScraperSecret(String);

impl ScreenScraperSecret {
    pub fn parse(value: &str) -> Result<Self, CredentialError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(CredentialError::Empty);
        }
        if value.len() > 4096 {
            return Err(CredentialError::TooLong);
        }
        if value.chars().any(|ch| ch.is_control() || !ch.is_ascii()) {
            return Err(CredentialError::InvalidCharacters);
        }
        Ok(Self(value.to_string()))
    }

    fn value(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ScreenScraperSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ScreenScraperSecret(redacted)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    Empty,
    TooLong,
    InvalidCharacters,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "credential is empty",
            Self::TooLong => "credential is too long",
            Self::InvalidCharacters => "credential contains invalid characters",
        })
    }
}

/// Credentials required by the current API. They are intentionally not part
/// of [`ScreenScraperConfig`] and have no persistence method.
#[derive(Clone, PartialEq, Eq)]
pub struct ScreenScraperCredentials {
    developer_id: String,
    developer_password: ScreenScraperSecret,
    user_id: Option<String>,
    user_password: Option<ScreenScraperSecret>,
}

impl fmt::Debug for ScreenScraperCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScreenScraperCredentials")
            .field("developer_id", &self.developer_id)
            .field("developer_password", &"redacted")
            .field("user_id", &self.user_id)
            .field(
                "user_password",
                &self.user_password.as_ref().map(|_| "redacted"),
            )
            .finish()
    }
}

impl ScreenScraperCredentials {
    pub fn new(
        developer_id: &str,
        developer_password: ScreenScraperSecret,
        user_id: Option<&str>,
        user_password: Option<ScreenScraperSecret>,
    ) -> Result<Self, CredentialError> {
        let developer_id = developer_id.trim();
        if developer_id.is_empty() || developer_id.len() > 256 {
            return Err(CredentialError::Empty);
        }
        if developer_id
            .chars()
            .any(|ch| ch.is_control() || !ch.is_ascii())
        {
            return Err(CredentialError::InvalidCharacters);
        }
        let user_id = user_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        Ok(Self {
            developer_id: developer_id.to_string(),
            developer_password,
            user_id,
            user_password,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenScraperConfig {
    pub enabled: bool,
    pub base_url: String,
    pub softname: String,
    pub timeout: Duration,
    pub max_retries: usize,
}

impl Default for ScreenScraperConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: DEFAULT_BASE_URL.to_string(),
            softname: "EmuWiz".to_string(),
            timeout: DEFAULT_TIMEOUT,
            max_retries: MAX_RETRIES,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenScraperLookup {
    pub platform_id: Option<u32>,
    pub title: Option<String>,
    pub region: Option<String>,
    pub language: Option<String>,
    pub rom_name: Option<String>,
    pub rom_size: Option<u64>,
    pub crc32: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupKind {
    Search,
    GameById { game_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataProvenance {
    pub provider: &'static str,
    pub provider_record_id: String,
    pub retrieved_at_unix_seconds: u64,
    pub match_basis: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichedField {
    pub value: String,
    pub provenance: MetadataProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenScraperEnrichment {
    pub identity_contribution: IdentityContribution,
    pub provider_game_id: String,
    pub title: Option<EnrichedField>,
    pub alternative_title: Option<EnrichedField>,
    pub description: Option<EnrichedField>,
    pub release_date: Option<EnrichedField>,
    pub developer: Option<EnrichedField>,
    pub publisher: Option<EnrichedField>,
    pub genre: Option<EnrichedField>,
    pub players: Option<EnrichedField>,
    pub rating: Option<EnrichedField>,
    pub region: Option<EnrichedField>,
    pub language: Option<EnrichedField>,
    pub external_url: Option<EnrichedField>,
    pub media_references: Vec<MediaReference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaReference {
    pub kind: String,
    pub url: String,
    pub rights_note: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaSnapshot {
    pub requests_today: Option<u64>,
    pub negative_requests_today: Option<u64>,
    pub max_requests_per_minute: Option<u64>,
    pub max_requests_per_day: Option<u64>,
    pub max_negative_requests_per_day: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub retry_after: Option<Duration>,
}

pub trait ScreenScraperTransport {
    fn get(
        &self,
        url: &str,
        max_bytes: usize,
        timeout: Duration,
    ) -> Result<ProviderResponse, ScreenScraperError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenScraperError {
    Disabled,
    MissingCredentials,
    Authentication {
        status: u16,
    },
    QuotaExhausted {
        status: u16,
        retry_after: Option<Duration>,
    },
    Temporary {
        status: Option<u16>,
    },
    NoResult,
    MalformedResponse {
        detail: String,
    },
    Network {
        detail: String,
    },
    ResponseTooLarge {
        limit: usize,
    },
    InvalidRequest {
        detail: String,
    },
}

impl ScreenScraperError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Temporary { .. } | Self::Network { .. })
    }
}

#[derive(Debug)]
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(DEFAULT_TIMEOUT))
            .max_redirects(0)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl ScreenScraperTransport for UreqTransport {
    fn get(
        &self,
        url: &str,
        max_bytes: usize,
        timeout: Duration,
    ) -> Result<ProviderResponse, ScreenScraperError> {
        let request = self
            .agent
            .get(url)
            .header("Accept", "application/json")
            .config()
            .timeout_global(Some(timeout))
            .build();
        let response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(status)) => {
                return Ok(ProviderResponse {
                    status,
                    body: Vec::new(),
                    retry_after: None,
                });
            }
            Err(ureq::Error::Timeout(_)) => {
                return Err(ScreenScraperError::Network {
                    detail: "request timed out".into(),
                });
            }
            Err(error) => {
                return Err(ScreenScraperError::Network {
                    detail: classify_network_error(&error),
                });
            }
        };
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = response
            .into_body()
            .into_with_config()
            .limit((max_bytes + 1) as u64)
            .read_to_vec()
            .map_err(|error| ScreenScraperError::Network {
                detail: error.to_string(),
            })?;
        if body.len() > max_bytes {
            return Err(ScreenScraperError::ResponseTooLarge { limit: max_bytes });
        }
        Ok(ProviderResponse {
            status,
            body,
            retry_after,
        })
    }
}

/// A read-only client. It never stores responses or writes provider state.
pub struct ScreenScraperClient<T> {
    config: ScreenScraperConfig,
    credentials: Option<ScreenScraperCredentials>,
    transport: T,
}

impl<T> ScreenScraperClient<T> {
    pub fn new(
        config: ScreenScraperConfig,
        credentials: Option<ScreenScraperCredentials>,
        transport: T,
    ) -> Self {
        Self {
            config,
            credentials,
            transport,
        }
    }

    pub fn lookup(
        &self,
        kind: LookupKind,
        input: &ScreenScraperLookup,
    ) -> Result<LookupOutcome, ScreenScraperError>
    where
        T: ScreenScraperTransport,
    {
        if !self.config.enabled {
            return Err(ScreenScraperError::Disabled);
        }
        let credentials = self
            .credentials
            .as_ref()
            .ok_or(ScreenScraperError::MissingCredentials)?;
        let url = build_request_url(&self.config, credentials, kind, input)?;
        let response = self.request_with_bounded_retry(&url)?;
        if response.status == 404 {
            return Ok(LookupOutcome::NoResult {
                quota: parse_quota(&response.body),
            });
        }
        if response.status == 401 || response.status == 403 {
            return Err(ScreenScraperError::Authentication {
                status: response.status,
            });
        }
        if response.status == 429 || response.status == 430 || response.status == 431 {
            return Err(ScreenScraperError::QuotaExhausted {
                status: response.status,
                retry_after: response.retry_after,
            });
        }
        if !(200..300).contains(&response.status) {
            return Err(if matches!(response.status, 408 | 423 | 500..=599) {
                ScreenScraperError::Temporary {
                    status: Some(response.status),
                }
            } else {
                ScreenScraperError::Network {
                    detail: format!("provider returned HTTP {}", response.status),
                }
            });
        }
        parse_outcome(&response.body)
    }

    fn request_with_bounded_retry(&self, url: &str) -> Result<ProviderResponse, ScreenScraperError>
    where
        T: ScreenScraperTransport,
    {
        let retries = self.config.max_retries.min(MAX_RETRIES);
        let mut attempt = 0;
        loop {
            let response = self
                .transport
                .get(url, MAX_RESPONSE_BYTES, self.config.timeout)?;
            if !(matches!(response.status, 408 | 423 | 500..=599)) || attempt >= retries {
                return Ok(response);
            }
            attempt += 1;
            thread::sleep(Duration::from_millis(50 * (1 << attempt)));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupOutcome {
    Candidates {
        candidates: Vec<ScreenScraperEnrichment>,
        quota: Option<QuotaSnapshot>,
    },
    NoResult {
        quota: Option<QuotaSnapshot>,
    },
}

fn build_request_url(
    config: &ScreenScraperConfig,
    credentials: &ScreenScraperCredentials,
    kind: LookupKind,
    input: &ScreenScraperLookup,
) -> Result<String, ScreenScraperError> {
    let base = config.base_url.trim_end_matches('/');
    let parsed_base =
        url::Url::parse(base).map_err(|error| ScreenScraperError::InvalidRequest {
            detail: format!("invalid ScreenScraper API URL: {error}"),
        })?;
    if parsed_base.scheme() != "https"
        || parsed_base.host_str() != Some("api.screenscraper.fr")
        || !parsed_base.username().is_empty()
        || parsed_base.password().is_some()
        || parsed_base.query().is_some()
        || parsed_base.fragment().is_some()
    {
        return Err(ScreenScraperError::InvalidRequest {
            detail: "ScreenScraper requests require the official HTTPS API host without embedded credentials or query state".into(),
        });
    }
    let endpoint = match kind {
        LookupKind::Search => "jeuRecherche.php",
        LookupKind::GameById { .. } => "jeuInfos.php",
    };
    if config.softname.trim().is_empty() {
        return Err(ScreenScraperError::InvalidRequest {
            detail: "softname must not be empty".into(),
        });
    }
    if matches!(kind, LookupKind::Search)
        && input.title.as_deref().unwrap_or_default().trim().is_empty()
    {
        return Err(ScreenScraperError::InvalidRequest {
            detail: "search title must not be empty".into(),
        });
    }
    if let Some(name) = &input.rom_name
        && (name.len() > 255 || name.contains('/') || name.contains('\\'))
    {
        return Err(ScreenScraperError::InvalidRequest {
            detail: "rom name must be a basename without path separators".into(),
        });
    }
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("devid", &credentials.developer_id);
    query.append_pair("devpassword", credentials.developer_password.value());
    query.append_pair("softname", &config.softname);
    query.append_pair("output", "json");
    if let (Some(user_id), Some(password)) = (&credentials.user_id, &credentials.user_password) {
        query.append_pair("ssid", user_id);
        query.append_pair("sspassword", password.value());
    }
    match kind {
        LookupKind::Search => {
            query.append_pair(
                "recherche",
                input.title.as_deref().unwrap_or_default().trim(),
            );
        }
        LookupKind::GameById { game_id } => {
            query.append_pair("gameid", &game_id.to_string());
        }
    }
    if let Some(platform_id) = input.platform_id {
        query.append_pair("systemeid", &platform_id.to_string());
    }
    if let Some(region) = &input.region {
        query.append_pair("region", region);
    }
    if let Some(language) = &input.language {
        query.append_pair("langue", language);
    }
    if let Some(name) = &input.rom_name {
        query.append_pair("romnom", name);
    }
    if let Some(size) = input.rom_size {
        query.append_pair("romtaille", &size.to_string());
    }
    if let Some(crc32) = &input.crc32 {
        query.append_pair("crc", crc32);
    }
    if let Some(md5) = &input.md5 {
        query.append_pair("md5", md5);
    }
    if let Some(sha1) = &input.sha1 {
        query.append_pair("sha1", sha1);
    }
    Ok(format!("{base}/{endpoint}?{}", query.finish()))
}

fn parse_outcome(body: &[u8]) -> Result<LookupOutcome, ScreenScraperError> {
    let response: WireEnvelope =
        serde_json::from_slice(body).map_err(|error| ScreenScraperError::MalformedResponse {
            detail: format!("invalid JSON: {error}"),
        })?;
    let quota = response.ssuser.as_ref().map(QuotaSnapshot::from_wire);
    let games = response
        .games
        .or_else(|| response.game.map(|game| vec![game]))
        .unwrap_or_default();
    if games.is_empty() {
        return Ok(LookupOutcome::NoResult { quota });
    }
    let retrieved = now_unix();
    let candidates = games
        .into_iter()
        .take(MAX_CANDIDATES)
        .filter_map(|game| enrichment_from_wire(game, retrieved))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(ScreenScraperError::MalformedResponse {
            detail: "response contained no usable game IDs".into(),
        });
    }
    Ok(LookupOutcome::Candidates { candidates, quota })
}

fn enrichment_from_wire(game: WireGame, retrieved: u64) -> Option<ScreenScraperEnrichment> {
    let provider_game_id = game.id?.to_string();
    let provider_record_id = provider_game_id.clone();
    let provenance = |basis: &str| MetadataProvenance {
        provider: "ScreenScraper",
        provider_record_id: provider_record_id.clone(),
        retrieved_at_unix_seconds: retrieved,
        match_basis: basis.to_string(),
    };
    let field = |value: Option<String>, basis: &str| {
        value
            .filter(|value| !value.trim().is_empty())
            .map(|value| EnrichedField {
                value,
                provenance: provenance(basis),
            })
    };
    let title = game
        .names
        .as_ref()
        .and_then(|names| names.get("nom_ss").cloned())
        .or(game.name.clone());
    let alternative_title = game.names.as_ref().and_then(|names| {
        names
            .iter()
            .find(|(key, _)| key.as_str() != "nom_ss")
            .map(|(_, value)| value.clone())
    });
    let mut media_references = Vec::new();
    if let Some(media) = game.media {
        for (kind, url) in media {
            if url.starts_with("https://") || url.starts_with("http://") {
                media_references.push(MediaReference {
                    kind,
                    url,
                    rights_note: "reference only; EmuWiz does not cache or redistribute this media",
                });
            }
        }
    }
    Some(ScreenScraperEnrichment {
        identity_contribution: IdentityContribution::None,
        provider_game_id,
        title: field(title, "provider search/detail result"),
        alternative_title: field(alternative_title, "provider regional title"),
        description: field(game.synopsis, "provider description"),
        release_date: field(game.release_date, "provider release date"),
        developer: field(game.developer, "provider developer"),
        publisher: field(game.publisher, "provider publisher"),
        genre: field(game.genre, "provider genre"),
        players: field(game.players, "provider player count"),
        rating: field(game.rating, "provider rating"),
        region: field(game.region, "provider region"),
        language: field(game.language, "provider language"),
        external_url: field(game.external_url, "provider record reference"),
        media_references,
    })
}

fn parse_quota(body: &[u8]) -> Option<QuotaSnapshot> {
    serde_json::from_slice::<WireEnvelope>(body)
        .ok()
        .and_then(|response| response.ssuser.map(|user| QuotaSnapshot::from_wire(&user)))
}

impl QuotaSnapshot {
    fn from_wire(user: &WireUser) -> Self {
        Self {
            requests_today: user.requests_today,
            negative_requests_today: user.negative_requests_today,
            max_requests_per_minute: user.max_requests_per_minute,
            max_requests_per_day: user.max_requests_per_day,
            max_negative_requests_per_day: user.max_negative_requests_per_day,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WireEnvelope {
    #[serde(default, rename = "jeux")]
    games: Option<Vec<WireGame>>,
    #[serde(default, rename = "jeu")]
    game: Option<WireGame>,
    #[serde(default, rename = "ssuser")]
    ssuser: Option<WireUser>,
}

#[derive(Debug, Deserialize)]
struct WireUser {
    #[serde(default, rename = "requeststoday")]
    requests_today: Option<u64>,
    #[serde(default, rename = "requestskotoday")]
    negative_requests_today: Option<u64>,
    #[serde(default, rename = "maxrequestspermin", alias = "maxrequestsperdmin")]
    max_requests_per_minute: Option<u64>,
    #[serde(default, rename = "maxrequestsperday")]
    max_requests_per_day: Option<u64>,
    #[serde(default, rename = "maxrequestskoperday", alias = "maxrequestskoperd")]
    max_negative_requests_per_day: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WireGame {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "noms")]
    names: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, rename = "editeur")]
    publisher: Option<String>,
    #[serde(default, rename = "developpeur")]
    developer: Option<String>,
    #[serde(default, rename = "joueurs")]
    players: Option<String>,
    #[serde(default, rename = "synopsis")]
    synopsis: Option<String>,
    #[serde(default, rename = "date")]
    release_date: Option<String>,
    #[serde(default, rename = "genre")]
    genre: Option<String>,
    #[serde(default, rename = "note")]
    rating: Option<String>,
    #[serde(default, rename = "region")]
    region: Option<String>,
    #[serde(default, rename = "langue")]
    language: Option<String>,
    #[serde(default, rename = "url")]
    external_url: Option<String>,
    #[serde(default, rename = "medias")]
    media: Option<std::collections::BTreeMap<String, String>>,
}

fn classify_network_error(error: &ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(status) => format!("HTTP {status}"),
        ureq::Error::Timeout(_) => "request timed out".into(),
        _ => "network request failed".into(),
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct FakeTransport {
        responses: Arc<Mutex<Vec<Result<ProviderResponse, ScreenScraperError>>>>,
        urls: Arc<Mutex<Vec<String>>>,
    }

    impl ScreenScraperTransport for FakeTransport {
        fn get(
            &self,
            url: &str,
            _max_bytes: usize,
            _timeout: Duration,
        ) -> Result<ProviderResponse, ScreenScraperError> {
            self.urls.lock().unwrap().push(url.to_string());
            self.responses.lock().unwrap().remove(0)
        }
    }

    fn make_client(
        response: Result<ProviderResponse, ScreenScraperError>,
    ) -> (ScreenScraperClient<FakeTransport>, Arc<Mutex<Vec<String>>>) {
        let urls = Arc::new(Mutex::new(Vec::new()));
        let transport = FakeTransport {
            responses: Arc::new(Mutex::new(vec![response])),
            urls: urls.clone(),
        };
        let secret = ScreenScraperSecret::parse("dev-secret").unwrap();
        let credentials = ScreenScraperCredentials::new(
            "dev-id",
            secret,
            Some("user"),
            Some(ScreenScraperSecret::parse("user-secret").unwrap()),
        )
        .unwrap();
        let config = ScreenScraperConfig {
            enabled: true,
            max_retries: 0,
            ..ScreenScraperConfig::default()
        };
        (
            ScreenScraperClient::new(config, Some(credentials), transport),
            urls,
        )
    }

    fn ok(body: &str) -> Result<ProviderResponse, ScreenScraperError> {
        Ok(ProviderResponse {
            status: 200,
            body: body.as_bytes().to_vec(),
            retry_after: None,
        })
    }

    #[test]
    fn search_is_platform_aware_and_does_not_send_a_path_or_rom_bytes() {
        let (client, urls) = make_client(ok(r#"{"jeux":[{"id":42,"nom":"Game"}]}"#));
        let input = ScreenScraperLookup {
            platform_id: Some(12),
            title: Some("The Game".into()),
            rom_name: Some("The Game.zip".into()),
            rom_size: Some(12),
            ..Default::default()
        };
        let outcome = client.lookup(LookupKind::Search, &input).unwrap();
        assert!(matches!(outcome, LookupOutcome::Candidates { .. }));
        let url = &urls.lock().unwrap()[0];
        assert!(url.contains("systemeid=12"));
        assert!(url.contains("recherche=The+Game"));
        assert!(!url.contains("/home/") && !url.contains("rom-bytes"));
        assert!(url.contains("devpassword=dev-secret"));
    }

    #[test]
    fn exact_provider_id_uses_direct_lookup_and_has_no_identity_contribution() {
        let (client, urls) = make_client(ok(r#"{"jeu":{"id":42,"nom":"Game"}}"#));
        let outcome = client
            .lookup(LookupKind::GameById { game_id: 42 }, &Default::default())
            .unwrap();
        let LookupOutcome::Candidates { candidates, .. } = outcome else {
            panic!("expected candidate")
        };
        assert_eq!(candidates[0].provider_game_id, "42");
        assert_eq!(
            candidates[0].identity_contribution,
            IdentityContribution::None
        );
        assert!(urls.lock().unwrap()[0].contains("gameid=42"));
    }

    #[test]
    fn ambiguous_search_results_remain_candidates() {
        let (client, _) = make_client(ok(
            r#"{"jeux":[{"id":1,"nom":"Game"},{"id":2,"nom":"Game"}]}"#,
        ));
        let input = ScreenScraperLookup {
            title: Some("Game".into()),
            ..Default::default()
        };
        let LookupOutcome::Candidates { candidates, .. } =
            client.lookup(LookupKind::Search, &input).unwrap()
        else {
            panic!("expected candidates")
        };
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn quota_is_parsed_without_assuming_a_fixed_limit() {
        let (client, _) = make_client(ok(
            r#"{"ssuser":{"requeststoday":7,"maxrequestspermin":11,"maxrequestsperday":99},"jeux":[{"id":1}]}"#,
        ));
        let LookupOutcome::Candidates {
            quota: Some(quota), ..
        } = client
            .lookup(
                LookupKind::Search,
                &ScreenScraperLookup {
                    title: Some("Game".into()),
                    ..Default::default()
                },
            )
            .unwrap()
        else {
            panic!("expected quota")
        };
        assert_eq!(quota.requests_today, Some(7));
        assert_eq!(quota.max_requests_per_minute, Some(11));
        assert_eq!(quota.max_requests_per_day, Some(99));
    }

    #[test]
    fn auth_quota_and_no_result_are_distinct() {
        let (client, _) = make_client(Ok(ProviderResponse {
            status: 403,
            body: Vec::new(),
            retry_after: None,
        }));
        assert!(matches!(
            client.lookup(
                LookupKind::Search,
                &ScreenScraperLookup {
                    title: Some("x".into()),
                    ..Default::default()
                }
            ),
            Err(ScreenScraperError::Authentication { .. })
        ));
        let (client, _) = make_client(Ok(ProviderResponse {
            status: 430,
            body: Vec::new(),
            retry_after: Some(Duration::from_secs(60)),
        }));
        assert!(matches!(
            client.lookup(
                LookupKind::Search,
                &ScreenScraperLookup {
                    title: Some("x".into()),
                    ..Default::default()
                }
            ),
            Err(ScreenScraperError::QuotaExhausted { .. })
        ));
        let (client, _) = make_client(Ok(ProviderResponse {
            status: 404,
            body: Vec::new(),
            retry_after: None,
        }));
        assert!(matches!(
            client.lookup(
                LookupKind::Search,
                &ScreenScraperLookup {
                    title: Some("x".into()),
                    ..Default::default()
                }
            ),
            Ok(LookupOutcome::NoResult { .. })
        ));
    }

    #[test]
    fn temporary_provider_failure_is_retried_only_within_bound() {
        let urls = Arc::new(Mutex::new(Vec::new()));
        let transport = FakeTransport {
            responses: Arc::new(Mutex::new(vec![
                Ok(ProviderResponse {
                    status: 503,
                    body: Vec::new(),
                    retry_after: None,
                }),
                ok(r#"{"jeu":{"id":7,"nom":"Recovered"}}"#),
            ])),
            urls: urls.clone(),
        };
        let credentials = ScreenScraperCredentials::new(
            "dev-id",
            ScreenScraperSecret::parse("dev-secret").unwrap(),
            None,
            None,
        )
        .unwrap();
        let config = ScreenScraperConfig {
            enabled: true,
            max_retries: MAX_RETRIES + 10,
            ..ScreenScraperConfig::default()
        };
        let client = ScreenScraperClient::new(config, Some(credentials), transport);
        let outcome = client
            .lookup(
                LookupKind::Search,
                &ScreenScraperLookup {
                    title: Some("Recovered".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(matches!(outcome, LookupOutcome::Candidates { .. }));
        assert_eq!(urls.lock().unwrap().len(), 2);
    }

    #[test]
    fn malformed_response_fails_closed() {
        let (client, _) = make_client(ok("not-json"));
        assert!(matches!(
            client.lookup(
                LookupKind::Search,
                &ScreenScraperLookup {
                    title: Some("Game".into()),
                    ..Default::default()
                }
            ),
            Err(ScreenScraperError::MalformedResponse { .. })
        ));
    }

    #[test]
    fn insecure_or_non_official_endpoint_is_rejected_before_transport() {
        let (mut client, urls) = make_client(ok("{}"));
        client.config.base_url = "http://api.screenscraper.fr/api2".into();
        assert!(matches!(
            client.lookup(
                LookupKind::Search,
                &ScreenScraperLookup {
                    title: Some("Game".into()),
                    ..Default::default()
                }
            ),
            Err(ScreenScraperError::InvalidRequest { .. })
        ));
        assert!(urls.lock().unwrap().is_empty());
    }

    #[test]
    fn credentials_are_not_debuggable_or_persistable() {
        let credentials = ScreenScraperCredentials::new(
            "dev-id",
            ScreenScraperSecret::parse("super-secret-value").unwrap(),
            None,
            None,
        )
        .unwrap();
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("super-secret-value"));
    }

    #[test]
    fn disabled_or_missing_credentials_never_contact_transport() {
        let (mut client, urls) = make_client(ok("{}"));
        client.config.enabled = false;
        assert!(matches!(
            client.lookup(
                LookupKind::Search,
                &ScreenScraperLookup {
                    title: Some("x".into()),
                    ..Default::default()
                }
            ),
            Err(ScreenScraperError::Disabled)
        ));
        assert!(urls.lock().unwrap().is_empty());
    }
}
