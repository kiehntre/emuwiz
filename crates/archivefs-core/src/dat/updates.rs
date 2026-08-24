//! Typed, offline-only ownership and provenance for managed DAT snapshots.
//!
//! The user-editable DAT registry deliberately remains a registry of local
//! paths.  This module is the narrow, separate authority a later downloader
//! will need before it can replace any bytes: one built-in MAME software-list
//! descriptor, one state file, and at most current plus previous snapshots.
//!
//! There is intentionally no transport, URL fetching, scheduler, or generic
//! provider registration here.  A repository-relative path is delivery
//! metadata only; it is never used as a filesystem path.

use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dat::limits::DEFAULT_MAX_FILE_SIZE;
use crate::dat::model::DatEcosystem;
use crate::dat::sources::DatSourceOwnership;
use crate::{ArchiveFsError, Result};

/// The app-owned directory under EmuWiz's effective data directory.
pub const MANAGED_DAT_DIRECTORY: &str = "managed-dats";
const OBJECTS_DIRECTORY: &str = "objects";
const STATE_FILE_NAME: &str = "state.json";
const MAME_REPOSITORY: &str = "mamedev/mame";
const STAGING_DIRECTORY: &str = "staging";
const GITHUB_API_HOST: &str = "api.github.com";
const GITHUB_RAW_HOST: &str = "raw.githubusercontent.com";
const MANAGED_DAT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MANAGED_DAT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MANAGED_DAT_OVERALL_TIMEOUT: Duration = Duration::from_secs(90);
const MANAGED_DAT_HEADER_LIMIT: usize = 32 * 1024;
const MANAGED_DAT_NETWORK_CHUNK: usize = 64 * 1024;

/// The sole built-in managed-DAT provider initially supported by this model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDatProvider {
    MameSoftwareList,
}

impl ManagedDatProvider {
    fn storage_component(self) -> &'static str {
        match self {
            Self::MameSoftwareList => "mame-software-list",
        }
    }
}

/// A stable provider-scoped identity, not a filename-derived local-source ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManagedDatSourceId {
    pub provider: ManagedDatProvider,
    pub source_key: String,
}

impl ManagedDatSourceId {
    /// Creates the stable ID for one authoritative MAME software-list name.
    pub fn mame_software_list(source_key: impl Into<String>) -> Result<Self> {
        let source_key = source_key.into();
        validate_mame_software_list_name(&source_key)?;
        Ok(Self {
            provider: ManagedDatProvider::MameSoftwareList,
            source_key,
        })
    }

    fn validate(&self) -> Result<()> {
        match self.provider {
            ManagedDatProvider::MameSoftwareList => {
                validate_mame_software_list_name(&self.source_key)
            }
        }
    }

    /// A stable, app-owned relative storage path.  It never accepts a local
    /// filesystem path from configuration or a provider response.
    pub fn storage_relative_path(&self) -> PathBuf {
        PathBuf::from(self.provider.storage_component()).join(&self.source_key)
    }
}

impl fmt::Display for ManagedDatSourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}",
            self.provider.storage_component(),
            self.source_key
        )
    }
}

/// Managed update policy deliberately has no automatic mode yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDatUpdatePolicy {
    #[default]
    Disabled,
    Manual,
}

/// A built-in, validated future source contract.
///
/// Construction is intentionally limited to MAME software lists.  This keeps
/// an arbitrary URL or a provider-looking local DAT from becoming updater
/// authority by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatSourceDescriptor {
    source_id: ManagedDatSourceId,
    repository: &'static str,
    repository_relative_path: PathBuf,
    expected_ecosystem: DatEcosystem,
    expected_softwarelist_name: String,
    max_payload_size: u64,
    update_policy: ManagedDatUpdatePolicy,
}

impl ManagedDatSourceDescriptor {
    /// Constructs the fixed official MAME contract for one software list.
    ///
    /// It represents `mamedev/mame`, `hash/<name>.xml`, and a matching
    /// `<softwarelist name="<name>">`; it does not perform I/O or networking.
    pub fn mame_software_list(source_key: impl Into<String>) -> Result<Self> {
        let source_id = ManagedDatSourceId::mame_software_list(source_key)?;
        let source_key = source_id.source_key.clone();
        let descriptor = Self {
            source_id,
            repository: MAME_REPOSITORY,
            repository_relative_path: PathBuf::from("hash").join(format!("{source_key}.xml")),
            expected_ecosystem: DatEcosystem::MAMESoftwareList,
            expected_softwarelist_name: source_key,
            max_payload_size: DEFAULT_MAX_FILE_SIZE,
            update_policy: ManagedDatUpdatePolicy::Disabled,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn source_id(&self) -> &ManagedDatSourceId {
        &self.source_id
    }

    pub fn repository(&self) -> &'static str {
        self.repository
    }

    pub fn repository_relative_path(&self) -> &Path {
        &self.repository_relative_path
    }

    pub fn expected_ecosystem(&self) -> DatEcosystem {
        self.expected_ecosystem
    }

    pub fn expected_softwarelist_name(&self) -> &str {
        &self.expected_softwarelist_name
    }

    pub fn max_payload_size(&self) -> u64 {
        self.max_payload_size
    }

    pub fn update_policy(&self) -> ManagedDatUpdatePolicy {
        self.update_policy
    }

    /// Enables only the explicit, non-scheduled manual policy.
    pub fn with_update_policy(mut self, policy: ManagedDatUpdatePolicy) -> Self {
        self.update_policy = policy;
        self
    }

    /// Validates that this still represents the one built-in contract.
    pub fn validate(&self) -> Result<()> {
        self.source_id.validate()?;
        if self.source_id.provider != ManagedDatProvider::MameSoftwareList
            || self.repository != MAME_REPOSITORY
            || self.expected_ecosystem != DatEcosystem::MAMESoftwareList
            || self.expected_softwarelist_name != self.source_id.source_key
        {
            return Err(config_error(
                "managed DAT descriptor is not the fixed MAME software-list contract",
            ));
        }
        validate_repository_relative_path(&self.repository_relative_path)?;
        let expected_path =
            PathBuf::from("hash").join(format!("{}.xml", self.source_id.source_key));
        if self.repository_relative_path != expected_path {
            return Err(config_error(
                "managed MAME software-list path does not match its typed source ID",
            ));
        }
        if self.max_payload_size == 0 || self.max_payload_size > DEFAULT_MAX_FILE_SIZE {
            return Err(config_error(
                "managed DAT payload limit must be between one byte and the DAT parser limit",
            ));
        }
        Ok(())
    }
}

/// Rejects repository paths that could be interpreted as local filesystem
/// authority.  They are retained as remote metadata only after this check.
pub fn validate_repository_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(config_error(
            "repository-relative path must be non-empty and relative",
        ));
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(config_error(
                "repository-relative path must contain normal components only",
            ));
        }
    }
    Ok(())
}

/// One immutable managed object name.  It is a digest, never an upstream
/// filename, and is placed only below a source's `objects` directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedDatSnapshot {
    pub sha256: String,
}

impl ManagedDatSnapshot {
    pub fn new(sha256: impl Into<String>) -> Result<Self> {
        let snapshot = Self {
            sha256: sha256.into(),
        };
        validate_sha256(&snapshot.sha256)?;
        Ok(snapshot)
    }

    fn validate(&self) -> Result<()> {
        validate_sha256(&self.sha256)
    }
}

/// Durable provenance for exactly one current and one optional previous
/// validated snapshot.  This record is only state; it has no network behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedDatState {
    pub source_id: ManagedDatSourceId,
    pub current_snapshot: ManagedDatSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_snapshot: Option<ManagedDatSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// Repeated deliberately for simple provenance inspection without
    /// dereferencing `current_snapshot`.
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at_unix_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at_unix_seconds: Option<u64>,
    pub parsed_ecosystem: DatEcosystem,
    pub authoritative_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
}

