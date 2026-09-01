//! Curated, fail-closed emulator acquisition for the setup page.
//!
//! This is intentionally not a package manager. The catalogue contains only
//! fixed official sources, and the AppImage path accepts only a GitHub release
//! asset selected by a deterministic rule. No remote value is ever allowed
//! to choose a destination, and downloaded bytes are validated before an
//! atomic replacement in EmuWiz's own data directory.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_DOWNLOAD_BYTES: usize = 1_073_741_824;
pub const MIN_APPIMAGE_BYTES: usize = 1_048_576;
pub const MAX_RELEASE_METADATA_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_REDIRECTS: usize = 3;
const DOWNLOAD_CHUNK_BYTES: usize = 64 * 1024;
const MAX_PROGRESS_EVENTS: usize = 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
const HTTP_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EmulatorDistribution {
    GithubAppImage,
    Flatpak,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EmulatorDownloadSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub profile_name: &'static str,
    pub distribution: EmulatorDistribution,
    pub official_project: &'static str,
    pub project_url: &'static str,
    pub github_api_url: Option<&'static str>,
    pub flatpak_id: Option<&'static str>,
    pub asset_prefix: Option<&'static str>,
    pub installed_binary: &'static str,
}

/// One catalogue, rather than a second set of emulator registries spread
/// through GUI code. Manual entries remain visible for an honest next step.
pub const EMULATOR_DOWNLOAD_CATALOGUE: &[EmulatorDownloadSpec] = &[
    EmulatorDownloadSpec {
        id: "retroarch",
        display_name: "RetroArch",
        profile_name: "RetroArch",
        distribution: EmulatorDistribution::Flatpak,
        official_project: "RetroArch",
        project_url: "https://www.retroarch.com/",
        github_api_url: None,
        flatpak_id: Some("org.libretro.RetroArch"),
        asset_prefix: None,
        installed_binary: "retroarch",
    },
    EmulatorDownloadSpec {
        id: "pcsx2",
        display_name: "PCSX2",
        profile_name: "PCSX2",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "PCSX2",
        project_url: "https://github.com/PCSX2/pcsx2",
        github_api_url: Some("https://api.github.com/repos/PCSX2/pcsx2/releases/latest"),
        flatpak_id: None,
        asset_prefix: Some("pcsx2-"),
        installed_binary: "pcsx2.AppImage",
    },
    EmulatorDownloadSpec {
        id: "ppsspp",
        display_name: "PPSSPP",
        profile_name: "PPSSPP",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "PPSSPP",
        project_url: "https://github.com/hrydgard/ppsspp",
        github_api_url: Some("https://api.github.com/repos/hrydgard/ppsspp/releases/latest"),
        flatpak_id: None,
        asset_prefix: Some("PPSSPP-"),
        installed_binary: "ppsspp.AppImage",
    },
    EmulatorDownloadSpec {
        id: "rpcs3",
        display_name: "RPCS3",
        profile_name: "RPCS3",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "RPCS3",
        project_url: "https://github.com/RPCS3/rpcs3-binaries-linux",
        github_api_url: Some(
            "https://api.github.com/repos/RPCS3/rpcs3-binaries-linux/releases/latest",
        ),
        flatpak_id: None,
        asset_prefix: Some("rpcs3-"),
        installed_binary: "rpcs3.AppImage",
    },
    EmulatorDownloadSpec {
        id: "duckstation",
        display_name: "DuckStation",
        profile_name: "DuckStation",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "DuckStation",
        project_url: "https://github.com/stenzek/duckstation",
        github_api_url: Some(
            "https://api.github.com/repos/stenzek/duckstation/releases/tags/latest",
        ),
        flatpak_id: None,
        asset_prefix: Some("DuckStation-"),
        installed_binary: "duckstation.AppImage",
    },
    EmulatorDownloadSpec {
        id: "xemu",
        display_name: "xemu",
        profile_name: "xemu",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "xemu",
        project_url: "https://github.com/xemu-project/xemu",
        github_api_url: Some("https://api.github.com/repos/xemu-project/xemu/releases/latest"),
        flatpak_id: None,
        asset_prefix: Some("xemu-"),
        installed_binary: "xemu.AppImage",
    },
    EmulatorDownloadSpec {
        id: "dolphin",
        display_name: "Dolphin",
        profile_name: "Dolphin",
        distribution: EmulatorDistribution::Manual,
        official_project: "Dolphin Emulator",
        project_url: "https://dolphin-emu.org/download/",
        github_api_url: None,
        flatpak_id: None,
        asset_prefix: None,
        installed_binary: "dolphin-emu",
    },
    EmulatorDownloadSpec {
        id: "scummvm",
        display_name: "ScummVM",
        profile_name: "ScummVM",
        distribution: EmulatorDistribution::Manual,
        official_project: "ScummVM",
        project_url: "https://www.scummvm.org/downloads/",
        github_api_url: None,
        flatpak_id: None,
        asset_prefix: None,
        installed_binary: "scummvm",
    },
    EmulatorDownloadSpec {
        id: "shadps4",
        display_name: "shadPS4",
        profile_name: "shadPS4",
        distribution: EmulatorDistribution::Manual,
        official_project: "shadPS4",
        project_url: "https://github.com/shadps4-emu/shadPS4",
        github_api_url: None,
        flatpak_id: None,
        asset_prefix: None,
        installed_binary: "shadps4",
    },
];

