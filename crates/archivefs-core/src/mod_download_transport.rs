//! Bounded HTTPS transport for one already policy-reviewed mod payload.
//!
//! The public download entry point accepts a policy input and its previously
//! computed result. It never accepts an arbitrary URL. The real backend uses
//! `ureq` with redirects disabled and a resolver that rejects every forbidden
//! address before the connection is made; tests use the same streaming engine
//! with an injected response backend.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::mod_catalogue::ModCatalogueHash;
use crate::mod_download_policy::{
    ModDownloadHashClass, ModDownloadPolicyDecision, ModDownloadPolicyDecisionResult,
    ModDownloadPolicyInput, is_forbidden_resolved_address,
};

pub const DEFAULT_DOWNLOAD_LIMIT: u64 = 512 * 1024 * 1024;
pub const MAX_REDIRECTS: usize = 5;
const CHUNK_SIZE: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModDownloadRequest {
    pub policy_input: ModDownloadPolicyInput,
    pub policy_result: ModDownloadPolicyDecisionResult,
    pub review_confirmed: bool,
    pub staging_root: PathBuf,
    pub cache_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModDownloadProvenance {
    pub provider: String,
    pub original_url: String,
    pub final_url: String,
    pub redirect_chain: Vec<String>,
    pub payload_hosts: Vec<String>,
    pub retrieved_at_unix_secs: u64,
    pub declared_size: Option<u64>,
    pub content_length: Option<u64>,
    pub actual_bytes: u64,
    pub expected_hash: Option<ModCatalogueHash>,
    pub expected_hash_verified: bool,
    pub local_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModDownloadResult {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub provenance: ModDownloadProvenance,
}

#[derive(Debug)]
pub enum ModDownloadFailure {
    PolicyNotApproved,
    StagingUnsafe(String),
    CacheUnsafe(String),
    Url(String),
    DnsRejected(String),
    Redirect(String),
    HttpStatus(u16),
    Timeout,
    Tls(String),
    Transport(String),
    ContentLengthExceedsLimit(u64),
    StreamExceedsLimit(u64),
    Truncated { expected: u64, received: u64 },
    HashMismatch { expected: String, actual: String },
    UnsupportedExpectedHash,
    CacheConflict(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for ModDownloadFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PolicyNotApproved => formatter.write_str("download policy was not approved"),
            Self::StagingUnsafe(detail) | Self::CacheUnsafe(detail) => formatter.write_str(detail),
            Self::Url(detail)
            | Self::Redirect(detail)
            | Self::DnsRejected(detail)
            | Self::Transport(detail)
            | Self::Tls(detail) => formatter.write_str(detail),
            Self::HttpStatus(status) => write!(formatter, "mod server returned HTTP {status}"),
            Self::Timeout => formatter.write_str("mod download timed out"),
            Self::ContentLengthExceedsLimit(size) => write!(
                formatter,
                "response is {size} bytes, over the download limit"
            ),
            Self::StreamExceedsLimit(size) => write!(
                formatter,
                "response exceeded the download limit at {size} bytes"
            ),
            Self::Truncated { expected, received } => write!(
                formatter,
                "response was truncated: expected {expected} bytes, received {received}"
            ),
            Self::HashMismatch { expected, actual } => write!(
                formatter,
                "SHA-256 mismatch: expected {expected}, got {actual}"
            ),
            Self::UnsupportedExpectedHash => {
                formatter.write_str("expected hash is unsupported or malformed")
            }
            Self::CacheConflict(path) => write!(
                formatter,
                "content-addressed cache path conflicts with an existing object: {}",
                path.display()
            ),
            Self::Io(error) => write!(formatter, "download staging I/O failed: {error}"),
        }
    }
}

impl std::error::Error for ModDownloadFailure {}

impl From<io::Error> for ModDownloadFailure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct ModDownloadResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub location: Option<String>,
    pub body: Box<dyn Read + Send>,
}

pub trait ModDownloadBackend {
    fn get(&self, url: &str) -> Result<ModDownloadResponse, ModDownloadFailure>;
}

