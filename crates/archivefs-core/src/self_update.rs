//! Recovered from feature/archivefs-unified-platform at eba83521.
//! Explicit, check-only official EmuWiz release metadata; no files, artifacts,
//! installer, configuration, process execution or automatic startup check.
//! Reuses the existing HTTPS transport, including its shared proxy opt-out.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::emulator_download::{DownloadError, EmulatorDownloadTransport, EmulatorTransferContext};

/// The official EmuWiz project - see `README.md`'s own release
/// instructions and this repository's `origin` remote, both of which name
/// this exact repository. Not a guess: verified against current repo
/// metadata.
pub const EMUWIZ_OFFICIAL_REPOSITORY: &str = "https://github.com/kiehntre/emuwiz";

/// The one allowlisted initial request this module ever makes. GitHub's own
/// `/releases/latest` endpoint already excludes drafts and prereleases;
/// [`fetch_latest_emuwiz_release`] additionally checks `draft`/`prerelease`
/// itself rather than trusting that silently.
pub const EMUWIZ_RELEASE_API_URL: &str =
    "https://api.github.com/repos/kiehntre/emuwiz/releases/latest";

const MAX_RELEASE_METADATA_BYTES: u64 = 2 * 1024 * 1024;

/// Truthful, non-guessed self-update status. Every `*Unknown`/`CheckFailed`
/// variant exists so the GUI never has to invent an answer this backend
/// could not safely give.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmuWizUpdateStatus {
    /// The running version equals the latest published release.
    UpToDate,
    /// The latest published release is newer than the running version.
    UpdateAvailable,
    /// The running version string could not be parsed as semver.
    CurrentVersionUnknown,
    /// The latest release's tag could not be parsed as semver.
    LatestVersionUnknown,
    /// The running version is *newer* than the latest published release
    /// (e.g. a local/unreleased build). Never silently treated as
    /// "up to date" or as something requiring a downgrade.
    DevelopmentBuild,
    /// The caller could not complete the worker operation. Carries no
    /// readiness implication; see [`EmuWizUpdateCheck::detail`].
    CheckFailed,
    /// Transport could not deliver usable metadata: connection, TLS, read
    /// or transfer-policy failure. This does not prove global offline state.
    NetworkFailure,
    /// The official endpoint was unavailable or requested a redirect.
    Unavailable,
    /// Missing, inconsistent, oversized or untrusted release metadata.
    MalformedResponse,
}

/// The full result of one self-update check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmuWizUpdateCheck {
    /// The running EmuWiz version, verbatim - see
    /// [`current_emuwiz_version`] for exactly where this comes from.
    pub current_version: String,
    /// The latest published release's tag, when the lookup succeeded.
    pub latest_version: Option<String>,
    /// A link to the release, when known - never used to fetch or execute
    /// anything; provided only for a human to open.
    pub release_url: Option<String>,
    /// Plain text only; never executed or rendered as remote HTML.
    pub release_notes: Option<String>,
    pub status: EmuWizUpdateStatus,
    /// A short reason for an unknown or failed result, absent for a
    /// successful comparison.
    pub detail: Option<String>,
    /// Unix seconds when this check ran, for a caller that wants to show
    /// "checked N minutes ago". Always the local clock at call time - this
    /// module keeps no cache and does not persist this value itself.
    pub checked_at_unix_seconds: i64,
}