impl ManagedDatState {
    /// Creates state for a validated current snapshot.  A future downloader
    /// fills provenance fields only after validating bytes and parser output.
    pub fn new(
        descriptor: &ManagedDatSourceDescriptor,
        current_snapshot: ManagedDatSnapshot,
    ) -> Result<Self> {
        descriptor.validate()?;
        let state = Self {
            source_id: descriptor.source_id.clone(),
            sha256: current_snapshot.sha256.clone(),
            current_snapshot,
            previous_snapshot: None,
            upstream_revision: None,
            etag: None,
            last_modified: None,
            retrieved_at_unix_seconds: None,
            last_checked_at_unix_seconds: None,
            parsed_ecosystem: descriptor.expected_ecosystem,
            authoritative_name: descriptor.expected_softwarelist_name.clone(),
            validation_summary: None,
            last_failure: None,
        };
        state.validate_for(descriptor)?;
        Ok(state)
    }

    /// Ensures state belongs to this descriptor and can only name current plus
    /// one previous snapshot.
    pub fn validate_for(&self, descriptor: &ManagedDatSourceDescriptor) -> Result<()> {
        descriptor.validate()?;
        self.source_id.validate()?;
        self.current_snapshot.validate()?;
        if let Some(previous) = &self.previous_snapshot {
            previous.validate()?;
            if previous.sha256 == self.current_snapshot.sha256 {
                return Err(config_error(
                    "managed DAT previous snapshot must differ from current snapshot",
                ));
            }
        }
        validate_sha256(&self.sha256)?;
        if self.sha256 != self.current_snapshot.sha256 {
            return Err(config_error(
                "managed DAT SHA-256 must name the current snapshot",
            ));
        }
        if self.source_id != descriptor.source_id
            || self.parsed_ecosystem != descriptor.expected_ecosystem
            || self.authoritative_name != descriptor.expected_softwarelist_name
        {
            return Err(config_error(
                "managed DAT state does not match its typed descriptor",
            ));
        }
        validate_optional_metadata("upstream revision", &self.upstream_revision)?;
        validate_optional_metadata("ETag", &self.etag)?;
        validate_optional_metadata("Last-Modified", &self.last_modified)?;
        validate_optional_metadata("validation summary", &self.validation_summary)?;
        validate_optional_metadata("last failure", &self.last_failure)?;
        Ok(())
    }
}

/// A validated read-only snapshot path suitable for the existing DAT parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatReadOnlySource {
    source_id: ManagedDatSourceId,
    path: PathBuf,
}

impl ManagedDatReadOnlySource {
    pub fn ownership(&self) -> DatSourceOwnership {
        DatSourceOwnership::EmuWizManaged
    }

    pub fn source_id(&self) -> &ManagedDatSourceId {
        &self.source_id
    }