pub fn download_mod_payload(
    request: &ModDownloadRequest,
    backend: &dyn ModDownloadBackend,
) -> Result<ModDownloadResult, ModDownloadFailure> {
    if request.policy_result
        != crate::mod_download_policy::evaluate_mod_download_policy(&request.policy_input)
        || !request.policy_result.blockers.is_empty()
        || (request.policy_result.decision == ModDownloadPolicyDecision::RequiresReview
            && !request.review_confirmed)
        || request.policy_result.decision == ModDownloadPolicyDecision::Blocked
    {
        return Err(ModDownloadFailure::PolicyNotApproved);
    }
    if request.policy_input.hard_size_limit == 0 {
        return Err(ModDownloadFailure::ContentLengthExceedsLimit(0));
    }
    ensure_directory(&request.staging_root, "staging")?;
    ensure_directory(&request.cache_root, "cache")?;
    let temporary = request.staging_root.join(format!(
        ".emuwiz-mod-{}-{}.partial",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = download_to_staging(request, backend, &mut output, &temporary);
    drop(output);
    match result {
        Ok((sha256, size, final_url, redirects, hosts, content_length)) => {
            let destination =
                match publish_content_addressed(&request.cache_root, &temporary, &sha256, size) {
                    Ok(destination) => destination,
                    Err(error) => {
                        let _ = fs::remove_file(&temporary);
                        return Err(error);
                    }
                };
            Ok(ModDownloadResult {
                path: destination,
                size_bytes: size,
                sha256: sha256.clone(),
                provenance: ModDownloadProvenance {
                    provider: request.policy_input.provider.clone(),
                    original_url: request.policy_input.payload_url.clone(),
                    final_url,
                    redirect_chain: redirects,
                    payload_hosts: hosts,
                    retrieved_at_unix_secs: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    declared_size: request.policy_input.declared_size,
                    content_length,
                    actual_bytes: size,
                    expected_hash: request.policy_input.expected_hash.clone(),
                    expected_hash_verified: request.policy_result.hash_class
                        == ModDownloadHashClass::StrongExpectedHash,
                    local_sha256: sha256,
                },
            })
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// What a completed staging download reports back, in order: payload sha256,
/// bytes written, final URL after redirects, the redirect chain, the hosts
/// contacted, and the server's declared content length when it gave one.
type StagedDownload = (String, u64, String, Vec<String>, Vec<String>, Option<u64>);

fn download_to_staging(
    request: &ModDownloadRequest,
    backend: &dyn ModDownloadBackend,
    output: &mut File,
    temporary: &Path,
) -> Result<StagedDownload, ModDownloadFailure> {
    let mut current = request.policy_input.payload_url.clone();
    let mut visited = HashSet::from([current.clone()]);
    let mut redirects = Vec::new();
    let mut hosts = Vec::new();
    let mut response = loop {
        let parsed =
            Url::parse(&current).map_err(|error| ModDownloadFailure::Url(error.to_string()))?;
        if parsed.scheme() != "https" || parsed.username() != "" || parsed.password().is_some() {
            return Err(ModDownloadFailure::Redirect(
                "transport requires HTTPS without credentials".into(),
            ));
        }
        if let Some(host) = parsed.host_str() {
            hosts.push(host.to_string());
        }
        let response = backend.get(&current)?;
        if (300..400).contains(&response.status) {
            if redirects.len() >= MAX_REDIRECTS {
                return Err(ModDownloadFailure::Redirect(
                    "redirect limit exceeded".into(),
                ));
            }
            let location = response
                .location
                .ok_or_else(|| ModDownloadFailure::Redirect("redirect omitted Location".into()))?;
            let next = parsed
                .join(&location)
                .map_err(|error| ModDownloadFailure::Redirect(error.to_string()))?
                .to_string();
            let mut hop_input = request.policy_input.clone();
            hop_input.payload_url = current.clone();
            hop_input.redirects = vec![next.clone()];
            let hop = crate::mod_download_policy::evaluate_mod_download_policy(&hop_input);
            if !hop.blockers.is_empty() {
                return Err(ModDownloadFailure::Redirect(format!(
                    "redirect rejected: {:?}",
                    hop.blockers
                )));
            }
            if !visited.insert(next.clone()) {
                return Err(ModDownloadFailure::Redirect(
                    "redirect loop detected".into(),
                ));
            }
            redirects.push(next.clone());
            current = next;
            continue;
        }
        break response;
    };
    if !(200..300).contains(&response.status) {
        return Err(ModDownloadFailure::HttpStatus(response.status));
    }
    if response
        .content_length
        .is_some_and(|size| size > request.policy_input.hard_size_limit)
    {
        return Err(ModDownloadFailure::ContentLengthExceedsLimit(
            response.content_length.unwrap(),
        ));
    }
    let final_content_length = response.content_length;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut buffer = [0u8; CHUNK_SIZE];
    loop {
        let count = response
            .body
            .read(&mut buffer)
            .map_err(ModDownloadFailure::Io)?;
        if count == 0 {
            break;
        }
        received = received
            .checked_add(count as u64)
            .ok_or(ModDownloadFailure::StreamExceedsLimit(u64::MAX))?;
        if received > request.policy_input.hard_size_limit {
            return Err(ModDownloadFailure::StreamExceedsLimit(received));
        }
        hasher.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(ModDownloadFailure::Io)?;
    }
    if let Some(expected) = final_content_length.filter(|expected| *expected != received) {
        return Err(ModDownloadFailure::Truncated { expected, received });
    }
    output.sync_all().map_err(ModDownloadFailure::Io)?;
    let actual = digest_hex(&hasher.finalize());
    match request.policy_input.expected_hash.as_ref() {
        Some(expected)
            if request.policy_result.hash_class == ModDownloadHashClass::StrongExpectedHash =>
        {
            if !expected.value.eq_ignore_ascii_case(&actual) {
                return Err(ModDownloadFailure::HashMismatch {
                    expected: expected.value.clone(),
                    actual,
                });
            }
        }
        Some(_) if request.policy_result.hash_class == ModDownloadHashClass::UnsupportedHash => {
            return Err(ModDownloadFailure::UnsupportedExpectedHash);
        }
        _ => {}
    }
    let staged_size = fs::metadata(temporary)
        .map_err(ModDownloadFailure::Io)?
        .len();
    if staged_size != received {
        return Err(ModDownloadFailure::Truncated {
            expected: received,
            received: staged_size,
        });
    }
    Ok((
        actual,
        received,
        current,
        redirects,
        hosts,
        final_content_length,
    ))
}

fn ensure_directory(path: &Path, label: &str) -> Result<(), ModDownloadFailure> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ModDownloadFailure::StagingUnsafe(format!("{label} directory is unavailable: {error}"))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ModDownloadFailure::StagingUnsafe(format!(
            "{label} directory must be a real directory"
        )));
    }
    Ok(())
}

fn ensure_directory_or_create(path: &Path, label: &str) -> Result<(), ModDownloadFailure> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ModDownloadFailure::CacheUnsafe(format!(
                "{label} directory must be a real directory"
            )));
        }
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|error| {
        ModDownloadFailure::CacheUnsafe(format!("cannot create {label} directory: {error}"))
    })
}