pub fn emulator_download_spec(id: &str) -> Option<&'static EmulatorDownloadSpec> {
    EMULATOR_DOWNLOAD_CATALOGUE
        .iter()
        .find(|spec| spec.id == id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    #[serde(rename = "browser_download_url", alias = "download_url")]
    pub download_url: String,
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmulatorRelease {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseSelectionPolicy {
    /// Stable releases are required by default. Prereleases are never used
    /// unless a caller explicitly opts in.
    pub stable_only: bool,
}

impl Default for ReleaseSelectionPolicy {
    fn default() -> Self {
        Self { stable_only: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmulatorDownloadProgressPhase {
    ResolvingRelease,
    SelectingAsset,
    Downloading,
    Validating,
    Installing,
    Complete,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorDownloadProgress {
    pub phase: EmulatorDownloadProgressPhase,
    pub release_tag: Option<String>,
    pub asset_name: Option<String>,
    pub bytes_received: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct EmulatorDownloadCancellation(Arc<AtomicBool>);

impl EmulatorDownloadCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct EmulatorDownloadProgressReporter {
    callback: Arc<dyn Fn(EmulatorDownloadProgress) + Send + Sync>,
    emitted: Arc<AtomicUsize>,
}

impl std::fmt::Debug for EmulatorDownloadProgressReporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EmulatorDownloadProgressReporter")
            .field("emitted", &self.emitted.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl EmulatorDownloadProgressReporter {
    pub fn new(callback: impl Fn(EmulatorDownloadProgress) + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
            emitted: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn report(&self, progress: EmulatorDownloadProgress) {
        let index = self.emitted.fetch_add(1, Ordering::AcqRel);
        if index < MAX_PROGRESS_EVENTS {
            (self.callback)(progress);
        }
    }
}

#[derive(Clone, Copy)]
pub struct EmulatorTransferContext<'a> {
    pub cancellation: Option<&'a EmulatorDownloadCancellation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorHttpResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub location: Option<String>,
    pub bytes_received: u64,
}

/// Transport boundary for release metadata and asset bytes. Implementations
/// must stream successful response bodies into `destination`, enforce the
/// supplied limit while doing so, and return the actual byte count.
pub trait EmulatorDownloadTransport {
    fn get(
        &self,
        url: &str,
        maximum_bytes: u64,
        destination: &mut dyn Write,
        context: EmulatorTransferContext<'_>,
    ) -> Result<EmulatorHttpResponse, DownloadError>;
}

#[derive(Debug, Clone)]
pub struct HttpsEmulatorDownloadTransport {
    agent: ureq::Agent,
}

impl HttpsEmulatorDownloadTransport {
    #[must_use]
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(HTTP_TIMEOUT))
            .timeout_recv_body(Some(HTTP_IDLE_TIMEOUT))
            .build();
        Self {
            agent: config.new_agent(),
        }
    }
}

impl Default for HttpsEmulatorDownloadTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl EmulatorDownloadTransport for HttpsEmulatorDownloadTransport {
    fn get(
        &self,
        url: &str,
        maximum_bytes: u64,
        destination: &mut dyn Write,
        context: EmulatorTransferContext<'_>,
    ) -> Result<EmulatorHttpResponse, DownloadError> {
        if context
            .cancellation
            .is_some_and(EmulatorDownloadCancellation::is_cancelled)
        {
            return Err(DownloadError::Cancelled);
        }
        let parsed = url::Url::parse(url)
            .map_err(|error| DownloadError::InvalidAsset(format!("malformed URL: {error}")))?;
        if parsed.scheme() != "https" || parsed.username() != "" || parsed.password().is_some() {
            return Err(DownloadError::RedirectRejected(
                "emulator transport requires an HTTPS URL without credentials".into(),
            ));
        }
        let mut response = self
            .agent
            .get(url)
            .header("Accept", "application/json, application/octet-stream")
            .header("Accept-Encoding", "identity")
            .header(
                "User-Agent",
                concat!("archivefs/", env!("CARGO_PKG_VERSION")),
            )
            .call()
            .map_err(|error| DownloadError::Io(error.to_string()))?;
        let header_bytes = response
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum::<usize>();
        if header_bytes > MAX_HEADER_BYTES {
            return Err(DownloadError::Io(
                "response headers exceed the safety limit".into(),
            ));
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        if header("content-encoding").is_some_and(|value| !value.eq_ignore_ascii_case("identity")) {
            return Err(DownloadError::Io(
                "compressed transfer is not accepted".into(),
            ));
        }
        let content_length = header("content-length")
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|_| DownloadError::ContentLengthInvalid)
            })
            .transpose()?;
        if content_length.is_some_and(|length| length > maximum_bytes) {
            return Err(DownloadError::TooLarge);
        }
        let status = response.status().as_u16();
        let location = header("location");
        if !(200..300).contains(&status) {
            return Ok(EmulatorHttpResponse {
                status,
                content_length,
                location,
                bytes_received: 0,
            });
        }
        let mut reader = response.body_mut().as_reader();
        let mut buffer = [0u8; DOWNLOAD_CHUNK_BYTES];
        let mut received = 0u64;
        loop {
            if context
                .cancellation
                .is_some_and(EmulatorDownloadCancellation::is_cancelled)
            {
                return Err(DownloadError::Cancelled);
            }
            let count = reader
                .read(&mut buffer)
                .map_err(|error| DownloadError::Io(error.to_string()))?;
            if count == 0 {
                break;
            }
            received = received.saturating_add(count as u64);
            if received > maximum_bytes {
                return Err(DownloadError::TooLarge);
            }
            destination
                .write_all(&buffer[..count])
                .map_err(|error| DownloadError::Io(error.to_string()))?;
        }
        if let Some(expected) = content_length
            && expected != received
        {
            return Err(DownloadError::TruncatedTransfer { expected, received });
        }
        Ok(EmulatorHttpResponse {
            status,
            content_length,
            location,
            bytes_received: received,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Unsupported(String),
    InvalidAsset(String),
    InvalidRelease(String),
    AmbiguousRelease(String),
    ReleaseNotFound,
    RedirectRejected(String),
    RedirectLimit,
    ContentLengthInvalid,
    TruncatedTransfer { expected: u64, received: u64 },
    InvalidDigest(String),
    TooLarge,
    TooSmall,
    InvalidImage,
    ChecksumMismatch { expected: String, actual: String },
    Cancelled,
    HttpStatus(u16),
    Io(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(message)
            | Self::InvalidAsset(message)
            | Self::InvalidRelease(message)
            | Self::AmbiguousRelease(message)
            | Self::RedirectRejected(message)
            | Self::Io(message) => f.write_str(message),
            Self::ReleaseNotFound => f.write_str("no eligible emulator release was found"),
            Self::RedirectLimit => f.write_str("emulator download redirect limit exceeded"),
            Self::ContentLengthInvalid => f.write_str("response Content-Length is invalid"),
            Self::TruncatedTransfer { expected, received } => write!(
                f,
                "response was truncated: expected {expected} bytes, received {received}"
            ),
            Self::InvalidDigest(message) => f.write_str(message),
            Self::TooLarge => write!(
                f,
                "download exceeds the {} byte safety limit",
                MAX_DOWNLOAD_BYTES
            ),
            Self::TooSmall => f.write_str("download is too small to be an AppImage"),
            Self::InvalidImage => f.write_str("download is not a valid Linux AppImage/ELF"),
            Self::ChecksumMismatch { expected, actual } => {
                write!(f, "SHA-256 mismatch: expected {expected}, got {actual}")
            }
            Self::Cancelled => f.write_str("emulator download was cancelled"),
            Self::HttpStatus(status) => write!(f, "emulator server returned HTTP {status}"),
        }
    }
}

impl std::error::Error for DownloadError {}

pub fn select_release(
    releases: &[EmulatorRelease],
    policy: ReleaseSelectionPolicy,
) -> Result<EmulatorRelease, DownloadError> {
    let eligible: Vec<_> = releases
        .iter()
        .filter(|release| !release.draft && (!policy.stable_only || !release.prerelease))
        .cloned()
        .collect();
    match eligible.as_slice() {
        [] => Err(DownloadError::ReleaseNotFound),
        [release] => Ok(release.clone()),
        _ => Err(DownloadError::AmbiguousRelease(format!(
            "{} eligible releases remain; an explicit release is required",
            eligible.len()
        ))),
    }
}

#[derive(Debug, Clone, Default)]
pub struct EmulatorDownloadOptions {
    pub release_policy: ReleaseSelectionPolicy,
    pub cancellation: Option<EmulatorDownloadCancellation>,
    pub progress: Option<EmulatorDownloadProgressReporter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmulatorDownloadReceipt {
    pub emulator_id: String,
    pub release_tag: String,
    pub asset_name: String,
    pub installed_path: PathBuf,
    pub sha256: String,
    pub upstream_digest: Option<String>,
    pub digest_verified: bool,
}

/// Resolve, download, validate, and atomically install one official Linux
/// AppImage. This is the explicit side-effecting entry point; constructing a
/// catalogue/spec or an options value performs no network or filesystem I/O.
pub fn download_and_install_appimage(
    root: &Path,
    spec: &EmulatorDownloadSpec,
    transport: &dyn EmulatorDownloadTransport,
    options: &EmulatorDownloadOptions,
) -> Result<EmulatorDownloadReceipt, DownloadError> {
    if spec.distribution != EmulatorDistribution::GithubAppImage {
        return Err(DownloadError::Unsupported(
            "this emulator does not have an automated AppImage download lane".into(),
        ));
    }
    let release_url = spec.github_api_url.ok_or_else(|| {
        DownloadError::Unsupported("this emulator has no official release metadata URL".into())
    })?;
    if options
        .cancellation
        .as_ref()
        .is_some_and(EmulatorDownloadCancellation::is_cancelled)
    {
        report(
            options,
            EmulatorDownloadProgressPhase::Cancelled,
            None,
            None,
            0,
            None,
        );
        return Err(DownloadError::Cancelled);
    }
    report(
        options,
        EmulatorDownloadProgressPhase::ResolvingRelease,
        None,
        None,
        0,
        None,
    );
    ensure_not_cancelled(options)?;
    let release = fetch_release_metadata(release_url, spec, transport, options)?;
    let release = select_release(std::slice::from_ref(&release), options.release_policy)?;
    report(
        options,
        EmulatorDownloadProgressPhase::SelectingAsset,
        Some(&release.tag_name),
        None,
        0,
        None,
    );
    ensure_not_cancelled(options)?;
    let asset = select_x86_64_asset(spec, &release.assets)?;
    let expected_digest = asset
        .digest
        .as_deref()
        .map(normalize_sha256_digest)
        .transpose()?;

    let directory = prepare_install_directory(root, spec)?;
    let temporary = create_download_temporary(&directory, spec.installed_binary)?;
    let result = (|| {
        ensure_not_cancelled(options)?;
        let mut output = OpenOptions::new()
            .append(true)
            .open(&temporary)
            .map_err(io_error)?;
        report(
            options,
            EmulatorDownloadProgressPhase::Downloading,
            Some(&release.tag_name),
            Some(&asset.name),
            0,
            None,
        );
        let response = download_with_redirects(
            &asset.download_url,
            spec,
            EndpointKind::Asset,
            transport,
            &mut output,
            options,
        )?;
        output.sync_all().map_err(io_error)?;
        drop(output);
        if response.bytes_received == 0 {
            return Err(DownloadError::InvalidAsset(
                "asset response contained no bytes".into(),
            ));
        }
        report(
            options,
            EmulatorDownloadProgressPhase::Validating,
            Some(&release.tag_name),
            Some(&asset.name),
            response.bytes_received,
            response.content_length,
        );
        ensure_not_cancelled(options)?;
        let validation = validate_appimage_file(&temporary)?;
        if validation.size_bytes != response.bytes_received {
            return Err(DownloadError::TruncatedTransfer {
                expected: response.bytes_received,
                received: validation.size_bytes,
            });
        }
        if let Some(expected) = expected_digest.as_deref()
            && !expected.eq_ignore_ascii_case(&validation.sha256)
        {
            return Err(DownloadError::ChecksumMismatch {
                expected: expected.to_string(),
                actual: validation.sha256,
            });
        }
        report(
            options,
            EmulatorDownloadProgressPhase::Installing,
            Some(&release.tag_name),
            Some(&asset.name),
            validation.size_bytes,
            Some(validation.size_bytes),
        );
        ensure_not_cancelled(options)?;
        let destination = install_validated_file(
            &directory,
            spec,
            &temporary,
            &validation.sha256,
            &release.tag_name,
            expected_digest.as_deref(),
        )?;
        report(
            options,
            EmulatorDownloadProgressPhase::Complete,
            Some(&release.tag_name),
            Some(&asset.name),
            validation.size_bytes,
            Some(validation.size_bytes),
        );
        Ok(EmulatorDownloadReceipt {
            emulator_id: spec.id.to_string(),
            release_tag: release.tag_name,
            asset_name: asset.name,
            installed_path: destination,
            sha256: validation.sha256,
            upstream_digest: asset.digest,
            digest_verified: expected_digest.is_some(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        if matches!(result, Err(DownloadError::Cancelled)) {
            report(
                options,
                EmulatorDownloadProgressPhase::Cancelled,
                None,
                None,
                0,
                None,
            );
        }
    }
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileValidation {
    size_bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointKind {
    Metadata,
    Asset,
}

fn fetch_release_metadata(
    url: &str,
    spec: &EmulatorDownloadSpec,
    transport: &dyn EmulatorDownloadTransport,
    options: &EmulatorDownloadOptions,
) -> Result<EmulatorRelease, DownloadError> {
    validate_endpoint_url(url, spec, EndpointKind::Metadata, true)?;
    let mut bytes = Vec::new();
    let response = download_with_redirects(
        url,
        spec,
        EndpointKind::Metadata,
        transport,
        &mut bytes,
        options,
    )?;
    if response.bytes_received == 0 {
        return Err(DownloadError::InvalidRelease(
            "release metadata response was empty".into(),
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        DownloadError::InvalidRelease(format!("release metadata is invalid: {error}"))
    })
}

fn download_with_redirects(
    initial_url: &str,
    spec: &EmulatorDownloadSpec,
    kind: EndpointKind,
    transport: &dyn EmulatorDownloadTransport,
    destination: &mut dyn Write,
    options: &EmulatorDownloadOptions,
) -> Result<EmulatorHttpResponse, DownloadError> {
    let maximum = match kind {
        EndpointKind::Metadata => MAX_RELEASE_METADATA_BYTES as u64,
        EndpointKind::Asset => MAX_DOWNLOAD_BYTES as u64,
    };
    let mut current = initial_url.to_string();
    let mut visited = HashSet::new();
    for redirect_count in 0..=MAX_REDIRECTS {
        ensure_not_cancelled(options)?;
        validate_endpoint_url(&current, spec, kind, redirect_count == 0)?;
        if !visited.insert(current.clone()) {
            return Err(DownloadError::RedirectRejected(
                "redirect loop detected".into(),
            ));
        }
        let response = transport.get(
            &current,
            maximum,
            destination,
            EmulatorTransferContext {
                cancellation: options.cancellation.as_ref(),
            },
        )?;
        if (300..400).contains(&response.status) {
            if response.bytes_received != 0 {
                return Err(DownloadError::RedirectRejected(
                    "redirect response unexpectedly contained a body".into(),
                ));
            }
            if redirect_count == MAX_REDIRECTS {
                return Err(DownloadError::RedirectLimit);
            }
            let location = response.location.ok_or_else(|| {
                DownloadError::RedirectRejected("redirect omitted Location".into())
            })?;
            let base = url::Url::parse(&current)
                .map_err(|error| DownloadError::RedirectRejected(error.to_string()))?;
            current = base
                .join(&location)
                .map_err(|error| DownloadError::RedirectRejected(error.to_string()))?
                .to_string();
            continue;
        }
        if !(200..300).contains(&response.status) {
            return Err(DownloadError::HttpStatus(response.status));
        }
        if let Some(expected) = response.content_length
            && expected != response.bytes_received
        {
            return Err(DownloadError::TruncatedTransfer {
                expected,
                received: response.bytes_received,
            });
        }
        return Ok(response);
    }
    Err(DownloadError::RedirectLimit)
}

fn validate_endpoint_url(
    value: &str,
    spec: &EmulatorDownloadSpec,
    kind: EndpointKind,
    initial: bool,
) -> Result<(), DownloadError> {
    let url = url::Url::parse(value)
        .map_err(|error| DownloadError::RedirectRejected(format!("invalid URL: {error}")))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(DownloadError::RedirectRejected(
            "emulator downloads require HTTPS URLs without credentials or fragments".into(),
        ));
    }
    let host = url.host_str().unwrap_or_default();
    match kind {
        EndpointKind::Metadata => {
            if host != "api.github.com" || (initial && spec.github_api_url != Some(value)) {
                return Err(DownloadError::RedirectRejected(
                    "release metadata URL is outside the GitHub API allowlist".into(),
                ));
            }
        }
        EndpointKind::Asset => {
            let project_path = url::Url::parse(spec.project_url)
                .ok()
                .map(|project| project.path().trim_end_matches('/').to_string());
            let initial_project = project_path
                .is_some_and(|path| url.path().starts_with(&format!("{path}/releases/")));
            let allowed_redirect_host = matches!(
                host,
                "github.com"
                    | "objects.githubusercontent.com"
                    | "release-assets.githubusercontent.com"
                    | "githubusercontent.com"
            );
            if (initial && (host != "github.com" || !initial_project))
                || (!initial && host == "github.com" && !initial_project)
                || (!initial && !allowed_redirect_host)
            {
                return Err(DownloadError::RedirectRejected(
                    "asset URL is outside the official GitHub download allowlist".into(),
                ));
            }
        }
    }
    Ok(())
}

fn report(
    options: &EmulatorDownloadOptions,
    phase: EmulatorDownloadProgressPhase,
    release_tag: Option<&str>,
    asset_name: Option<&str>,
    bytes_received: u64,
    total_bytes: Option<u64>,
) {
    if let Some(reporter) = &options.progress {
        reporter.report(EmulatorDownloadProgress {
            phase,
            release_tag: release_tag.map(str::to_string),
            asset_name: asset_name.map(str::to_string),
            bytes_received,
            total_bytes,
        });
    }
}

fn ensure_not_cancelled(options: &EmulatorDownloadOptions) -> Result<(), DownloadError> {
    if options
        .cancellation
        .as_ref()
        .is_some_and(EmulatorDownloadCancellation::is_cancelled)
    {
        Err(DownloadError::Cancelled)
    } else {
        Ok(())
    }
}

pub fn select_x86_64_asset(
    spec: &EmulatorDownloadSpec,
    assets: &[ReleaseAsset],
) -> Result<ReleaseAsset, DownloadError> {
    if spec.distribution != EmulatorDistribution::GithubAppImage {
        return Err(DownloadError::Unsupported(
            "this emulator is not distributed through the automated AppImage lane".into(),
        ));
    }
    let prefix = spec.asset_prefix.ok_or_else(|| {
        DownloadError::Unsupported("no deterministic asset rule is configured".into())
    })?;
    let matches: Vec<_> = assets
        .iter()
        .filter(|asset| {
            let lower = asset.name.to_ascii_lowercase();
            asset.name.starts_with(prefix)
                && (lower.ends_with(".appimage") || lower.ends_with(".appimage.x86_64"))
                && (lower.contains("x86_64") || lower.contains("x64"))
                && !["arm", "aarch64", "debug", "zsync", "checksum", "sha256"]
                    .iter()
                    .any(|bad| lower.contains(bad))
        })
        .cloned()
        .collect();
    if matches.len() != 1 {
        return Err(DownloadError::InvalidAsset(format!(
            "expected exactly one deterministic Linux x86_64 asset, found {}",
            matches.len()
        )));
    }
    let asset = matches.into_iter().next().expect("checked one match");
    let url = url::Url::parse(&asset.download_url)
        .map_err(|_| DownloadError::InvalidAsset("asset URL is malformed".into()))?;
    let project_path = url::Url::parse(spec.project_url)
        .ok()
        .map(|project| project.path().trim_end_matches('/').to_string());
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || project_path.is_none_or(|path| !url.path().starts_with(&format!("{path}/releases/")))
    {
        return Err(DownloadError::InvalidAsset(
            "asset URL is not an allowlisted HTTPS GitHub host".into(),
        ));
    }
    Ok(asset)
}

fn normalize_sha256_digest(value: &str) -> Result<String, DownloadError> {
    let digest = value
        .strip_prefix("sha256:")
        .or_else(|| value.strip_prefix("sha256="))
        .unwrap_or(value)
        .trim();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DownloadError::InvalidDigest(format!(
            "upstream digest is not a valid SHA-256 value: {value}"
        )));
    }
    Ok(digest.to_ascii_lowercase())
}

fn validate_appimage_file(path: &Path) -> Result<FileValidation, DownloadError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DownloadError::InvalidAsset(
            "downloaded AppImage path is not a regular file".into(),
        ));
    }
    if metadata.len() > MAX_DOWNLOAD_BYTES as u64 {
        return Err(DownloadError::TooLarge);
    }
    if metadata.len() < MIN_APPIMAGE_BYTES as u64 {
        return Err(DownloadError::TooSmall);
    }
    let mut file = File::open(path).map_err(io_error)?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header).map_err(io_error)?;
    if header != *b"\x7fELF" {
        return Err(DownloadError::InvalidImage);
    }
    let mut hash = Sha256::new();
    hash.update(header);
    let mut buffer = [0u8; DOWNLOAD_CHUNK_BYTES];
    let mut read_bytes = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        read_bytes = read_bytes.saturating_add(count as u64);
        hash.update(&buffer[..count]);
    }
    Ok(FileValidation {
        size_bytes: read_bytes.saturating_add(4),
        sha256: hex_digest(hash),
    })
}

fn hex_digest(hash: Sha256) -> String {
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn prepare_install_directory(
    root: &Path,
    spec: &EmulatorDownloadSpec,
) -> Result<PathBuf, DownloadError> {
    ensure_directory(root)?;
    let emulators = root.join("emulators");
    ensure_directory(&emulators)?;
    let directory = emulators.join(spec.id);
    ensure_directory(&directory)?;
    Ok(directory)
}

fn ensure_directory(path: &Path) -> Result<(), DownloadError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(DownloadError::Unsupported(format!(
                "installation path is not a real directory: {}",
                path.display()
            )))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(io_error)
        }
        Err(error) => Err(io_error(error)),
    }
}

fn create_download_temporary(directory: &Path, binary: &str) -> Result<PathBuf, DownloadError> {
    for nonce in 0..100u16 {
        let path = directory.join(format!(".{binary}.download-{}-{nonce}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
    Err(DownloadError::Io(
        "could not allocate a unique emulator download temporary file".into(),
    ))
}

fn install_validated_file(
    directory: &Path,
    spec: &EmulatorDownloadSpec,
    temporary: &Path,
    sha256: &str,
    release_tag: &str,
    upstream_digest: Option<&str>,
) -> Result<PathBuf, DownloadError> {
    let destination = directory.join(spec.installed_binary);
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(DownloadError::Unsupported(
                "existing emulator destination is not a regular file".into(),
            ));
        }
        Ok(_) if !is_emuwiz_managed(directory) => {
            return Err(DownloadError::Unsupported(
                "an existing installation is not marked EmuWiz-managed; it was left untouched"
                    .into(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    let provenance = InstallProvenance {
        emulator: spec.id.to_string(),
        version: release_tag.to_string(),
        official_source: spec.project_url.to_string(),
        installed_path: spec.installed_binary.to_string(),
        sha256: sha256.to_string(),
        upstream_digest: upstream_digest.map(str::to_string),
    };
    let provenance_bytes = serde_json::to_vec_pretty(&provenance)
        .map_err(|error| DownloadError::Io(error.to_string()))?;
    let provenance_temporary =
        directory.join(format!(".install-{}.json-{}", spec.id, std::process::id()));
    let mut metadata_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&provenance_temporary)
        .map_err(io_error)?;
    if let Err(error) = metadata_file
        .write_all(&provenance_bytes)
        .and_then(|_| metadata_file.sync_all())
    {
        let _ = fs::remove_file(&provenance_temporary);
        return Err(io_error(error));
    }
    drop(metadata_file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temporary, fs::Permissions::from_mode(0o755)).map_err(io_error)?;
    }
    fs::rename(temporary, &destination).map_err(|error| {
        let _ = fs::remove_file(&provenance_temporary);
        io_error(error)
    })?;
    if let Err(error) = fs::rename(&provenance_temporary, directory.join("install.json")) {
        let _ = fs::remove_file(&provenance_temporary);
        return Err(io_error(error));
    }
    Ok(destination)
}

pub fn validate_appimage(bytes: &[u8]) -> Result<String, DownloadError> {
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(DownloadError::TooLarge);
    }
    if bytes.len() < MIN_APPIMAGE_BYTES {
        return Err(DownloadError::TooSmall);
    }
    if !bytes.starts_with(b"\x7fELF") {
        return Err(DownloadError::InvalidImage);
    }
    Ok(sha256_hex(bytes))
}

pub fn install_appimage_at(
    root: &Path,
    spec: &EmulatorDownloadSpec,
    bytes: &[u8],
    expected_digest: Option<&str>,
) -> Result<PathBuf, DownloadError> {
    let digest = validate_appimage(bytes)?;
    if let Some(expected) =
        expected_digest.map(|value| value.strip_prefix("sha256:").unwrap_or(value))
        && !expected.eq_ignore_ascii_case(&digest)
    {
        return Err(DownloadError::ChecksumMismatch {
            expected: expected.to_string(),
            actual: digest,
        });
    }
    let directory = prepare_install_directory(root, spec)?;
    let destination = directory.join(spec.installed_binary);
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(DownloadError::Unsupported(
                "existing emulator destination is not a regular file".into(),
            ));
        }
        if !is_emuwiz_managed(&directory) {
            return Err(DownloadError::Unsupported(
                "an existing installation is not marked EmuWiz-managed; it was left untouched"
                    .into(),
            ));
        }
    }
    let temporary = directory.join(format!(".{}.download", spec.installed_binary));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(io_error(error));
    }
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)).map_err(io_error)?;
    }
    fs::rename(&temporary, &destination).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        io_error(error)
    })?;
    let provenance = InstallProvenance {
        emulator: spec.id.to_string(),
        version: "release asset".to_string(),
        official_source: spec.project_url.to_string(),
        installed_path: spec.installed_binary.to_string(),
        sha256: digest,
        upstream_digest: None,
    };
    let provenance_bytes = serde_json::to_vec_pretty(&provenance)
        .map_err(|error| DownloadError::Io(error.to_string()))?;
    fs::write(directory.join("install.json"), provenance_bytes).map_err(io_error)?;
    Ok(destination)
}