    /// The ordinary regular-file input path for `parse_dat_file` and other
    /// read-only DAT consumers.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Returns the managed DAT root under EmuWiz's effective data directory.
/// This merely resolves a path; it does not create it.
pub fn managed_dat_root() -> Result<PathBuf> {
    crate::app_dirs::data_path(MANAGED_DAT_DIRECTORY)
}

/// The durable state-file location for a managed source.  The returned path is
/// always lexically below `managed_root`; it is never caller-supplied.
pub fn managed_dat_state_path(
    managed_root: &Path,
    source_id: &ManagedDatSourceId,
) -> Result<PathBuf> {
    let source_dir = managed_source_dir(managed_root, source_id)?;
    Ok(source_dir.join(STATE_FILE_NAME))
}

/// Loads one managed state record after checking its path for symlink escape.
pub fn load_managed_dat_state(
    managed_root: &Path,
    descriptor: &ManagedDatSourceDescriptor,
) -> Result<ManagedDatState> {
    descriptor.validate()?;
    let path = managed_dat_state_path(managed_root, descriptor.source_id())?;
    ensure_existing_path_is_not_symlinked(managed_root, &path)?;
    let text =
        fs::read_to_string(&path).map_err(|source| ArchiveFsError::io(path.clone(), source))?;
    let state: ManagedDatState = serde_json::from_str(&text).map_err(|error| {
        config_error(format!(
            "failed to parse managed DAT state {}: {error}",
            path.display()
        ))
    })?;
    state.validate_for(descriptor)?;
    Ok(state)
}

/// Atomically saves a state record below the managed root.  It never accepts a
/// state-file destination outside the typed source's storage directory.
pub fn save_managed_dat_state(managed_root: &Path, state: &ManagedDatState) -> Result<()> {
    let descriptor =
        ManagedDatSourceDescriptor::mame_software_list(state.source_id.source_key.clone())?;
    state.validate_for(&descriptor)?;
    create_managed_source_dir(managed_root, &state.source_id)?;
    let path = managed_dat_state_path(managed_root, &state.source_id)?;
    ensure_existing_path_is_not_symlinked(managed_root, &path)?;
    let body = serde_json::to_string_pretty(state)
        .map_err(|error| config_error(format!("failed to serialize managed DAT state: {error}")))?;
    crate::atomic_write_text(&path, &format!("{body}\n"))
}

/// Validates ownership of a current or previous snapshot and returns its
/// ordinary regular-file path.  There is no API that accepts an arbitrary
/// external file and labels it managed.
pub fn validate_managed_snapshot_ownership(
    managed_root: &Path,
    state: &ManagedDatState,
    snapshot: &ManagedDatSnapshot,
) -> Result<PathBuf> {
    let descriptor =
        ManagedDatSourceDescriptor::mame_software_list(state.source_id.source_key.clone())?;
    state.validate_for(&descriptor)?;
    let known = snapshot == &state.current_snapshot
        || state
            .previous_snapshot
            .as_ref()
            .is_some_and(|previous| previous == snapshot);
    if !known {
        return Err(config_error(
            "managed DAT snapshot is not current or previous state",
        ));
    }
    managed_snapshot_path(managed_root, &state.source_id, snapshot)
}

/// Resolves the current managed snapshot as a normal read-only DAT parser
/// input, after all ownership checks have passed.
pub fn resolve_current_managed_dat_source(
    managed_root: &Path,
    state: &ManagedDatState,
) -> Result<ManagedDatReadOnlySource> {
    resolve_managed_dat_snapshot_source(managed_root, state, &state.current_snapshot)
}

/// Resolves either the current or retained previous snapshot as a normal
/// read-only DAT input.  Callers cannot supply an arbitrary filesystem path:
/// the snapshot must be named by the validated managed state.
pub fn resolve_managed_dat_snapshot_source(
    managed_root: &Path,
    state: &ManagedDatState,
    snapshot: &ManagedDatSnapshot,
) -> Result<ManagedDatReadOnlySource> {
    let path = validate_managed_snapshot_ownership(managed_root, state, snapshot)?;
    Ok(ManagedDatReadOnlySource {
        source_id: state.source_id.clone(),
        path,
    })
}

fn managed_snapshot_path(
    managed_root: &Path,
    source_id: &ManagedDatSourceId,
    snapshot: &ManagedDatSnapshot,
) -> Result<PathBuf> {
    snapshot.validate()?;
    let source_dir = managed_source_dir(managed_root, source_id)?;
    let path = source_dir.join(OBJECTS_DIRECTORY).join(&snapshot.sha256);
    ensure_existing_path_is_not_symlinked(managed_root, &path)?;
    let metadata =
        fs::symlink_metadata(&path).map_err(|source| ArchiveFsError::io(path.clone(), source))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(config_error(format!(
            "managed DAT snapshot is not a regular non-symlink file: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn managed_source_dir(managed_root: &Path, source_id: &ManagedDatSourceId) -> Result<PathBuf> {
    validate_managed_root_path(managed_root)?;
    source_id.validate()?;
    let path = managed_root.join(source_id.storage_relative_path());
    if !path.starts_with(managed_root) {
        return Err(config_error("managed DAT source path escaped managed root"));
    }
    Ok(path)
}

fn create_managed_source_dir(managed_root: &Path, source_id: &ManagedDatSourceId) -> Result<()> {
    let source_dir = managed_source_dir(managed_root, source_id)?;
    fs::create_dir_all(&source_dir)
        .map_err(|source| ArchiveFsError::io(source_dir.clone(), source))?;
    ensure_existing_path_is_not_symlinked(managed_root, &source_dir)?;
    let metadata = fs::symlink_metadata(&source_dir)
        .map_err(|source| ArchiveFsError::io(source_dir.clone(), source))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(config_error(format!(
            "managed DAT source directory is not a real directory: {}",
            source_dir.display()
        )));
    }
    Ok(())
}

fn validate_managed_root_path(root: &Path) -> Result<()> {
    if !root.is_absolute() {
        return Err(config_error("managed DAT root must be absolute"));
    }
    let mut current = PathBuf::new();
    for component in root.components() {
        match component {
            Component::CurDir | Component::ParentDir => {
                return Err(config_error(
                    "managed DAT root must not contain traversal components",
                ));
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
                // Check every existing ancestor, rather than only `root`.
                // `symlink_metadata(root)` alone would follow a symlinked
                // parent and could falsely make an outside directory look
                // app-owned.
                check_non_symlink_if_present(&current)?;
            }
        }
    }
    Ok(())
}

/// Rejects a symlink at any existing component between the managed root and an
/// expected descendant.  This avoids using canonicalisation as ownership
/// evidence: a symlink is never accepted as an owned directory or object.
fn ensure_existing_path_is_not_symlinked(root: &Path, path: &Path) -> Result<()> {
    validate_managed_root_path(root)?;
    if !path.starts_with(root) {
        return Err(config_error("managed DAT path is outside managed root"));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| config_error("managed DAT path escaped root"))?;
    let mut current = root.to_path_buf();
    check_non_symlink_if_present(&current)?;
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(config_error(
                "managed DAT path contains non-normal descendant component",
            ));
        };
        current.push(part);
        check_non_symlink_if_present(&current)?;
    }
    Ok(())
}

fn check_non_symlink_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(config_error(format!(
            "managed DAT path must not use symlinks: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ArchiveFsError::io(path.to_path_buf(), error)),
    }
}

fn validate_mame_software_list_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(config_error(
            "MAME software-list name must be 1-64 lowercase ASCII letters, digits, or underscores",
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(config_error(
            "managed DAT SHA-256 must be exactly 64 hexadecimal characters",
        ));
    }
    Ok(())
}

fn validate_optional_metadata(label: &str, value: &Option<String>) -> Result<()> {
    if value
        .as_ref()
        .is_some_and(|value| value.len() > 4096 || value.contains('\0'))
    {
        return Err(config_error(format!(
            "managed DAT {label} is too long or contains NUL"
        )));
    }
    Ok(())
}

fn config_error(detail: impl Into<String>) -> ArchiveFsError {
    ArchiveFsError::Config(detail.into())
}

/// The HTTP request shape used by the managed-DAT updater.  URLs are created
/// internally from a validated built-in descriptor; callers never supply one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatHttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// Response metadata retained for explicit update decisions and provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatHttpResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub retry_after_seconds: Option<u64>,
    pub downloaded_bytes: u64,
}

/// Transport failures are classified rather than made actionable by matching
/// prose.  A GUI may present `detail`, but logic should use `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedDatTransportFailureKind {
    Offline,
    Timeout,
    Network,
    Tls,
    InvalidResponse,
    Destination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDatTransportError {
    pub kind: ManagedDatTransportFailureKind,
    pub detail: String,
}

impl ManagedDatTransportError {
    fn new(kind: ManagedDatTransportFailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

/// Narrow synchronous seam for tests and the future GUI worker.  It performs
/// one serial request and streams any successful response into `destination`.
pub trait ManagedDatTransport {
    fn get(
        &self,
        request: &ManagedDatHttpRequest,
        maximum_bytes: u64,
        destination: &mut dyn Write,
    ) -> std::result::Result<ManagedDatHttpResponse, ManagedDatTransportError>;
}

/// Rustls-backed HTTPS transport.  It has no redirect policy because both
/// approved hosts and immutable URL shapes are fixed by this module.
#[derive(Debug, Clone)]
pub struct HttpsManagedDatTransport {
    agent: ureq::Agent,
}

impl HttpsManagedDatTransport {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(MANAGED_DAT_OVERALL_TIMEOUT))
            .timeout_resolve(Some(MANAGED_DAT_CONNECT_TIMEOUT))
            .timeout_connect(Some(MANAGED_DAT_CONNECT_TIMEOUT))
            .timeout_recv_response(None)
            .timeout_recv_body(Some(MANAGED_DAT_IDLE_TIMEOUT))
            .build();
        Self {
            agent: config.new_agent(),
        }
    }
}

impl Default for HttpsManagedDatTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ManagedDatTransport for HttpsManagedDatTransport {
    fn get(
        &self,
        request: &ManagedDatHttpRequest,
        maximum_bytes: u64,
        destination: &mut dyn Write,
    ) -> std::result::Result<ManagedDatHttpResponse, ManagedDatTransportError> {
        validate_managed_dat_http_url(&request.url)?;
        let mut call = self
            .agent
            .get(&request.url)
            .config()
            .timeout_global(Some(MANAGED_DAT_OVERALL_TIMEOUT))
            .build()
            .header("Accept-Encoding", "identity")
            .header("User-Agent", concat!("EmuWiz/", env!("CARGO_PKG_VERSION")));
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        let mut response = call.call().map_err(classify_managed_dat_ureq_error)?;
        let header_bytes = response
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum::<usize>();
        if header_bytes > MANAGED_DAT_HEADER_LIMIT {
            return Err(ManagedDatTransportError::new(
                ManagedDatTransportFailureKind::InvalidResponse,
                "response headers exceed the managed DAT limit",
            ));
        }
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let content_length = match header("content-length") {
            Some(value) => Some(value.parse::<u64>().map_err(|_| {
                ManagedDatTransportError::new(
                    ManagedDatTransportFailureKind::InvalidResponse,
                    "response Content-Length is invalid",
                )
            })?),
            None => None,
        };
        let etag = header("etag");
        let last_modified = header("last-modified");
        let retry_after_seconds = header("retry-after").and_then(|value| value.parse::<u64>().ok());
        if content_length.is_some_and(|size| size > maximum_bytes) {
            return Err(ManagedDatTransportError::new(
                ManagedDatTransportFailureKind::InvalidResponse,
                "response Content-Length exceeds the managed DAT limit",
            ));
        }
        let status = response.status().as_u16();
        let mut downloaded_bytes = 0u64;
        if (200..300).contains(&status) {
            let mut reader = response.body_mut().as_reader();
            let mut buffer = [0u8; MANAGED_DAT_NETWORK_CHUNK];
            loop {
                let count = reader.read(&mut buffer).map_err(|error| {
                    ManagedDatTransportError::new(
                        if error.kind() == std::io::ErrorKind::TimedOut {
                            ManagedDatTransportFailureKind::Timeout
                        } else {
                            ManagedDatTransportFailureKind::Network
                        },
                        error.to_string(),
                    )
                })?;
                if count == 0 {
                    break;
                }
                downloaded_bytes = downloaded_bytes.saturating_add(count as u64);
                if downloaded_bytes > maximum_bytes {
                    return Err(ManagedDatTransportError::new(
                        ManagedDatTransportFailureKind::InvalidResponse,
                        "received body exceeds the managed DAT limit",
                    ));
                }
                destination.write_all(&buffer[..count]).map_err(|error| {
                    ManagedDatTransportError::new(
                        ManagedDatTransportFailureKind::Destination,
                        error.to_string(),
                    )
                })?;
            }
        }
        Ok(ManagedDatHttpResponse {
            status,
            content_length,
            etag,
            last_modified,
            retry_after_seconds,
            downloaded_bytes,
        })
    }
}

/// Structured result of a manual check or update request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedDatUpdateOutcome {
    Disabled,
    UpToDate {
        upstream_revision: Option<String>,
    },
    UpdateAvailable {
        upstream_revision: String,
    },
    Updated {
        upstream_revision: String,
        sha256: String,
    },
    Offline,
    RateLimited {
        retry_after_seconds: Option<u64>,
    },
    Failed {
        kind: ManagedDatUpdateFailureKind,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedDatUpdateFailureKind {
    Network,
    Timeout,
    Tls,
    Forbidden,
    NotFound,
    HttpStatus,
    InvalidResponse,
    DownloadTooLarge,
    EmptyDownload,
    TruncatedDownload,
    Parser,
    WrongEcosystem,
    WrongAuthoritativeName,
    EmptyCatalogue,
    Storage,
}

/// Options shared by the explicit check and update APIs.  `offline` is an
/// affirmative no-network mode, never inferred from a failure.
#[derive(Debug, Clone)]
pub struct ManagedDatUpdateOptions {
    pub managed_root: PathBuf,
    pub offline: bool,
    pub now_unix_seconds: u64,
}

impl ManagedDatUpdateOptions {
    pub fn new(managed_root: PathBuf, now_unix_seconds: u64) -> Self {
        Self {
            managed_root,
            offline: false,
            now_unix_seconds,
        }
    }
}

/// Checks the current immutable MAME revision.  It never downloads XML or
/// changes a current/previous snapshot; it only persists check metadata for an
/// already installed state.
pub fn check_managed_dat_update(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    descriptor.validate()?;
    if descriptor.update_policy() == ManagedDatUpdatePolicy::Disabled {
        return Ok(ManagedDatUpdateOutcome::Disabled);
    }
    if options.offline {
        return Ok(ManagedDatUpdateOutcome::Offline);
    }
    let existing = load_optional_managed_dat_state(&options.managed_root, descriptor)?;
    let revision = match resolve_mame_revision(descriptor, existing.as_ref(), transport) {
        Ok(revision) => revision,
        Err(outcome) => return Ok(outcome),
    };
    if revision.not_modified
        || existing.as_ref().is_some_and(|state| {
            state.upstream_revision.as_deref() == Some(revision.commit.as_str())
        })
    {
        if let Some(mut state) = existing {
            state.last_checked_at_unix_seconds = Some(options.now_unix_seconds);
            state.etag = revision.etag.or(state.etag);
            state.last_modified = revision.last_modified.or(state.last_modified);
            if let Err(error) = save_managed_dat_state(&options.managed_root, &state) {
                return Ok(storage_failure(error));
            }
            return Ok(ManagedDatUpdateOutcome::UpToDate {
                upstream_revision: state.upstream_revision,
            });
        }
        return Ok(ManagedDatUpdateOutcome::UpToDate {
            upstream_revision: None,
        });
    }
    Ok(ManagedDatUpdateOutcome::UpdateAvailable {
        upstream_revision: revision.commit,
    })
}

/// Downloads and validates the XML at a revision resolved during this call.
/// No bytes become current until they are parsed, content-addressed, and the
/// next state record has been atomically written.
pub fn update_managed_dat(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    transport: &dyn ManagedDatTransport,
) -> Result<ManagedDatUpdateOutcome> {
    descriptor.validate()?;
    if descriptor.update_policy() == ManagedDatUpdatePolicy::Disabled {
        return Ok(ManagedDatUpdateOutcome::Disabled);
    }
    if options.offline {
        return Ok(ManagedDatUpdateOutcome::Offline);
    }
    let existing = load_optional_managed_dat_state(&options.managed_root, descriptor)?;
    let revision = match resolve_mame_revision(descriptor, existing.as_ref(), transport) {
        Ok(revision) => revision,
        Err(outcome) => return Ok(outcome),
    };
    if revision.not_modified {
        return mark_up_to_date(existing, options, revision);
    }
    if existing
        .as_ref()
        .is_some_and(|state| state.upstream_revision.as_deref() == Some(revision.commit.as_str()))
    {
        return mark_up_to_date(existing, options, revision);
    }

    let source_dir = match create_managed_source_dir(&options.managed_root, descriptor.source_id())
    {
        Ok(()) => managed_source_dir(&options.managed_root, descriptor.source_id())?,
        Err(error) => return Ok(storage_failure(error)),
    };
    let staging = match create_private_staging_file(&options.managed_root, &source_dir) {
        Ok(file) => file,
        Err(error) => return Ok(storage_failure(error)),
    };
    let _cleanup = ManagedDatStagingCleanup(staging.path.clone());
    let (response, sha256) =
        match download_mame_xml(descriptor, &revision.commit, &staging.path, transport) {
            Ok(download) => download,
            Err(outcome) => return Ok(outcome),
        };
    let parsed = match crate::dat::parsers::parse_dat_file(
        &staging.path,
        crate::dat::limits::DatLimits::default(),
    ) {
        Ok(outcome) => outcome.dat,
        Err(error) => {
            return Ok(ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::Parser,
                detail: error.to_string(),
            });
        }
    };
    if parsed.source.ecosystem != descriptor.expected_ecosystem() {
        return Ok(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::WrongEcosystem,
            detail: format!(
                "expected {}, received {}",
                descriptor.expected_ecosystem().label(),
                parsed.source.ecosystem.label()
            ),
        });
    }
    if parsed.source.name.as_deref() != Some(descriptor.expected_softwarelist_name()) {
        return Ok(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::WrongAuthoritativeName,
            detail: "parsed MAME software-list name differs from typed descriptor".to_string(),
        });
    }
    if parsed.games.is_empty() || parsed.source.entry_count == 0 {
        return Ok(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::EmptyCatalogue,
            detail: "MAME software-list contains no software records".to_string(),
        });
    }
    publish_validated_snapshot(
        descriptor,
        options,
        existing,
        revision,
        response,
        sha256,
        staging.path,
        parsed.source.entry_count as u64,
    )
}