fn publish_content_addressed(
    root: &Path,
    temporary: &Path,
    digest: &str,
    size: u64,
) -> Result<PathBuf, ModDownloadFailure> {
    let directory = root.join("sha256");
    ensure_directory_or_create(&directory, "SHA-256 cache")?;
    let destination = directory.join(digest);
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ModDownloadFailure::CacheConflict(destination));
        }
        if metadata.len() != size || sha256_file(&destination)? != digest {
            return Err(ModDownloadFailure::CacheConflict(destination));
        }
        fs::remove_file(temporary)?;
        return Ok(destination);
    }
    fs::hard_link(temporary, &destination).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            ModDownloadFailure::CacheConflict(destination.clone())
        } else {
            ModDownloadFailure::Io(error)
        }
    })?;
    fs::remove_file(temporary)?;
    Ok(destination)
}

fn sha256_file(path: &Path) -> Result<String, ModDownloadFailure> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; CHUNK_SIZE];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(digest_hex(&hasher.finalize()))
}

fn digest_hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemModResolver;

impl crate::identity_source::net_policy::HostResolver for SystemModResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        use std::net::ToSocketAddrs;
        (host, port)
            .to_socket_addrs()
            .map_err(|error| error.to_string())
            .map(|items| items.map(|item| item.ip()).collect())
    }
}