#[derive(Debug, Serialize, Deserialize)]
struct InstallProvenance {
    emulator: String,
    version: String,
    official_source: String,
    installed_path: String,
    sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_digest: Option<String>,
}

fn is_emuwiz_managed(directory: &Path) -> bool {
    fs::symlink_metadata(directory.join("install.json"))
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

fn io_error(error: io::Error) -> DownloadError {
    DownloadError::Io(error.to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    fn spec() -> &'static EmulatorDownloadSpec {
        emulator_download_spec("pcsx2").unwrap()
    }

    fn image() -> Vec<u8> {
        let mut bytes = vec![0u8; MIN_APPIMAGE_BYTES];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes
    }

    fn release(assets: Vec<ReleaseAsset>) -> EmulatorRelease {
        EmulatorRelease {
            tag_name: "v1.2.3".into(),
            name: Some("Stable release".into()),
            draft: false,
            prerelease: false,
            published_at: Some("2026-09-01T00:00:00Z".into()),
            assets,
        }
    }

    fn asset(digest: Option<String>) -> ReleaseAsset {
        ReleaseAsset {
            name: "pcsx2-x86_64.AppImage".into(),
            download_url:
                "https://github.com/PCSX2/pcsx2/releases/download/v1.2.3/pcsx2-x86_64.AppImage"
                    .into(),
            digest,
        }
    }

    fn metadata_response(release: &EmulatorRelease) -> MockResponse {
        let body = serde_json::to_vec(release).unwrap();
        MockResponse {
            status: 200,
            content_length: Some(body.len() as u64),
            location: None,
            body,
        }
    }

    fn asset_response(body: Vec<u8>) -> MockResponse {
        MockResponse {
            status: 200,
            content_length: Some(body.len() as u64),
            location: None,
            body,
        }
    }

    #[derive(Debug)]
    struct MockResponse {
        status: u16,
        content_length: Option<u64>,
        location: Option<String>,
        body: Vec<u8>,
    }

    struct MockTransport {
        responses: Mutex<VecDeque<MockResponse>>,
        calls: Mutex<Vec<String>>,
    }

    impl MockTransport {
        fn new(responses: Vec<MockResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl EmulatorDownloadTransport for MockTransport {
        fn get(
            &self,
            url: &str,
            maximum_bytes: u64,
            destination: &mut dyn Write,
            context: EmulatorTransferContext<'_>,
        ) -> Result<EmulatorHttpResponse, DownloadError> {
            self.calls.lock().unwrap().push(url.to_string());
            if context
                .cancellation
                .is_some_and(EmulatorDownloadCancellation::is_cancelled)
            {
                return Err(DownloadError::Cancelled);
            }
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("mock response was not configured");
            if response
                .content_length
                .is_some_and(|length| length > maximum_bytes)
            {
                return Err(DownloadError::TooLarge);
            }
            if !(200..300).contains(&response.status) {
                return Ok(EmulatorHttpResponse {
                    status: response.status,
                    content_length: response.content_length,
                    location: response.location,
                    bytes_received: 0,
                });
            }
            if response.body.len() as u64 > maximum_bytes {
                return Err(DownloadError::TooLarge);
            }
            destination.write_all(&response.body).unwrap();
            Ok(EmulatorHttpResponse {
                status: response.status,
                content_length: response.content_length,
                location: response.location,
                bytes_received: response.body.len() as u64,
            })
        }
    }

    #[test]
    fn catalogue_has_one_entry_for_each_supported_emulator() {
        for id in [
            "retroarch",
            "dolphin",
            "pcsx2",
            "ppsspp",
            "rpcs3",
            "duckstation",
            "xemu",
            "scummvm",
            "shadps4",
        ] {
            assert!(emulator_download_spec(id).is_some(), "missing {id}");
        }
    }

    #[test]
    fn asset_selection_rejects_wrong_architecture_and_unexpected_names() {
        let assets = vec![
            ReleaseAsset {
                name: "pcsx2-arm64.AppImage".into(),
                download_url:
                    "https://github.com/PCSX2/pcsx2/releases/download/x/pcsx2-arm64.AppImage".into(),
                digest: None,
            },
            ReleaseAsset {
                name: "pcsx2-debug-x86_64.AppImage".into(),
                download_url:
                    "https://github.com/PCSX2/pcsx2/releases/download/x/pcsx2-debug-x86_64.AppImage"
                        .into(),
                digest: None,
            },
        ];
        assert!(matches!(
            select_x86_64_asset(spec(), &assets),
            Err(DownloadError::InvalidAsset(_))
        ));
    }

    #[test]
    fn asset_selection_prefers_one_appimage_over_archives_but_refuses_ambiguity() {
        let mut app = asset(None);
        let mut archive = app.clone();
        archive.name = "pcsx2-x86_64.tar.xz".into();
        assert_eq!(
            select_x86_64_asset(spec(), &[app.clone(), archive]).unwrap(),
            app
        );
        app.name = "pcsx2-another-x86_64.AppImage".into();
        assert!(matches!(
            select_x86_64_asset(spec(), &[asset(None), app]),
            Err(DownloadError::InvalidAsset(_))
        ));
    }

    #[test]
    fn missing_linux_asset_is_refused_instead_of_using_an_archive_or_other_architecture() {
        let assets = vec![ReleaseAsset {
            name: "pcsx2-arm64.tar.xz".into(),
            download_url: "https://github.com/PCSX2/pcsx2/releases/download/v1/pcsx2-arm64.tar.xz"
                .into(),
            digest: None,
        }];
        assert!(matches!(
            select_x86_64_asset(spec(), &assets),
            Err(DownloadError::InvalidAsset(_))
        ));
    }

    #[test]
    fn release_selection_ignores_prereleases_and_drafts_when_stable_is_required() {
        let stable = release(vec![asset(None)]);
        let mut prerelease = stable.clone();
        prerelease.tag_name = "v1.3.0-rc1".into();
        prerelease.prerelease = true;
        let mut draft = stable.clone();
        draft.tag_name = "v1.4.0-draft".into();
        draft.draft = true;
        assert_eq!(
            select_release(
                &[stable.clone(), prerelease, draft],
                ReleaseSelectionPolicy::default()
            )
            .unwrap(),
            stable
        );
        assert!(matches!(
            select_release(
                &[stable, release(vec![asset(None)])],
                ReleaseSelectionPolicy::default()
            ),
            Err(DownloadError::AmbiguousRelease(_))
        ));
    }

    #[test]
    fn release_selection_can_explicitly_allow_one_prerelease() {
        let mut prerelease = release(vec![asset(None)]);
        prerelease.prerelease = true;
        assert_eq!(
            select_release(
                &[prerelease.clone()],
                ReleaseSelectionPolicy { stable_only: false }
            )
            .unwrap(),
            prerelease
        );
    }

    #[test]
    fn valid_asset_is_selected_only_from_allowlisted_https_hosts() {
        let assets = vec![ReleaseAsset {
            name: "pcsx2-x86_64.AppImage".into(),
            download_url:
                "https://github.com/PCSX2/pcsx2/releases/download/x/pcsx2-x86_64.AppImage".into(),
            digest: None,
        }];
        assert!(select_x86_64_asset(spec(), &assets).is_ok());
        let mut bad = assets;
        bad[0].download_url = "https://example.invalid/pcsx2-x86_64.AppImage".into();
        assert!(select_x86_64_asset(spec(), &bad).is_err());
    }

    #[test]
    fn metadata_uses_github_browser_download_url_shape() {
        let json = serde_json::to_string(&asset(None)).unwrap();
        assert!(json.contains("browser_download_url"));
        assert!(!json.contains("\"download_url\""));
    }

    #[test]
    fn invalid_and_oversized_downloads_fail_before_install() {
        assert_eq!(
            validate_appimage(b"not an appimage"),
            Err(DownloadError::TooSmall)
        );
        let mut bytes = image();
        bytes[0] = b'N';
        assert_eq!(validate_appimage(&bytes), Err(DownloadError::InvalidImage));
    }

    #[test]
    fn install_is_atomic_and_records_provenance() {
        let root = tempfile::tempdir().unwrap();
        let bytes = image();
        let destination = install_appimage_at(root.path(), spec(), &bytes, None).unwrap();
        assert!(destination.is_file());
        assert!(root.path().join("emulators/pcsx2/install.json").is_file());
        assert_eq!(std::fs::read(destination).unwrap(), bytes);
    }

    #[test]
    fn unmanaged_existing_install_is_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("emulators/pcsx2");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(spec().installed_binary), b"user file").unwrap();
        assert!(matches!(
            install_appimage_at(root.path(), spec(), &image(), None),
            Err(DownloadError::Unsupported(_))
        ));
        assert_eq!(
            std::fs::read(directory.join(spec().installed_binary)).unwrap(),
            b"user file"
        );
    }

    #[test]
    fn streamed_transport_enforces_the_advertised_and_actual_limits() {
        let transport = MockTransport::new(vec![MockResponse {
            status: 200,
            content_length: Some(5),
            location: None,
            body: vec![1, 2, 3, 4, 5],
        }]);
        assert_eq!(
            transport
                .get(
                    "https://example.test/asset",
                    4,
                    &mut Vec::new(),
                    EmulatorTransferContext { cancellation: None }
                )
                .unwrap_err(),
            DownloadError::TooLarge
        );
        let transport = MockTransport::new(vec![MockResponse {
            status: 200,
            content_length: None,
            location: None,
            body: vec![1, 2, 3, 4, 5],
        }]);
        assert_eq!(
            transport
                .get(
                    "https://example.test/asset",
                    4,
                    &mut Vec::new(),
                    EmulatorTransferContext { cancellation: None }
                )
                .unwrap_err(),
            DownloadError::TooLarge
        );
    }

    #[test]
    fn successful_service_download_is_bounded_atomic_and_honest_about_missing_digest() {
        let root = tempfile::tempdir().unwrap();
        let body = image();
        let transport = MockTransport::new(vec![
            metadata_response(&release(vec![asset(None)])),
            asset_response(body.clone()),
        ]);
        let receipt = download_and_install_appimage(
            root.path(),
            spec(),
            &transport,
            &EmulatorDownloadOptions::default(),
        )
        .unwrap();
        assert_eq!(receipt.release_tag, "v1.2.3");
        assert_eq!(receipt.asset_name, "pcsx2-x86_64.AppImage");
        assert_eq!(receipt.upstream_digest, None);
        assert!(!receipt.digest_verified);
        assert_eq!(fs::read(&receipt.installed_path).unwrap(), body);
        assert!(root.path().join("emulators/pcsx2/install.json").is_file());
        assert_eq!(transport.calls().len(), 2);
        assert!(
            !root
                .path()
                .join("emulators/pcsx2")
                .read_dir()
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("download-"))
        );
    }

    #[test]
    fn digest_mismatch_refuses_install_and_preserves_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let original = image();
        let destination = install_appimage_at(root.path(), spec(), &original, None).unwrap();
        let mut different = image();
        different[42] = 1;
        let release = release(vec![asset(Some(format!(
            "sha256:{}",
            sha256_hex(&original)
        )))]);
        let transport =
            MockTransport::new(vec![metadata_response(&release), asset_response(different)]);
        assert!(matches!(
            download_and_install_appimage(
                root.path(),
                spec(),
                &transport,
                &EmulatorDownloadOptions::default()
            ),
            Err(DownloadError::ChecksumMismatch { .. })
        ));
        assert_eq!(fs::read(destination).unwrap(), original);
    }

    #[test]
    fn invalid_appimage_preserves_existing_install_and_leaves_no_partial() {
        let root = tempfile::tempdir().unwrap();
        let original = image();
        let destination = install_appimage_at(root.path(), spec(), &original, None).unwrap();
        let invalid = vec![b'N'; MIN_APPIMAGE_BYTES];
        let transport = MockTransport::new(vec![
            metadata_response(&release(vec![asset(None)])),
            asset_response(invalid),
        ]);
        assert_eq!(
            download_and_install_appimage(
                root.path(),
                spec(),
                &transport,
                &EmulatorDownloadOptions::default()
            )
            .unwrap_err(),
            DownloadError::InvalidImage
        );
        assert_eq!(fs::read(destination).unwrap(), original);
        assert!(
            !root
                .path()
                .join("emulators/pcsx2")
                .read_dir()
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("download-"))
        );
    }

    #[test]
    fn truncated_response_is_rejected_before_install() {
        let root = tempfile::tempdir().unwrap();
        let body = image();
        let mut response = asset_response(body);
        response.content_length = Some(MIN_APPIMAGE_BYTES as u64 + 1);
        let transport = MockTransport::new(vec![
            metadata_response(&release(vec![asset(None)])),
            response,
        ]);
        assert!(matches!(
            download_and_install_appimage(
                root.path(),
                spec(),
                &transport,
                &EmulatorDownloadOptions::default()
            ),
            Err(DownloadError::TruncatedTransfer { .. })
        ));
        assert!(!root.path().join("emulators/pcsx2/pcsx2.AppImage").exists());
    }

    #[test]
    fn unsafe_redirect_and_redirect_loop_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let unsafe_redirect = MockTransport::new(vec![
            metadata_response(&release(vec![asset(None)])),
            MockResponse {
                status: 302,
                content_length: Some(0),
                location: Some("http://evil.test/asset".into()),
                body: Vec::new(),
            },
        ]);
        assert!(matches!(
            download_and_install_appimage(
                root.path(),
                spec(),
                &unsafe_redirect,
                &EmulatorDownloadOptions::default()
            ),
            Err(DownloadError::RedirectRejected(_))
        ));
        let loop_transport = MockTransport::new(vec![MockResponse {
            status: 302,
            content_length: Some(0),
            location: Some(spec().github_api_url.unwrap().into()),
            body: Vec::new(),
        }]);
        let mut bytes = Vec::new();
        assert!(matches!(
            download_with_redirects(
                spec().github_api_url.unwrap(),
                spec(),
                EndpointKind::Metadata,
                &loop_transport,
                &mut bytes,
                &EmulatorDownloadOptions::default()
            ),
            Err(DownloadError::RedirectRejected(_))
        ));
    }

    #[test]
    fn cancellation_emits_cancelled_and_never_creates_an_install() {
        let root = tempfile::tempdir().unwrap();
        let cancellation = EmulatorDownloadCancellation::default();
        cancellation.cancel();
        let phases = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&phases);
        let options = EmulatorDownloadOptions {
            cancellation: Some(cancellation),
            progress: Some(EmulatorDownloadProgressReporter::new(move |event| {
                observed.lock().unwrap().push(event.phase);
            })),
            ..Default::default()
        };
        assert_eq!(
            download_and_install_appimage(
                root.path(),
                spec(),
                &MockTransport::new(Vec::new()),
                &options
            )
            .unwrap_err(),
            DownloadError::Cancelled
        );
        assert_eq!(
            *phases.lock().unwrap(),
            vec![EmulatorDownloadProgressPhase::Cancelled]
        );
        assert!(!root.path().join("emulators").exists());
    }

    #[test]
    fn progress_events_follow_resolution_selection_download_validation_installation() {
        let root = tempfile::tempdir().unwrap();
        let phases = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&phases);
        let options = EmulatorDownloadOptions {
            progress: Some(EmulatorDownloadProgressReporter::new(move |event| {
                observed.lock().unwrap().push(event.phase);
            })),
            ..Default::default()
        };
        download_and_install_appimage(
            root.path(),
            spec(),
            &MockTransport::new(vec![
                metadata_response(&release(vec![asset(None)])),
                asset_response(image()),
            ]),
            &options,
        )
        .unwrap();
        assert_eq!(
            *phases.lock().unwrap(),
            vec![
                EmulatorDownloadProgressPhase::ResolvingRelease,
                EmulatorDownloadProgressPhase::SelectingAsset,
                EmulatorDownloadProgressPhase::Downloading,
                EmulatorDownloadProgressPhase::Validating,
                EmulatorDownloadProgressPhase::Installing,
                EmulatorDownloadProgressPhase::Complete,
            ]
        );
    }

    #[test]
    fn manual_emulators_have_no_automated_download_lane() {
        for id in ["dolphin", "scummvm", "shadps4"] {
            assert_eq!(
                emulator_download_spec(id).unwrap().distribution,
                EmulatorDistribution::Manual
            );
        }
    }
}