/// EmuWiz's own authoritative current version: `env!("CARGO_PKG_VERSION")`,
/// resolved from the workspace `version.workspace = true` this crate
/// inherits (`Cargo.toml`'s `[workspace.package] version`) - the exact same
/// constant the CLI's `--version`, the GUI's About page, and the GUI's
/// title-bar version line already use. There is deliberately no second
/// version constant anywhere in this module.
pub fn current_emuwiz_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The minimal shape of a GitHub release this module needs - never the
/// full `emulator_download::EmulatorRelease` shape (assets, digests, ...),
/// none of which apply to a version-only check.
#[derive(Debug, Deserialize)]
struct GithubReleaseSummary {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

// A second bound at the caller protects against an injected transport ignoring
// maximum_bytes. Production transport independently enforces size and timeouts.
#[derive(Default)]
struct MetadataBuffer {
    bytes: Vec<u8>,
    exceeded: bool,
}
impl Write for MetadataBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > MAX_RELEASE_METADATA_BYTES as usize {
            self.exceeded = true;
            return Err(std::io::Error::other("release metadata exceeds 2 MiB"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn fetch_latest_emuwiz_release(
    transport: &dyn EmulatorDownloadTransport,
) -> Result<GithubReleaseSummary, DownloadError> {
    let mut buffer = MetadataBuffer::default();
    let response = transport.get(
        EMUWIZ_RELEASE_API_URL,
        MAX_RELEASE_METADATA_BYTES,
        &mut buffer,
        EmulatorTransferContext { cancellation: None },
    );
    if buffer.exceeded {
        return Err(DownloadError::InvalidRelease(
            "release metadata exceeds 2 MiB".into(),
        ));
    }
    let response = response?;
    // Even another GitHub API path could name a different repository. No
    // redirect is followed: only the fixed, established official URL is read.
    if (300..400).contains(&response.status) {
        return Err(DownloadError::RedirectRejected("release metadata redirect is not allowlisted; only the fixed official endpoint is trusted".into()));
    }
    if response.status != 200 {
        return Err(DownloadError::HttpStatus(response.status));
    }
    if buffer.bytes.is_empty()
        || response.bytes_received != buffer.bytes.len() as u64
        || response
            .content_length
            .is_some_and(|n| n != buffer.bytes.len() as u64)
    {
        return Err(DownloadError::InvalidRelease(
            "release metadata is empty or its length is inconsistent".into(),
        ));
    }
    let release: GithubReleaseSummary = serde_json::from_slice(&buffer.bytes)
        .map_err(|e| DownloadError::InvalidRelease(format!("release metadata is invalid: {e}")))?;
    let version = parse_semver(&release.tag_name).ok_or_else(|| {
        DownloadError::InvalidRelease("release tag is not a supported semantic version".into())
    })?;
    if release.tag_name.len() > 128
        || release.tag_name.trim() != release.tag_name
        || release.draft
        || release.prerelease
        || !version.pre.is_empty()
    {
        return Err(DownloadError::InvalidRelease(
            "latest stable release metadata is inconsistent, draft or prerelease".into(),
        ));
    }
    if let Some(link) = &release.html_url {
        if !validated_emuwiz_release_url(link, &release.tag_name) {
            return Err(DownloadError::InvalidRelease(
                "release link does not identify this official release".into(),
            ));
        }
    }
    Ok(release)
}

/// A release-page link must bind to this repository AND this exact tag. This
/// is also rechecked by the GUI before emitting a native hyperlink action.
pub fn validated_emuwiz_release_url(value: &str, tag: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Ok(mut expected) = url::Url::parse(EMUWIZ_OFFICIAL_REPOSITORY) else {
        return false;
    };
    expected
        .path_segments_mut()
        .expect("HTTPS base")
        .extend(["releases", "tag", tag]);
    url == expected && value == url.as_str()
}

fn normalize_version_tag(value: &str) -> &str {
    value.trim().strip_prefix('v').unwrap_or(value.trim())
}

/// Parses a version string as strict semver only after stripping one
/// leading `v` (the common release-tag convention, e.g. `v0.8.2`). Never a
/// lexicographic fallback: an unparseable string is `None`, which callers
/// turn into an honest `*VersionUnknown` status rather than a guess.
fn parse_semver(value: &str) -> Option<semver::Version> {
    semver::Version::parse(normalize_version_tag(value)).ok()
}

/// Pure comparison, no I/O: deterministic semver precedence, never a
/// lexicographic string comparison. `0.8.1-alpha` sorts before `0.8.2`
/// (different numeric core); `0.8.2-beta` sorts before plain `0.8.2` (same
/// numeric core, semver defines a prerelease as lower precedence than its
/// own release) - see the module tests for the exact table this
/// implements.
pub fn compare_emuwiz_versions(current: &str, latest: &str) -> EmuWizUpdateStatus {
    let Some(current_version) = parse_semver(current) else {
        return EmuWizUpdateStatus::CurrentVersionUnknown;
    };
    let Some(latest_version) = parse_semver(latest) else {
        return EmuWizUpdateStatus::LatestVersionUnknown;
    };
    match current_version.cmp_precedence(&latest_version) {
        std::cmp::Ordering::Less => EmuWizUpdateStatus::UpdateAvailable,
        std::cmp::Ordering::Equal => EmuWizUpdateStatus::UpToDate,
        std::cmp::Ordering::Greater => EmuWizUpdateStatus::DevelopmentBuild,
    }
}

fn unix_seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// The one public entry point: check whether a newer official EmuWiz
/// release is published, against the running version. Explicit-call only -
/// see the module doc comment for exactly what this does and does not do.
///
/// `transport` is injected so tests never touch the network (see this
/// module's `tests` for a fixture transport) and so the GUI can supply the
/// same transport it already constructs for emulator downloads. A failed
/// lookup produces a specific failure status with a `detail` -
/// never a panic, never a change to any readiness/health signal, and never
/// a loss of the (still-returned) `current_version`.
pub fn check_emuwiz_update(transport: &dyn EmulatorDownloadTransport) -> EmuWizUpdateCheck {
    let current_version = current_emuwiz_version().to_string();
    let checked_at_unix_seconds = unix_seconds_now();
    match fetch_latest_emuwiz_release(transport) {
        Ok(release) => {
            let status = compare_emuwiz_versions(&current_version, &release.tag_name);
            let detail = match status {
                EmuWizUpdateStatus::CurrentVersionUnknown => Some(format!(
                    "the running version {current_version:?} could not be parsed as semver"
                )),
                EmuWizUpdateStatus::LatestVersionUnknown => Some(format!(
                    "the latest release tag {:?} could not be parsed as semver",
                    release.tag_name
                )),
                _ => None,
            };
            EmuWizUpdateCheck {
                current_version,
                latest_version: Some(release.tag_name),
                release_url: release.html_url,
                release_notes: release.body,
                status,
                detail,
                checked_at_unix_seconds,
            }
        }
        Err(error) => EmuWizUpdateCheck {
            current_version,
            latest_version: None,
            release_url: None,
            release_notes: None,
            status: match &error {
                DownloadError::Io(_) => EmuWizUpdateStatus::NetworkFailure,
                DownloadError::InvalidRelease(_)
                | DownloadError::TooLarge
                | DownloadError::ContentLengthInvalid
                | DownloadError::TruncatedTransfer { .. } => EmuWizUpdateStatus::MalformedResponse,
                _ => EmuWizUpdateStatus::Unavailable,
            },
            detail: Some(match &error {
                DownloadError::TooLarge => "Release metadata exceeds the 2 MiB limit".into(),
                DownloadError::HttpStatus(code) => {
                    format!("Official release endpoint returned HTTP {code}")
                }
                _ => error.to_string(),
            }),
            checked_at_unix_seconds,
        },
    }
}

#[cfg(test)]
mod tests;