#[derive(Debug, Clone)]
pub struct UreqModDownloadBackend<R = SystemModResolver> {
    agent: ureq::Agent,
    _resolver: std::marker::PhantomData<R>,
}

impl UreqModDownloadBackend<SystemModResolver> {
    pub fn new() -> Self {
        Self::with_resolver(SystemModResolver)
    }
}

impl<R> UreqModDownloadBackend<R>
where
    R: crate::identity_source::net_policy::HostResolver
        + std::fmt::Debug
        + Send
        + Sync
        + Clone
        + 'static,
{
    pub fn with_resolver(resolver: R) -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(GLOBAL_TIMEOUT))
            .timeout_recv_body(Some(IDLE_TIMEOUT))
            .build();
        let agent = ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::DefaultConnector::default(),
            PolicyResolver(resolver.clone()),
        );
        Self {
            agent,
            _resolver: std::marker::PhantomData,
        }
    }
}

impl Default for UreqModDownloadBackend<SystemModResolver> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> ModDownloadBackend for UreqModDownloadBackend<R>
where
    R: crate::identity_source::net_policy::HostResolver
        + std::fmt::Debug
        + Send
        + Sync
        + Clone
        + 'static,
{
    fn get(&self, url: &str) -> Result<ModDownloadResponse, ModDownloadFailure> {
        let parsed = Url::parse(url).map_err(|error| ModDownloadFailure::Url(error.to_string()))?;
        if parsed.scheme() != "https" {
            return Err(ModDownloadFailure::Url("transport requires HTTPS".into()));
        }
        let response = self
            .agent
            .get(url)
            .header("Accept", "application/octet-stream")
            .header("Accept-Encoding", "identity")
            .header("User-Agent", concat!("EmuWiz/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(|error| match error {
                ureq::Error::Timeout(_) => ModDownloadFailure::Timeout,
                ureq::Error::Tls(error) => ModDownloadFailure::Tls(error.to_string()),
                other => ModDownloadFailure::Transport(other.to_string()),
            })?;
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|_| ModDownloadFailure::Transport("invalid Content-Length".into()))
            })
            .transpose()?;
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(ModDownloadResponse {
            status: response.status().as_u16(),
            content_length,
            location,
            body: Box::new(response.into_body().into_reader()),
        })
    }
}

#[derive(Debug, Clone)]
struct PolicyResolver<R>(R);