#[derive(Debug)]
struct ResolvedMameRevision {
    commit: String,
    etag: Option<String>,
    last_modified: Option<String>,
    not_modified: bool,
}

fn resolve_mame_revision(
    descriptor: &ManagedDatSourceDescriptor,
    existing: Option<&ManagedDatState>,
    transport: &dyn ManagedDatTransport,
) -> std::result::Result<ResolvedMameRevision, ManagedDatUpdateOutcome> {
    let mut headers = vec![(
        "Accept".to_string(),
        "application/vnd.github+json".to_string(),
    )];
    if let Some(state) = existing {
        if let Some(etag) = &state.etag {
            headers.push(("If-None-Match".to_string(), etag.clone()));
        }
        if let Some(last_modified) = &state.last_modified {
            headers.push(("If-Modified-Since".to_string(), last_modified.clone()));
        }
    }
    let request = ManagedDatHttpRequest {
        url: format!(
            "https://{GITHUB_API_HOST}/repos/{}/commits/master",
            descriptor.repository()
        ),
        headers,
    };
    let mut bytes = Vec::new();
    let response = transport
        .get(&request, 64 * 1024, &mut bytes)
        .map_err(transport_failure)?;
    match response.status {
        304 => Ok(ResolvedMameRevision {
            commit: existing
                .and_then(|state| state.upstream_revision.clone())
                .unwrap_or_default(),
            etag: response.etag,
            last_modified: response.last_modified,
            not_modified: true,
        }),
        200 => {
            if response
                .content_length
                .is_some_and(|length| length != response.downloaded_bytes)
                || bytes.len() as u64 != response.downloaded_bytes
            {
                return Err(ManagedDatUpdateOutcome::Failed {
                    kind: ManagedDatUpdateFailureKind::TruncatedDownload,
                    detail: "revision response was truncated".to_string(),
                });
            }
            let commit = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|value| {
                    value
                        .get("sha")
                        .and_then(|sha| sha.as_str())
                        .map(str::to_owned)
                })
                .filter(|sha| is_git_commit_sha(sha))
                .ok_or_else(|| ManagedDatUpdateOutcome::Failed {
                    kind: ManagedDatUpdateFailureKind::InvalidResponse,
                    detail: "GitHub revision response did not contain a commit SHA".to_string(),
                })?;
            Ok(ResolvedMameRevision {
                commit,
                etag: response.etag,
                last_modified: response.last_modified,
                not_modified: false,
            })
        }
        status => Err(http_failure(status, response.retry_after_seconds)),
    }
}