impl<R> ureq::unversioned::resolver::Resolver for PolicyResolver<R>
where
    R: crate::identity_source::net_policy::HostResolver
        + std::fmt::Debug
        + Send
        + Sync
        + Clone
        + 'static,
{
    fn resolve(
        &self,
        uri: &http::Uri,
        _config: &ureq::config::Config,
        _timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let authority = uri.authority().ok_or(ureq::Error::HostNotFound)?;
        let host = authority.host();
        let port = authority.port_u16().unwrap_or(443);
        let addresses = self
            .0
            .resolve(host, port)
            .map_err(|_| ureq::Error::HostNotFound)?;
        if addresses.is_empty()
            || addresses.len() > 16
            || addresses
                .iter()
                .any(|address| is_forbidden_resolved_address(*address))
        {
            return Err(ureq::Error::HostNotFound);
        }
        let mut output = <Self as ureq::unversioned::resolver::Resolver>::empty(self);
        for address in addresses {
            output.push(SocketAddr::new(address, port));
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::sync::Mutex;
    use tempfile::tempdir;

    struct FakeBackend(Mutex<VecDeque<Result<ModDownloadResponse, ModDownloadFailure>>>);
    impl FakeBackend {
        fn one(body: &[u8], content_length: Option<u64>) -> Self {
            Self(Mutex::new(VecDeque::from([Ok(ModDownloadResponse {
                status: 200,
                content_length,
                location: None,
                body: Box::new(Cursor::new(body.to_vec())),
            })])))
        }
    }
    impl ModDownloadBackend for FakeBackend {
        fn get(&self, _url: &str) -> Result<ModDownloadResponse, ModDownloadFailure> {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(ModDownloadFailure::Transport("no response".into())))
        }
    }

    fn request(root: &Path, expected: Option<ModCatalogueHash>) -> ModDownloadRequest {
        let input = ModDownloadPolicyInput {
            provider: "fixture".into(),
            provider_source_page_url: Some("https://catalogue.example.test/r".into()),
            payload_url: "https://mods.example.test/a.zip".into(),
            redirects: Vec::new(),
            declared_size: None,
            content_length: None,
            hard_size_limit: 1024,
            expected_hash: expected,
            payload_filename: Some("a.zip".into()),
            redirect_policy: crate::mod_download_policy::ModRedirectPolicy::AllowHttpsHostChange,
            max_redirects: 5,
            allow_http_with_review: false,
        };
        let policy_result = crate::mod_download_policy::evaluate_mod_download_policy(&input);
        fs::create_dir(root.join("cache")).unwrap();
        ModDownloadRequest {
            policy_input: input,
            policy_result,
            review_confirmed: true,
            staging_root: root.join("stage"),
            cache_root: root.join("cache"),
        }
    }

    #[test]
    fn bounded_success_publishes_content_addressed_bytes() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("stage")).unwrap();
        let result = download_mod_payload(
            &request(root.path(), None),
            &FakeBackend::one(b"payload", Some(7)),
        )
        .unwrap();
        assert_eq!(result.size_bytes, 7);
        assert_eq!(result.sha256, sha256_file(&result.path).unwrap());
        assert!(
            !root
                .path()
                .join("stage")
                .read_dir()
                .unwrap()
                .any(|entry| entry.is_ok())
        );
    }
    #[test]
    fn hash_mismatch_removes_staging_and_publishes_nothing() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("stage")).unwrap();
        let mut expected = "0".repeat(64);
        expected.replace_range(..1, "1");
        let request = request(
            root.path(),
            Some(ModCatalogueHash {
                algorithm: crate::mod_catalogue::ModCatalogueHashAlgorithm::Sha256,
                value: expected,
            }),
        );
        let error =
            download_mod_payload(&request, &FakeBackend::one(b"payload", Some(7))).unwrap_err();
        assert!(matches!(error, ModDownloadFailure::HashMismatch { .. }));
        assert!(!root.path().join("cache/sha256").exists());
    }
    #[test]
    fn content_length_and_stream_limits_are_enforced() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("stage")).unwrap();
        let mut request = request(root.path(), None);
        request.policy_input.content_length = Some(1025);
        request.policy_result =
            crate::mod_download_policy::evaluate_mod_download_policy(&request.policy_input);
        assert!(matches!(
            download_mod_payload(&request, &FakeBackend::one(b"x", Some(1025))),
            Err(ModDownloadFailure::PolicyNotApproved)
        ));
        request.policy_input.content_length = None;
        request.policy_input.hard_size_limit = 1;
        request.policy_result =
            crate::mod_download_policy::evaluate_mod_download_policy(&request.policy_input);
        assert!(matches!(
            download_mod_payload(&request, &FakeBackend::one(b"xx", None)),
            Err(ModDownloadFailure::StreamExceedsLimit(2))
        ));
    }
    #[test]
    fn redirect_to_private_literal_is_rejected_before_second_request() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("stage")).unwrap();
        let mut request = request(root.path(), None);
        request.policy_input.redirects = Vec::new();
        let policy =
            crate::mod_download_policy::evaluate_mod_download_policy(&request.policy_input);
        request.policy_result = policy;
        struct Redirect;
        impl ModDownloadBackend for Redirect {
            fn get(&self, _url: &str) -> Result<ModDownloadResponse, ModDownloadFailure> {
                Ok(ModDownloadResponse {
                    status: 302,
                    content_length: Some(0),
                    location: Some("https://127.0.0.1/x".into()),
                    body: Box::new(Cursor::new(Vec::new())),
                })
            }
        }
        assert!(matches!(
            download_mod_payload(&request, &Redirect),
            Err(ModDownloadFailure::Redirect(_))
        ));
    }
    #[test]
    fn resolved_address_policy_rejects_mixed_public_and_private_sets() {
        assert!(is_forbidden_resolved_address(
            "192.168.1.1".parse().unwrap()
        ));
        assert!(!is_forbidden_resolved_address(
            "93.184.216.34".parse().unwrap()
        ));
    }
}