fn download_mame_xml(
    descriptor: &ManagedDatSourceDescriptor,
    commit: &str,
    staging_path: &Path,
    transport: &dyn ManagedDatTransport,
) -> std::result::Result<(ManagedDatHttpResponse, String), ManagedDatUpdateOutcome> {
    if !is_git_commit_sha(commit) {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::InvalidResponse,
            detail: "resolved revision is not a commit SHA".to_string(),
        });
    }
    let request = ManagedDatHttpRequest {
        url: format!(
            "https://{GITHUB_RAW_HOST}/{}/{}/{}",
            descriptor.repository(),
            commit,
            descriptor.repository_relative_path().to_string_lossy()
        ),
        headers: Vec::new(),
    };
    let file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(staging_path)
        .map_err(|error| storage_failure(ArchiveFsError::io(staging_path.to_path_buf(), error)))?;
    let mut writer = HashingWriter::new(file);
    let response = transport
        .get(&request, descriptor.max_payload_size(), &mut writer)
        .map_err(transport_failure)?;
    writer
        .flush()
        .map_err(|error| storage_failure(ArchiveFsError::io(staging_path.to_path_buf(), error)))?;
    let sha256 = digest_hex(writer.hasher.finalize());
    let actual = fs::metadata(staging_path)
        .map_err(|error| storage_failure(ArchiveFsError::io(staging_path.to_path_buf(), error)))?
        .len();
    match response.status {
        200 => {}
        status => return Err(http_failure(status, response.retry_after_seconds)),
    }
    if actual == 0 || response.downloaded_bytes == 0 {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::EmptyDownload,
            detail: "MAME software-list response was empty".to_string(),
        });
    }
    if response
        .content_length
        .is_some_and(|length| length > descriptor.max_payload_size())
    {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::DownloadTooLarge,
            detail: "MAME software-list declared Content-Length exceeds the configured limit"
                .to_string(),
        });
    }
    if actual > descriptor.max_payload_size()
        || response.downloaded_bytes > descriptor.max_payload_size()
    {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::DownloadTooLarge,
            detail: "MAME software-list response exceeded the configured limit".to_string(),
        });
    }
    if response
        .content_length
        .is_some_and(|length| length != response.downloaded_bytes)
        || actual != response.downloaded_bytes
    {
        return Err(ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::TruncatedDownload,
            detail: "MAME software-list response length did not match bytes received".to_string(),
        });
    }
    Ok((response, sha256))
}

fn publish_validated_snapshot(
    descriptor: &ManagedDatSourceDescriptor,
    options: &ManagedDatUpdateOptions,
    existing: Option<ManagedDatState>,
    revision: ResolvedMameRevision,
    _response: ManagedDatHttpResponse,
    sha256: String,
    staging_path: PathBuf,
    entry_count: u64,
) -> Result<ManagedDatUpdateOutcome> {
    let snapshot = ManagedDatSnapshot::new(sha256.clone())?;
    let source_dir = managed_source_dir(&options.managed_root, descriptor.source_id())?;
    let objects = source_dir.join(OBJECTS_DIRECTORY);
    fs::create_dir_all(&objects).map_err(|error| ArchiveFsError::io(objects.clone(), error))?;
    ensure_existing_path_is_not_symlinked(&options.managed_root, &objects)?;
    let object_path = objects.join(&sha256);
    if object_path.exists() {
        let existing_digest = sha256_file(&object_path)?;
        if existing_digest != sha256 {
            return Ok(ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::Storage,
                detail: "existing managed DAT object does not match its content-addressed name"
                    .to_string(),
            });
        }
    } else {
        fs::rename(&staging_path, &object_path)
            .map_err(|error| ArchiveFsError::io(object_path.clone(), error))?;
    }

    let old_current = existing
        .as_ref()
        .map(|state| state.current_snapshot.clone());
    let old_previous = existing
        .as_ref()
        .and_then(|state| state.previous_snapshot.clone());
    let mut state = existing.unwrap_or_else(|| {
        ManagedDatState::new(descriptor, snapshot.clone()).expect("validated descriptor and digest")
    });
    if state.current_snapshot != snapshot {
        state.previous_snapshot = Some(state.current_snapshot.clone());
        state.current_snapshot = snapshot.clone();
        state.sha256 = sha256.clone();
    }
    state.upstream_revision = Some(revision.commit.clone());
    state.etag = revision.etag;
    state.last_modified = revision.last_modified;
    state.retrieved_at_unix_seconds = Some(options.now_unix_seconds);
    state.last_checked_at_unix_seconds = Some(options.now_unix_seconds);
    state.parsed_ecosystem = descriptor.expected_ecosystem();
    state.authoritative_name = descriptor.expected_softwarelist_name().to_string();
    state.validation_summary = Some(format!(
        "validated MAME software-list with {entry_count} records"
    ));
    state.last_failure = None;
    if let Err(error) = save_managed_dat_state(&options.managed_root, &state) {
        return Ok(storage_failure(error));
    }
    // Only after the state pointer is durable may an object older than the
    // retained previous snapshot be forgotten.  A failed cleanup leaves an
    // unreachable immutable object, never damages the active source.
    if let Some(old_previous) = old_previous {
        if old_current.as_ref() != Some(&snapshot) && old_previous != snapshot {
            let obsolete = objects.join(old_previous.sha256);
            let _ = fs::remove_file(obsolete);
        }
    }
    Ok(ManagedDatUpdateOutcome::Updated {
        upstream_revision: revision.commit,
        sha256,
    })
}

fn mark_up_to_date(
    existing: Option<ManagedDatState>,
    options: &ManagedDatUpdateOptions,
    revision: ResolvedMameRevision,
) -> Result<ManagedDatUpdateOutcome> {
    let Some(mut state) = existing else {
        return Ok(ManagedDatUpdateOutcome::UpToDate {
            upstream_revision: None,
        });
    };
    state.last_checked_at_unix_seconds = Some(options.now_unix_seconds);
    state.etag = revision.etag.or(state.etag);
    state.last_modified = revision.last_modified.or(state.last_modified);
    if let Err(error) = save_managed_dat_state(&options.managed_root, &state) {
        return Ok(storage_failure(error));
    }
    Ok(ManagedDatUpdateOutcome::UpToDate {
        upstream_revision: state.upstream_revision,
    })
}

fn load_optional_managed_dat_state(
    root: &Path,
    descriptor: &ManagedDatSourceDescriptor,
) -> Result<Option<ManagedDatState>> {
    match load_managed_dat_state(root, descriptor) {
        Ok(state) => Ok(Some(state)),
        Err(ArchiveFsError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

struct PrivateStagingFile {
    path: PathBuf,
}

struct ManagedDatStagingCleanup(PathBuf);

impl Drop for ManagedDatStagingCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn create_private_staging_file(root: &Path, source_dir: &Path) -> Result<PrivateStagingFile> {
    let staging = source_dir.join(STAGING_DIRECTORY);
    fs::create_dir_all(&staging).map_err(|error| ArchiveFsError::io(staging.clone(), error))?;
    ensure_existing_path_is_not_symlinked(root, &staging)?;
    for sequence in 0..128u32 {
        let path = staging.join(format!("{}-{sequence}.part", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(PrivateStagingFile { path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ArchiveFsError::io(path, error)),
        }
    }
    Err(config_error(
        "could not allocate a private managed DAT staging file",
    ))
}

struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; MANAGED_DAT_NETWORK_CHUNK];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(digest_hex(hasher.finalize()))
}

fn digest_hex(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn validate_managed_dat_http_url(value: &str) -> std::result::Result<(), ManagedDatTransportError> {
    let url = url::Url::parse(value).map_err(|error| {
        ManagedDatTransportError::new(
            ManagedDatTransportFailureKind::InvalidResponse,
            error.to_string(),
        )
    })?;
    if url.scheme() != "https" || !matches!(url.host_str(), Some(GITHUB_API_HOST | GITHUB_RAW_HOST))
    {
        return Err(ManagedDatTransportError::new(
            ManagedDatTransportFailureKind::InvalidResponse,
            "managed DAT transport rejected a non-approved HTTPS host",
        ));
    }
    Ok(())
}

fn classify_managed_dat_ureq_error(error: ureq::Error) -> ManagedDatTransportError {
    let detail = error.to_string();
    let lower = detail.to_ascii_lowercase();
    let kind = if lower.contains("timed out") || lower.contains("timeout") {
        ManagedDatTransportFailureKind::Timeout
    } else if lower.contains("tls") || lower.contains("certificate") {
        ManagedDatTransportFailureKind::Tls
    } else {
        ManagedDatTransportFailureKind::Network
    };
    ManagedDatTransportError::new(kind, detail)
}

fn transport_failure(error: ManagedDatTransportError) -> ManagedDatUpdateOutcome {
    ManagedDatUpdateOutcome::Failed {
        kind: match error.kind {
            ManagedDatTransportFailureKind::Timeout => ManagedDatUpdateFailureKind::Timeout,
            ManagedDatTransportFailureKind::Tls => ManagedDatUpdateFailureKind::Tls,
            ManagedDatTransportFailureKind::Offline
            | ManagedDatTransportFailureKind::Network
            | ManagedDatTransportFailureKind::Destination => ManagedDatUpdateFailureKind::Network,
            ManagedDatTransportFailureKind::InvalidResponse => {
                ManagedDatUpdateFailureKind::InvalidResponse
            }
        },
        detail: error.detail,
    }
}

fn http_failure(status: u16, retry_after_seconds: Option<u64>) -> ManagedDatUpdateOutcome {
    match status {
        403 => ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::Forbidden,
            detail: "managed DAT source returned HTTP 403".to_string(),
        },
        404 => ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::NotFound,
            detail: "managed DAT source returned HTTP 404".to_string(),
        },
        429 => ManagedDatUpdateOutcome::RateLimited {
            retry_after_seconds,
        },
        _ => ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::HttpStatus,
            detail: format!("managed DAT source returned HTTP {status}"),
        },
    }
}

fn storage_failure(error: ArchiveFsError) -> ManagedDatUpdateOutcome {
    ManagedDatUpdateOutcome::Failed {
        kind: ManagedDatUpdateFailureKind::Storage,
        detail: error.to_string(),
    }
}

fn is_git_commit_sha(value: &str) -> bool {
    (40..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use crate::dat::limits::DatLimits;
    use crate::dat::parsers::parse_dat_file;
    use crate::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn descriptor() -> ManagedDatSourceDescriptor {
        ManagedDatSourceDescriptor::mame_software_list("gamecom")
            .unwrap()
            .with_update_policy(ManagedDatUpdatePolicy::Manual)
    }

    fn state() -> ManagedDatState {
        ManagedDatState::new(&descriptor(), ManagedDatSnapshot::new(SHA_A).unwrap()).unwrap()
    }

    fn write_current_object(root: &Path, state: &ManagedDatState) -> PathBuf {
        let path = root
            .join(state.source_id.storage_relative_path())
            .join(OBJECTS_DIRECTORY)
            .join(&state.current_snapshot.sha256);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"<softwarelist name="gamecom" description="Game.com">
<software name="test"><description>Test</description><year>1997</year><publisher>Test</publisher>
<part name="cart" interface="cart"><dataarea name="rom" size="1"><rom name="test.bin" size="1" crc="00000000"/></dataarea></part>
</software></softwarelist>"#,
        )
        .unwrap();
        path
    }

    const REV_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const REV_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const REV_C: &str = "cccccccccccccccccccccccccccccccccccccccc";

    #[derive(Clone)]
    struct FakeReply {
        status: u16,
        body: Vec<u8>,
        content_length: Option<u64>,
        downloaded_bytes: Option<u64>,
        etag: Option<String>,
        last_modified: Option<String>,
        retry_after_seconds: Option<u64>,
    }

    impl FakeReply {
        fn ok(body: impl Into<Vec<u8>>) -> Self {
            let body = body.into();
            Self {
                status: 200,
                content_length: Some(body.len() as u64),
                downloaded_bytes: None,
                body,
                etag: None,
                last_modified: None,
                retry_after_seconds: None,
            }
        }

        fn status(status: u16) -> Self {
            Self {
                status,
                body: Vec::new(),
                content_length: Some(0),
                downloaded_bytes: Some(0),
                etag: None,
                last_modified: None,
                retry_after_seconds: None,
            }
        }
    }

    struct FakeTransport {
        replies: RefCell<Vec<std::result::Result<FakeReply, ManagedDatTransportError>>>,
        calls: RefCell<Vec<ManagedDatHttpRequest>>,
    }

    impl FakeTransport {
        fn new(replies: Vec<std::result::Result<FakeReply, ManagedDatTransportError>>) -> Self {
            Self {
                replies: RefCell::new(replies),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl ManagedDatTransport for FakeTransport {
        fn get(
            &self,
            request: &ManagedDatHttpRequest,
            maximum_bytes: u64,
            destination: &mut dyn Write,
        ) -> std::result::Result<ManagedDatHttpResponse, ManagedDatTransportError> {
            self.calls.borrow_mut().push(request.clone());
            let reply = self.replies.borrow_mut().remove(0)?;
            if (200..300).contains(&reply.status) && reply.body.len() as u64 > maximum_bytes {
                return Err(ManagedDatTransportError::new(
                    ManagedDatTransportFailureKind::InvalidResponse,
                    "mock body too large",
                ));
            }
            if (200..300).contains(&reply.status) {
                destination.write_all(&reply.body).map_err(|error| {
                    ManagedDatTransportError::new(
                        ManagedDatTransportFailureKind::Destination,
                        error.to_string(),
                    )
                })?;
            }
            Ok(ManagedDatHttpResponse {
                status: reply.status,
                content_length: reply.content_length,
                etag: reply.etag,
                last_modified: reply.last_modified,
                retry_after_seconds: reply.retry_after_seconds,
                downloaded_bytes: reply.downloaded_bytes.unwrap_or(reply.body.len() as u64),
            })
        }
    }

    fn revision(sha: &str) -> FakeReply {
        FakeReply::ok(format!(r#"{{"sha":"{sha}"}}"#))
    }

    fn mame_xml(name: &str, game: &str) -> Vec<u8> {
        format!(
            r#"<softwarelist name="{name}" description="Test"><software name="{game}"><description>Test</description><year>1997</year><publisher>Test</publisher><part name="cart" interface="cart"><dataarea name="rom" size="1"><rom name="{game}.bin" size="1" crc="00000000"/></dataarea></part></software></softwarelist>"#
        )
        .into_bytes()
    }

    fn update_options(root: PathBuf) -> ManagedDatUpdateOptions {
        ManagedDatUpdateOptions::new(root, 1_700_000_000)
    }

    fn install(root: PathBuf, revision_sha: &str, body: Vec<u8>) -> ManagedDatState {
        let transport =
            FakeTransport::new(vec![Ok(revision(revision_sha)), Ok(FakeReply::ok(body))]);
        let outcome =
            update_managed_dat(&descriptor(), &update_options(root.clone()), &transport).unwrap();
        assert!(matches!(outcome, ManagedDatUpdateOutcome::Updated { .. }));
        load_managed_dat_state(&root, &descriptor()).unwrap()
    }

    #[test]
    fn old_toml_entries_are_user_local() {
        let config: crate::dat::sources::DatSourcesConfig = toml::from_str(
            r#"[[sources]]
id = "old"
display_name = "Old"
path = "/tmp/old.dat"
kind = "file"
"#,
        )
        .unwrap();
        let (registry, problems) = DatSourceRegistry::from_config(&config);
        assert!(problems.is_empty());
        assert!(registry.entries()[0].is_user_local());
    }

    #[test]
    fn normal_new_sources_are_user_local() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("normal.dat");
        fs::write(&path, "clrmamepro ( name \"test\" )").unwrap();
        let mut registry = DatSourceRegistry::new();
        registry
            .add(DatSourceEntry::new(
                "normal".into(),
                "Normal".into(),
                path,
                DatSourceKind::File,
            ))
            .unwrap();
        assert!(registry.entries()[0].is_user_local());
    }

    #[test]
    fn origin_text_never_changes_local_ownership() {
        for origin in ["MAME", "https://github.com/mamedev/mame"] {
            let entry = DatSourceEntry {
                origin: Some(origin.into()),
                ..DatSourceEntry::new(
                    "local".into(),
                    "Local".into(),
                    PathBuf::from("/tmp/local.dat"),
                    DatSourceKind::File,
                )
            };
            assert!(entry.is_user_local());
        }
    }

    #[test]
    fn managed_source_id_is_stable_and_descriptor_is_fixed_mame_contract() {
        let descriptor = ManagedDatSourceDescriptor::mame_software_list("gamecom").unwrap();
        assert_eq!(
            descriptor.source_id().to_string(),
            "mame-software-list/gamecom"
        );
        assert_eq!(descriptor.repository(), MAME_REPOSITORY);
        assert_eq!(
            descriptor.repository_relative_path(),
            Path::new("hash/gamecom.xml")
        );
        assert_eq!(
            descriptor.expected_ecosystem(),
            DatEcosystem::MAMESoftwareList
        );
        assert_eq!(descriptor.expected_softwarelist_name(), "gamecom");
        assert_eq!(descriptor.update_policy(), ManagedDatUpdatePolicy::Disabled);
        descriptor.validate().unwrap();
    }

    #[test]
    fn repository_path_rejects_traversal_and_absolute_paths() {
        assert!(validate_repository_relative_path(Path::new("hash/../gamecom.xml")).is_err());
        assert!(validate_repository_relative_path(Path::new("/etc/passwd")).is_err());
        assert!(ManagedDatSourceId::mame_software_list("../gamecom").is_err());
    }

    #[test]
    fn managed_object_path_is_below_root_and_external_paths_cannot_be_claimed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let state = state();
        let expected = write_current_object(&root, &state);
        let resolved = resolve_current_managed_dat_source(&root, &state).unwrap();
        assert!(resolved.path().starts_with(&root));
        assert_eq!(resolved.path(), expected);
        assert_eq!(resolved.ownership(), DatSourceOwnership::EmuWizManaged);

        let external = temp.path().join("outside.dat");
        fs::write(&external, "not managed").unwrap();
        let missing =
            ManagedDatState::new(&descriptor(), ManagedDatSnapshot::new(SHA_B).unwrap()).unwrap();
        assert!(resolve_current_managed_dat_source(&root, &missing).is_err());
        assert!(external.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_managed_object_is_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let state = state();
        let external = temp.path().join("outside.dat");
        fs::write(&external, "outside").unwrap();
        let target = root
            .join(state.source_id.storage_relative_path())
            .join(OBJECTS_DIRECTORY)
            .join(&state.current_snapshot.sha256);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        symlink(&external, &target).unwrap();
        assert!(resolve_current_managed_dat_source(&root, &state).is_err());
    }

    #[test]
    fn current_previous_and_provenance_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let mut state = state();
        state.previous_snapshot = Some(ManagedDatSnapshot::new(SHA_B).unwrap());
        state.upstream_revision = Some("a1b2c3d4".into());
        state.etag = Some("\"etag\"".into());
        state.last_modified = Some("Wed, 21 Oct 2015 07:28:00 GMT".into());
        state.retrieved_at_unix_seconds = Some(1_700_000_000);
        state.last_checked_at_unix_seconds = Some(1_700_000_100);
        state.validation_summary = Some("parsed cleanly".into());
        state.last_failure = Some("previous timeout".into());
        save_managed_dat_state(&root, &state).unwrap();
        let loaded = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(loaded, state);
    }

    #[test]
    fn managed_current_snapshot_is_an_ordinary_read_only_dat_input() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let state = state();
        write_current_object(&root, &state);
        let source = resolve_current_managed_dat_source(&root, &state).unwrap();
        let parsed = parse_dat_file(source.path(), DatLimits::default()).unwrap();
        assert_eq!(parsed.dat.source.ecosystem, DatEcosystem::MAMESoftwareList);
    }

    #[test]
    fn user_local_source_has_no_managed_replacement_authority() {
        let source = DatSourceEntry::new(
            "local".into(),
            "Local".into(),
            PathBuf::from("/tmp/local.dat"),
            DatSourceKind::File,
        );
        assert!(source.is_user_local());
    }

    #[test]
    fn first_install_records_exact_immutable_revision_and_sha256() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let body = mame_xml("gamecom", "first");
        let expected_sha = {
            let mut hasher = Sha256::new();
            hasher.update(&body);
            let digest = hasher.finalize();
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let transport = FakeTransport::new(vec![Ok(revision(REV_A)), Ok(FakeReply::ok(body))]);
        let result =
            update_managed_dat(&descriptor(), &update_options(root.clone()), &transport).unwrap();
        assert_eq!(
            result,
            ManagedDatUpdateOutcome::Updated {
                upstream_revision: REV_A.to_string(),
                sha256: expected_sha.clone(),
            }
        );
        let state = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(state.upstream_revision.as_deref(), Some(REV_A));
        assert_eq!(state.sha256, expected_sha);
        assert!(transport.calls.borrow()[1].url.contains(REV_A));
        assert!(!transport.calls.borrow()[1].url.contains("/master/"));
    }

    #[test]
    fn wrong_name_malformed_and_truncated_downloads_never_install() {
        let cases = vec![
            (
                FakeReply::ok(mame_xml("wrong", "game")),
                ManagedDatUpdateFailureKind::WrongAuthoritativeName,
            ),
            (
                FakeReply::ok(b"<softwarelist".to_vec()),
                ManagedDatUpdateFailureKind::Parser,
            ),
            (
                {
                    let mut reply = FakeReply::ok(mame_xml("gamecom", "game"));
                    reply.content_length = Some(reply.body.len() as u64 + 1);
                    reply
                },
                ManagedDatUpdateFailureKind::TruncatedDownload,
            ),
        ];
        for (body_reply, kind) in cases {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(MANAGED_DAT_DIRECTORY);
            let transport = FakeTransport::new(vec![Ok(revision(REV_A)), Ok(body_reply)]);
            let result =
                update_managed_dat(&descriptor(), &update_options(root.clone()), &transport)
                    .unwrap();
            assert!(
                matches!(result, ManagedDatUpdateOutcome::Failed { kind: actual, .. } if actual == kind)
            );
            assert!(
                load_optional_managed_dat_state(&root, &descriptor())
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn oversized_response_is_rejected_before_install() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let mut reply = FakeReply::ok(mame_xml("gamecom", "large"));
        reply.content_length = Some(DEFAULT_MAX_FILE_SIZE + 1);
        let transport = FakeTransport::new(vec![Ok(revision(REV_A)), Ok(reply)]);
        let result =
            update_managed_dat(&descriptor(), &update_options(root.clone()), &transport).unwrap();
        assert!(matches!(
            result,
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::DownloadTooLarge,
                ..
            }
        ));
        assert!(
            load_optional_managed_dat_state(&root, &descriptor())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unchanged_304_is_up_to_date_and_check_uses_conditional_headers() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let mut state = install(root.clone(), REV_A, mame_xml("gamecom", "first"));
        state.etag = Some("\"revision-etag\"".into());
        state.last_modified = Some("Wed, 21 Oct 2015 07:28:00 GMT".into());
        save_managed_dat_state(&root, &state).unwrap();
        let transport = FakeTransport::new(vec![Ok(FakeReply::status(304))]);
        let result =
            check_managed_dat_update(&descriptor(), &update_options(root.clone()), &transport)
                .unwrap();
        assert!(matches!(result, ManagedDatUpdateOutcome::UpToDate { .. }));
        let request = &transport.calls.borrow()[0];
        assert!(
            request
                .headers
                .iter()
                .any(|(name, _)| name == "If-None-Match")
        );
        assert!(
            request
                .headers
                .iter()
                .any(|(name, _)| name == "If-Modified-Since")
        );
    }

    #[test]
    fn changed_revision_same_bytes_does_not_churn_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let body = mame_xml("gamecom", "same");
        let first = install(root.clone(), REV_A, body.clone());
        let transport = FakeTransport::new(vec![Ok(revision(REV_B)), Ok(FakeReply::ok(body))]);
        let result =
            update_managed_dat(&descriptor(), &update_options(root.clone()), &transport).unwrap();
        assert!(matches!(result, ManagedDatUpdateOutcome::Updated { .. }));
        let second = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(second.current_snapshot, first.current_snapshot);
        assert_eq!(second.previous_snapshot, first.previous_snapshot);
        assert_eq!(second.upstream_revision.as_deref(), Some(REV_B));
    }

    #[test]
    fn changed_bytes_promote_current_keep_previous_and_forget_third_old_object() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let first = install(root.clone(), REV_A, mame_xml("gamecom", "one"));
        let transport_b = FakeTransport::new(vec![
            Ok(revision(REV_B)),
            Ok(FakeReply::ok(mame_xml("gamecom", "two"))),
        ]);
        update_managed_dat(&descriptor(), &update_options(root.clone()), &transport_b).unwrap();
        let second = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(
            second.previous_snapshot.as_ref(),
            Some(&first.current_snapshot)
        );
        let transport_c = FakeTransport::new(vec![
            Ok(revision(REV_C)),
            Ok(FakeReply::ok(mame_xml("gamecom", "three"))),
        ]);
        update_managed_dat(&descriptor(), &update_options(root.clone()), &transport_c).unwrap();
        let third = load_managed_dat_state(&root, &descriptor()).unwrap();
        assert_eq!(
            third.previous_snapshot.as_ref(),
            Some(&second.current_snapshot)
        );
        let objects = root
            .join(third.source_id.storage_relative_path())
            .join(OBJECTS_DIRECTORY);
        assert!(!objects.join(first.current_snapshot.sha256).exists());
    }

    #[test]
    fn http_rate_limit_and_network_failures_preserve_current() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(MANAGED_DAT_DIRECTORY);
        let before = install(root.clone(), REV_A, mame_xml("gamecom", "first"));
        let mut limited = FakeReply::status(429);
        limited.retry_after_seconds = Some(60);
        let rate_transport = FakeTransport::new(vec![Ok(limited)]);
        assert_eq!(
            update_managed_dat(
                &descriptor(),
                &update_options(root.clone()),
                &rate_transport
            )
            .unwrap(),
            ManagedDatUpdateOutcome::RateLimited {
                retry_after_seconds: Some(60)
            }
        );
        let network_transport = FakeTransport::new(vec![Err(ManagedDatTransportError::new(
            ManagedDatTransportFailureKind::Timeout,
            "timeout",
        ))]);
        assert!(matches!(
            update_managed_dat(
                &descriptor(),
                &update_options(root.clone()),
                &network_transport
            )
            .unwrap(),
            ManagedDatUpdateOutcome::Failed {
                kind: ManagedDatUpdateFailureKind::Timeout,
                ..
            }
        ));
        assert_eq!(
            load_managed_dat_state(&root, &descriptor())
                .unwrap()
                .current_snapshot,
            before.current_snapshot
        );
    }

    #[test]
    fn forbidden_and_not_found_are_structured_and_do_not_create_state() {
        for (status, expected) in [
            (403, ManagedDatUpdateFailureKind::Forbidden),
            (404, ManagedDatUpdateFailureKind::NotFound),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(MANAGED_DAT_DIRECTORY);
            let transport = FakeTransport::new(vec![Ok(FakeReply::status(status))]);
            let result =
                update_managed_dat(&descriptor(), &update_options(root.clone()), &transport)
                    .unwrap();
            assert!(
                matches!(result, ManagedDatUpdateOutcome::Failed { kind, .. } if kind == expected)
            );
            assert!(
                load_optional_managed_dat_state(&root, &descriptor())
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn offline_mode_performs_zero_requests_and_no_background_work_exists() {
        let temp = tempfile::tempdir().unwrap();
        let mut options = update_options(temp.path().join(MANAGED_DAT_DIRECTORY));
        options.offline = true;
        let transport = FakeTransport::new(Vec::new());
        assert_eq!(
            check_managed_dat_update(&descriptor(), &options, &transport).unwrap(),
            ManagedDatUpdateOutcome::Offline
        );
        assert_eq!(
            update_managed_dat(&descriptor(), &options, &transport).unwrap(),
            ManagedDatUpdateOutcome::Offline
        );
        assert!(transport.calls.borrow().is_empty());
    }

    #[test]
    fn disabled_descriptor_performs_zero_requests() {
        let temp = tempfile::tempdir().unwrap();
        let transport = FakeTransport::new(Vec::new());
        let disabled = ManagedDatSourceDescriptor::mame_software_list("gamecom").unwrap();
        let result = update_managed_dat(
            &disabled,
            &update_options(temp.path().join(MANAGED_DAT_DIRECTORY)),
            &transport,
        )
        .unwrap();
        assert_eq!(result, ManagedDatUpdateOutcome::Disabled);
        assert!(transport.calls.borrow().is_empty());
    }
}
