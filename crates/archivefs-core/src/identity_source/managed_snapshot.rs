//! Provider-neutral managed source snapshots.
//!
//! This is the common lifecycle for identity providers whose bytes are fetched
//! from a remote or local source and interpreted by a provider-owned parser.
//! It deliberately does not know MAME, ScummVM, DAT, or any other provider
//! schema. The provider supplies [`ValidationReport`] after inspecting a staged
//! candidate; this module owns bounded staging, hashing, immutable storage,
//! explicit activation, history, rollback, and the `NeedsRecheck` signal.
//!
//! The existing managed-DAT module remains the authority for its closed typed
//! DAT contracts. This module is intentionally generic so future providers do
//! not have to add provider-specific download/cache/history implementations.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{ArchiveFsError, Result};

const STORE_SCHEMA_VERSION: u32 = 1;
const OBJECTS_DIR: &str = "objects";
const SNAPSHOTS_DIR: &str = "snapshots";
const STAGING_DIR: &str = "staging";
const STATE_FILE: &str = "state.json";
const LOCK_FILE: &str = ".update.lock";
const MAX_METADATA_LEN: usize = 4096;
const MAX_WARNINGS: usize = 128;
const MAX_REDIRECTS: usize = 5;
const CHUNK_SIZE: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(90);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether a source is remote or explicitly local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedSourceKind {
    Remote,
    Local,
}

/// A source reference. Remote references are restricted to HTTPS by
/// [`ManagedSourceDescriptor::validate`]. Local paths are never used as store
/// paths and are read only as regular, non-symlink files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManagedSourceReference {
    RemoteUrl(String),
    LocalPath(PathBuf),
}

/// Provider-neutral source metadata and policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedSourceDescriptor {
    pub provider_id: String,
    pub display_name: String,
    pub source_kind: ManagedSourceKind,
    pub source: ManagedSourceReference,
    pub expected_media_type: String,
    pub maximum_size_bytes: u64,
    pub attribution_url: Option<String>,
    pub parser_schema_version: String,
    pub trust: ManagedSourceTrust,
}

/// Source trust is provenance, not an authorization to execute content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedSourceTrust {
    Official,
    Community,
    UserProvided,
    Unverified,
}

impl ManagedSourceDescriptor {
    pub fn validate(&self) -> Result<()> {
        validate_component("provider ID", &self.provider_id)?;
        validate_metadata("display name", &self.display_name)?;
        validate_metadata("media type", &self.expected_media_type)?;
        validate_metadata("parser schema version", &self.parser_schema_version)?;
        if self.maximum_size_bytes == 0 {
            return Err(config("managed source size limit must be positive"));
        }
        if let Some(url) = &self.attribution_url {
            validate_https_url(url, "attribution URL")?;
        }
        match (&self.source_kind, &self.source) {
            (ManagedSourceKind::Remote, ManagedSourceReference::RemoteUrl(url)) => {
                validate_https_url(url, "source URL")?;
            }
            (ManagedSourceKind::Local, ManagedSourceReference::LocalPath(path)) => {
                if path.as_os_str().is_empty() {
                    return Err(config("local managed source path is empty"));
                }
            }
            _ => return Err(config("managed source kind and reference do not match")),
        }
        Ok(())
    }

    fn source_url(&self) -> Result<&str> {
        match &self.source {
            ManagedSourceReference::RemoteUrl(url) => Ok(url),
            ManagedSourceReference::LocalPath(_) => Err(config("source is local, not remote")),
        }
    }
}

/// Provider-supplied metadata for a candidate or a metadata-only check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedSourceMetadata {
    pub provider_version: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<u64>,
    /// Optional digest supplied by a trusted manifest or provider adapter.
    pub expected_sha256: Option<String>,
}

/// One immutable, content-addressed snapshot record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedSourceSnapshot {
    pub provider_id: String,
    pub source: ManagedSourceReference,
    pub provider_version: Option<String>,
    pub retrieved_at_unix_seconds: u64,
    pub sha256: String,
    pub size_bytes: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub expected_media_type: String,
    pub attribution_url: Option<String>,
    pub parser_schema_version: String,
    pub trust: ManagedSourceTrust,
    pub validation_summary: String,
    pub record_count: Option<u64>,
    pub warnings: Vec<String>,
}

/// Provider-owned validation result. Invalid candidates are never published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub valid: bool,
    pub summary: String,
    pub record_count: Option<u64>,
    pub warnings: Vec<String>,
}

/// A file staged and hashed, but not yet validated or active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedCandidate {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub metadata: ManagedSourceMetadata,
}

/// A validated candidate ready for preview or explicit activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCandidate {
    pub snapshot: ManagedSourceSnapshot,
    pub object_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoreState {
    schema_version: u32,
    active_sha256: Option<String>,
    history: Vec<String>,
    generation: u64,
}

/// Result of metadata-only update checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    Offline {
        active: Option<ManagedSourceSnapshot>,
    },
    Unchanged {
        metadata: ManagedSourceMetadata,
    },
    Available {
        metadata: ManagedSourceMetadata,
    },
}

/// Human-readable, side-effect-free activation preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationPreview {
    pub old: Option<ManagedSourceSnapshot>,
    pub new: ManagedSourceSnapshot,
    pub changed: bool,
    pub validation_status: String,
    pub warnings: Vec<String>,
}

/// Activation result and the generic stale-identity signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationResult {
    pub active: ManagedSourceSnapshot,
    pub generation: u64,
    pub change: SnapshotChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotChange {
    pub provider_id: String,
    pub previous_sha256: Option<String>,
    pub current_sha256: String,
    pub freshness: VerificationFreshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationFreshness {
    Current,
    NeedsRecheck,
}

/// Metadata returned by a transport without downloading a body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceResponseMetadata {
    pub status: u16,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<u64>,
    pub provider_version: Option<String>,
}

/// Narrow transport seam. Tests inject a fixture; production uses the HTTPS
/// implementation below. `fetch` must stream through the supplied writer.
pub trait ManagedSourceTransport {
    fn metadata(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> std::result::Result<SourceResponseMetadata, String>;
    fn fetch(
        &self,
        url: &str,
        headers: &[(String, String)],
        maximum_size: u64,
        destination: &mut dyn Write,
    ) -> std::result::Result<SourceResponseMetadata, String>;
}

/// Safe HTTPS transport with bounded redirects and streaming size checks.
#[derive(Debug, Clone)]
pub struct HttpsManagedSourceTransport {
    agent: ureq::Agent,
}

impl Default for HttpsManagedSourceTransport {
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

impl ManagedSourceTransport for HttpsManagedSourceTransport {
    fn metadata(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> std::result::Result<SourceResponseMetadata, String> {
        let response = self.request(url, headers, true)?;
        if matches!(response.0.status, 405 | 501) {
            // Some servers do not implement HEAD. This fallback discards the
            // GET body and still does not stage or activate any candidate.
            return Ok(self.request(url, headers, false)?.0);
        }
        Ok(response.0)
    }

    fn fetch(
        &self,
        url: &str,
        headers: &[(String, String)],
        maximum_size: u64,
        destination: &mut dyn Write,
    ) -> std::result::Result<SourceResponseMetadata, String> {
        let (metadata, mut response) = self.request(url, headers, false)?;
        if !(200..300).contains(&metadata.status) {
            return Err(format!("source returned HTTP {}", metadata.status));
        }
        if metadata
            .content_length
            .is_some_and(|length| length > maximum_size)
        {
            return Err("source Content-Length exceeds configured limit".into());
        }
        let mut reader = response.body_mut().as_reader();
        let mut buffer = [0u8; CHUNK_SIZE];
        let mut total = 0u64;
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or_else(|| "source size overflow".to_string())?;
            if total > maximum_size {
                return Err("source exceeded configured size limit".into());
            }
            destination
                .write_all(&buffer[..count])
                .map_err(|error| error.to_string())?;
        }
        if metadata
            .content_length
            .is_some_and(|length| length != total)
        {
            return Err("source body was truncated".into());
        }
        Ok(SourceResponseMetadata {
            content_length: Some(total),
            ..metadata
        })
    }
}

impl HttpsManagedSourceTransport {
    fn request(
        &self,
        initial_url: &str,
        headers: &[(String, String)],
        head: bool,
    ) -> std::result::Result<(SourceResponseMetadata, http::Response<ureq::Body>), String> {
        let mut current = initial_url.to_string();
        for hop in 0..=MAX_REDIRECTS {
            validate_https_url(&current, "source URL").map_err(|error| error.to_string())?;
            let mut request = if head {
                self.agent.head(&current)
            } else {
                self.agent.get(&current)
            };
            request = request.header("Accept-Encoding", "identity");
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let response = request.call().map_err(|error| error.to_string())?;
            let status = response.status().as_u16();
            if (300..400).contains(&status) {
                if hop == MAX_REDIRECTS {
                    return Err("source redirect limit exceeded".into());
                }
                let next = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| "source redirect omitted Location".to_string())?;
                let next = Url::parse(&current)
                    .and_then(|base| base.join(next))
                    .map_err(|error| error.to_string())?
                    .to_string();
                current = next;
                continue;
            }
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
            };
            let content_length = header("content-length")
                .map(|value| value.parse::<u64>().map_err(|error| error.to_string()))
                .transpose()?;
            return Ok((
                SourceResponseMetadata {
                    status,
                    etag: header("etag"),
                    last_modified: header("last-modified"),
                    content_length,
                    provider_version: header("x-provider-version"),
                },
                response,
            ));
        }
        Err("source redirect loop detected".into())
    }
}

/// Filesystem-backed immutable snapshot store.
#[derive(Debug, Clone)]
pub struct ManagedSourceStore {
    root: PathBuf,
    descriptor: ManagedSourceDescriptor,
}

impl ManagedSourceStore {
    pub fn new(root: PathBuf, descriptor: ManagedSourceDescriptor) -> Result<Self> {
        descriptor.validate()?;
        validate_root(&root)?;
        Ok(Self { root, descriptor })
    }

    pub fn descriptor(&self) -> &ManagedSourceDescriptor {
        &self.descriptor
    }

    pub fn active_snapshot(&self) -> Result<Option<ManagedSourceSnapshot>> {
        let state = self.load_state()?;
        state
            .active_sha256
            .map(|hash| self.load_snapshot(&hash).map(Some))
            .unwrap_or(Ok(None))
    }

    pub fn list_snapshots(&self) -> Result<Vec<ManagedSourceSnapshot>> {
        let directory = self.snapshots_dir();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(ArchiveFsError::io(directory, error)),
        };
        let mut snapshots = Vec::new();
        for entry in entries {
            let path = entry.map_err(ArchiveFsError::from)?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let hash = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or_else(|| config("snapshot filename is not valid UTF-8"))?;
            snapshots.push(self.load_snapshot(hash)?);
        }
        snapshots.sort_by(|left, right| {
            right
                .retrieved_at_unix_seconds
                .cmp(&left.retrieved_at_unix_seconds)
                .then_with(|| left.sha256.cmp(&right.sha256))
        });
        Ok(snapshots)
    }

    pub fn check_for_update(
        &self,
        offline: bool,
        transport: &dyn ManagedSourceTransport,
    ) -> Result<UpdateCheck> {
        let active = self.active_snapshot()?;
        if offline {
            return Ok(UpdateCheck::Offline { active });
        }
        let url = self.descriptor.source_url()?;
        let mut headers = Vec::new();
        if let Some(snapshot) = &active {
            if let Some(etag) = &snapshot.etag {
                headers.push(("If-None-Match".into(), etag.clone()));
            }
            if let Some(last_modified) = &snapshot.last_modified {
                headers.push(("If-Modified-Since".into(), last_modified.clone()));
            }
        }
        let metadata = transport
            .metadata(url, &headers)
            .map_err(|detail| config(format!("managed source metadata check failed: {detail}")))?;
        if metadata.status == 304 {
            return Ok(UpdateCheck::Unchanged {
                metadata: metadata.into(),
            });
        }
        if !(200..300).contains(&metadata.status) {
            return Err(config(format!(
                "managed source metadata returned HTTP {}",
                metadata.status
            )));
        }
        let metadata: ManagedSourceMetadata = metadata.into();
        let unchanged = active.as_ref().is_some_and(|snapshot| {
            metadata.provider_version.is_some()
                && metadata.provider_version == snapshot.provider_version
                || metadata.etag.is_some() && metadata.etag == snapshot.etag
                || metadata.last_modified.is_some()
                    && metadata.last_modified == snapshot.last_modified
        });
        Ok(if unchanged {
            UpdateCheck::Unchanged { metadata }
        } else {
            UpdateCheck::Available { metadata }
        })
    }

    pub fn stage_local(&self, metadata: ManagedSourceMetadata) -> Result<StagedCandidate> {
        self.descriptor.validate()?;
        let ManagedSourceReference::LocalPath(path) = &self.descriptor.source else {
            return Err(config("stage_local requires a local source"));
        };
        let source_metadata =
            fs::symlink_metadata(path).map_err(|error| ArchiveFsError::io(path.clone(), error))?;
        if !source_metadata.is_file() || source_metadata.file_type().is_symlink() {
            return Err(config("local source must be a regular non-symlink file"));
        }
        if source_metadata.len() > self.descriptor.maximum_size_bytes {
            return Err(config("local source exceeds configured size limit"));
        }
        let mut input =
            File::open(path).map_err(|error| ArchiveFsError::io(path.clone(), error))?;
        self.stage_reader(&mut input, metadata)
    }

    pub fn fetch_candidate(
        &self,
        transport: &dyn ManagedSourceTransport,
    ) -> Result<StagedCandidate> {
        let url = self.descriptor.source_url()?;
        let directory = self.prepare_staging_dir()?;
        let path = unique_temp_path(&directory, "candidate")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| ArchiveFsError::io(path.clone(), error))?;
        let response =
            match transport.fetch(url, &[], self.descriptor.maximum_size_bytes, &mut file) {
                Ok(response) => response,
                Err(detail) => {
                    drop(file);
                    let _ = fs::remove_file(&path);
                    return Err(config(format!("managed source fetch failed: {detail}")));
                }
            };
        file.sync_all()
            .map_err(|error| ArchiveFsError::io(path.clone(), error))?;
        drop(file);
        self.hash_staged(path, response.into())
    }

    pub fn validate_candidate(
        &self,
        staged: StagedCandidate,
        report: ValidationReport,
    ) -> Result<ValidatedCandidate> {
        if !report.valid {
            let _ = fs::remove_file(&staged.path);
            return Err(config(format!(
                "managed source candidate failed validation: {}",
                report.summary
            )));
        }
        if report.summary.len() > MAX_METADATA_LEN || report.summary.contains('\0') {
            return Err(config("validation summary is too long or contains NUL"));
        }
        let warnings: Vec<_> = report.warnings.into_iter().take(MAX_WARNINGS).collect();
        let snapshot = ManagedSourceSnapshot {
            provider_id: self.descriptor.provider_id.clone(),
            source: self.descriptor.source.clone(),
            provider_version: staged.metadata.provider_version,
            retrieved_at_unix_seconds: now(),
            sha256: staged.sha256.clone(),
            size_bytes: staged.size_bytes,
            etag: staged.metadata.etag,
            last_modified: staged.metadata.last_modified,
            expected_media_type: self.descriptor.expected_media_type.clone(),
            attribution_url: self.descriptor.attribution_url.clone(),
            parser_schema_version: self.descriptor.parser_schema_version.clone(),
            trust: self.descriptor.trust,
            validation_summary: report.summary,
            record_count: report.record_count,
            warnings,
        };
        self.publish_snapshot(staged.path, snapshot.clone())
    }

    pub fn preview_activation(&self, candidate: &ValidatedCandidate) -> Result<ActivationPreview> {
        self.validate_snapshot_record(&candidate.snapshot)?;
        let old = self.active_snapshot()?;
        Ok(ActivationPreview {
            changed: old.as_ref().map(|snapshot| snapshot.sha256.as_str())
                != Some(candidate.snapshot.sha256.as_str()),
            old,
            new: candidate.snapshot.clone(),
            validation_status: "valid".into(),
            warnings: candidate.snapshot.warnings.clone(),
        })
    }

    pub fn activate_snapshot(
        &self,
        candidate: &ValidatedCandidate,
        expected_active: Option<&str>,
    ) -> Result<ActivationResult> {
        let _lock = self.lock()?;
        self.validate_snapshot_record(&candidate.snapshot)?;
        let mut state = self.load_state()?;
        if state.active_sha256.as_deref() != expected_active {
            return Err(config(
                "activation request is stale; active snapshot changed",
            ));
        }
        let previous = state.active_sha256.clone();
        if previous.as_deref() != Some(candidate.snapshot.sha256.as_str()) {
            if let Some(hash) = previous.clone() {
                state.history.retain(|item| item != &hash);
                state.history.insert(0, hash);
            }
            state.active_sha256 = Some(candidate.snapshot.sha256.clone());
            state.generation = state.generation.saturating_add(1);
            self.save_state(&state)?;
        }
        Ok(ActivationResult {
            active: candidate.snapshot.clone(),
            generation: state.generation,
            change: SnapshotChange {
                provider_id: self.descriptor.provider_id.clone(),
                previous_sha256: previous.clone(),
                current_sha256: candidate.snapshot.sha256.clone(),
                freshness: if previous.as_deref() == Some(candidate.snapshot.sha256.as_str()) {
                    VerificationFreshness::Current
                } else {
                    VerificationFreshness::NeedsRecheck
                },
            },
        })
    }

    pub fn rollback_snapshot(&self, hash: &str) -> Result<ActivationResult> {
        validate_sha(hash)?;
        let _lock = self.lock()?;
        let mut state = self.load_state()?;
        let previous = state.active_sha256.clone();
        if !state.history.iter().any(|item| item == hash) {
            return Err(config("rollback target is not in snapshot history"));
        }
        let target = self.load_snapshot(hash)?;
        state.history.retain(|item| item != hash);
        if let Some(previous) = previous.clone() {
            state.history.insert(0, previous);
        }
        state.active_sha256 = Some(hash.to_string());
        state.generation = state.generation.saturating_add(1);
        self.save_state(&state)?;
        Ok(ActivationResult {
            active: target,
            generation: state.generation,
            change: SnapshotChange {
                provider_id: self.descriptor.provider_id.clone(),
                previous_sha256: previous,
                current_sha256: hash.to_string(),
                freshness: VerificationFreshness::NeedsRecheck,
            },
        })
    }

    fn stage_reader(
        &self,
        reader: &mut dyn Read,
        metadata: ManagedSourceMetadata,
    ) -> Result<StagedCandidate> {
        let directory = self.prepare_staging_dir()?;
        let path = unique_temp_path(&directory, "candidate")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| ArchiveFsError::io(path.clone(), error))?;
        let mut size = 0u64;
        let result = (|| -> io::Result<()> {
            let mut buffer = [0u8; CHUNK_SIZE];
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                size = size
                    .checked_add(count as u64)
                    .ok_or_else(|| io::Error::other("source size overflow"))?;
                if size > self.descriptor.maximum_size_bytes {
                    return Err(io::Error::other("source exceeded configured size limit"));
                }
                file.write_all(&buffer[..count])?;
            }
            file.sync_all()
        })();
        drop(file);
        if let Err(error) = result {
            let _ = fs::remove_file(&path);
            return Err(config(error.to_string()));
        }
        self.hash_staged(
            path,
            ManagedSourceMetadata {
                content_length: Some(size),
                ..metadata
            },
        )
    }

    fn hash_staged(
        &self,
        path: PathBuf,
        metadata: ManagedSourceMetadata,
    ) -> Result<StagedCandidate> {
        let mut file =
            File::open(&path).map_err(|error| ArchiveFsError::io(path.clone(), error))?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let mut buffer = [0u8; CHUNK_SIZE];
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|error| ArchiveFsError::io(path.clone(), error))?;
            if count == 0 {
                break;
            }
            size = size.saturating_add(count as u64);
            hasher.update(&buffer[..count]);
        }
        if metadata
            .content_length
            .is_some_and(|expected| expected != size)
        {
            let _ = fs::remove_file(&path);
            return Err(config(
                "staged source length does not match declared length",
            ));
        }
        let sha256 = hex(&hasher.finalize());
        if let Some(expected) = &metadata.expected_sha256 {
            validate_sha(expected)?;
            if !expected.eq_ignore_ascii_case(&sha256) {
                let _ = fs::remove_file(&path);
                return Err(config(
                    "staged source SHA-256 does not match expected digest",
                ));
            }
        }
        Ok(StagedCandidate {
            path,
            sha256,
            size_bytes: size,
            metadata,
        })
    }

    fn publish_snapshot(
        &self,
        staged: PathBuf,
        snapshot: ManagedSourceSnapshot,
    ) -> Result<ValidatedCandidate> {
        self.validate_snapshot_record(&snapshot)?;
        let _lock = self.lock()?;
        let objects = self.objects_dir();
        let snapshots = self.snapshots_dir();
        self.ensure_directory(&objects)?;
        self.ensure_directory(&snapshots)?;
        let object_path = objects.join(&snapshot.sha256);
        if let Ok(metadata) = fs::symlink_metadata(&object_path) {
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || sha256_file(&object_path)? != snapshot.sha256
            {
                return Err(config(
                    "existing snapshot object conflicts with its SHA-256 name",
                ));
            }
            let _ = fs::remove_file(&staged);
        } else {
            fs::rename(&staged, &object_path)
                .map_err(|error| ArchiveFsError::io(object_path.clone(), error))?;
        }
        let record_path = snapshots.join(format!("{}.json", snapshot.sha256));
        if let Ok(existing) = fs::read_to_string(&record_path) {
            let existing: ManagedSourceSnapshot =
                serde_json::from_str(&existing).map_err(|error| config(error.to_string()))?;
            if existing != snapshot {
                return Err(config("duplicate snapshot metadata conflicts"));
            }
        } else {
            let body = serde_json::to_string_pretty(&snapshot)
                .map_err(|error| config(error.to_string()))?;
            crate::atomic_write_text(&record_path, &format!("{body}\n"))?;
        }
        Ok(ValidatedCandidate {
            snapshot,
            object_path,
        })
    }

    fn validate_snapshot_record(&self, snapshot: &ManagedSourceSnapshot) -> Result<()> {
        if snapshot.provider_id != self.descriptor.provider_id
            || snapshot.source != self.descriptor.source
        {
            return Err(config("snapshot does not belong to this managed source"));
        }
        validate_sha(&snapshot.sha256)?;
        if snapshot.size_bytes > self.descriptor.maximum_size_bytes
            || snapshot.warnings.len() > MAX_WARNINGS
        {
            return Err(config("snapshot exceeds managed source bounds"));
        }
        Ok(())
    }

    fn load_snapshot(&self, hash: &str) -> Result<ManagedSourceSnapshot> {
        validate_sha(hash)?;
        let path = self.snapshots_dir().join(format!("{hash}.json"));
        let body =
            fs::read_to_string(&path).map_err(|error| ArchiveFsError::io(path.clone(), error))?;
        let snapshot: ManagedSourceSnapshot =
            serde_json::from_str(&body).map_err(|error| config(error.to_string()))?;
        self.validate_snapshot_record(&snapshot)?;
        let object = self.objects_dir().join(hash);
        let metadata = fs::symlink_metadata(&object)
            .map_err(|error| ArchiveFsError::io(object.clone(), error))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() != snapshot.size_bytes
            || sha256_file(&object)? != hash
        {
            return Err(config("snapshot object failed integrity verification"));
        }
        Ok(snapshot)
    }

    fn load_state(&self) -> Result<StoreState> {
        let path = self.state_path();
        match fs::read_to_string(&path) {
            Ok(body) => {
                let state: StoreState =
                    serde_json::from_str(&body).map_err(|error| config(error.to_string()))?;
                if state.schema_version != STORE_SCHEMA_VERSION {
                    return Err(config("unsupported managed snapshot state schema"));
                }
                if let Some(hash) = &state.active_sha256 {
                    validate_sha(hash)?;
                }
                for hash in &state.history {
                    validate_sha(hash)?;
                }
                Ok(state)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(StoreState {
                schema_version: STORE_SCHEMA_VERSION,
                active_sha256: None,
                history: Vec::new(),
                generation: 0,
            }),
            Err(error) => Err(ArchiveFsError::io(path, error)),
        }
    }

    fn save_state(&self, state: &StoreState) -> Result<()> {
        let body =
            serde_json::to_string_pretty(state).map_err(|error| config(error.to_string()))?;
        crate::atomic_write_text(&self.state_path(), &format!("{body}\n"))
    }

    fn lock(&self) -> Result<UpdateLock> {
        self.ensure_directory(&self.source_root())?;
        let path = self.source_root().join(LOCK_FILE);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                writeln!(file, "pid={} started={}", std::process::id(), now())
                    .map_err(|error| ArchiveFsError::io(path.clone(), error))?;
                file.sync_all()
                    .map_err(|error| ArchiveFsError::io(path.clone(), error))?;
                Ok(UpdateLock { path })
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let existing = fs::read_to_string(&path).map_err(|read_error| {
                    config(format!(
                        "another managed source update is active: {read_error}"
                    ))
                })?;
                let pid = existing.split_whitespace().find_map(|field| {
                    field
                        .strip_prefix("pid=")
                        .and_then(|value| value.parse::<i32>().ok())
                });
                if let Some(pid) = pid
                    && pid != std::process::id() as i32
                    && !process_exists(pid)
                {
                    fs::remove_file(&path).map_err(|remove_error| {
                        config(format!(
                            "stale managed source lock could not be removed: {remove_error}"
                        ))
                    })?;
                    return self.lock();
                }
                Err(config("another managed source update is active"))
            }
            Err(error) => Err(ArchiveFsError::io(path, error)),
        }
    }

    fn prepare_staging_dir(&self) -> Result<PathBuf> {
        let directory = self.source_root().join(STAGING_DIR);
        self.ensure_directory(&directory)?;
        Ok(directory)
    }

    fn ensure_directory(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
        reject_symlink(path)?;
        Ok(())
    }

    fn source_root(&self) -> PathBuf {
        self.root.join(&self.descriptor.provider_id)
    }
    fn objects_dir(&self) -> PathBuf {
        self.source_root().join(OBJECTS_DIR)
    }
    fn snapshots_dir(&self) -> PathBuf {
        self.source_root().join(SNAPSHOTS_DIR)
    }
    fn state_path(&self) -> PathBuf {
        self.source_root().join(STATE_FILE)
    }
}

struct UpdateLock {
    path: PathBuf,
}
impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl From<SourceResponseMetadata> for ManagedSourceMetadata {
    fn from(value: SourceResponseMetadata) -> Self {
        Self {
            provider_version: value.provider_version,
            etag: value.etag,
            last_modified: value.last_modified,
            content_length: value.content_length,
            expected_sha256: None,
        }
    }
}

fn validate_root(root: &Path) -> Result<()> {
    if !root.is_absolute()
        || root
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(config(
            "managed snapshot root must be absolute and traversal-free",
        ));
    }
    check_existing_ancestors(root)
}

fn validate_component(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(config(format!(
            "{label} must be a bounded path-safe identifier"
        )));
    }
    Ok(())
}

fn validate_metadata(label: &str, value: &str) -> Result<()> {
    if value.len() > MAX_METADATA_LEN || value.contains('\0') {
        return Err(config(format!("{label} is too long or contains NUL")));
    }
    Ok(())
}

fn validate_https_url(value: &str, label: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|error| config(format!("{label} is invalid: {error}")))?;
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err(config(format!("{label} must be HTTPS without credentials")));
    }
    Ok(())
}

fn validate_sha(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(config("snapshot SHA-256 is invalid"));
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<()> {
    check_existing_ancestors(path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(config(format!(
            "managed snapshot path is not a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn check_existing_ancestors(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(config(format!(
                    "managed snapshot path must not use symlinks: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ArchiveFsError::io(current, error)),
        }
    }
    Ok(())
}

fn unique_temp_path(directory: &Path, stem: &str) -> Result<PathBuf> {
    for sequence in 0..128u32 {
        let path = directory.join(format!(".{stem}-{}-{sequence}.part", std::process::id()));
        match fs::metadata(&path) {
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path),
            Err(error) => return Err(ArchiveFsError::io(path, error)),
        }
    }
    Err(config(
        "could not allocate a private managed snapshot staging path",
    ))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; CHUNK_SIZE];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
fn config(message: impl Into<String>) -> ArchiveFsError {
    ArchiveFsError::Config(message.into())
}

fn process_exists(pid: i32) -> bool {
    // A missing process is the only case in which automatic stale-lock
    // recovery is safe. Permission errors are treated as alive.
    match unsafe { libc::kill(pid, 0) } {
        0 => true,
        -1 => !matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        ),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    const BODY_V1: &[u8] = b"fixture provider v1";
    const BODY_V2: &[u8] = b"fixture provider v2";

    #[derive(Clone, Default)]
    struct FixtureTransport {
        bodies: Arc<Mutex<VecDeque<Vec<u8>>>>,
        metadata: SourceResponseMetadata,
    }
    impl ManagedSourceTransport for FixtureTransport {
        fn metadata(
            &self,
            _: &str,
            _: &[(String, String)],
        ) -> std::result::Result<SourceResponseMetadata, String> {
            Ok(self.metadata.clone())
        }
        fn fetch(
            &self,
            _: &str,
            _: &[(String, String)],
            max: u64,
            destination: &mut dyn Write,
        ) -> std::result::Result<SourceResponseMetadata, String> {
            let body = self
                .bodies
                .lock()
                .unwrap()
                .pop_front()
                .ok_or("no fixture body")?;
            if body.len() as u64 > max {
                return Err("oversized".into());
            }
            destination
                .write_all(&body)
                .map_err(|error| error.to_string())?;
            Ok(SourceResponseMetadata {
                status: 200,
                content_length: Some(body.len() as u64),
                ..self.metadata.clone()
            })
        }
    }

    fn descriptor() -> ManagedSourceDescriptor {
        ManagedSourceDescriptor {
            provider_id: "fixture".into(),
            display_name: "Fixture provider".into(),
            source_kind: ManagedSourceKind::Remote,
            source: ManagedSourceReference::RemoteUrl("https://example.test/source".into()),
            expected_media_type: "application/octet-stream".into(),
            maximum_size_bytes: 1024,
            attribution_url: Some("https://example.test".into()),
            parser_schema_version: "1".into(),
            trust: ManagedSourceTrust::Community,
        }
    }

    fn report(body: &[u8]) -> ValidationReport {
        ValidationReport {
            valid: true,
            summary: format!("{} bytes", body.len()),
            record_count: Some(1),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn fixture_lifecycle_activation_and_rollback_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let store = ManagedSourceStore::new(root.path().join("snapshots"), descriptor()).unwrap();
        let transport = FixtureTransport {
            bodies: Arc::new(Mutex::new(VecDeque::from([
                BODY_V1.to_vec(),
                BODY_V2.to_vec(),
            ]))),
            metadata: SourceResponseMetadata {
                status: 200,
                etag: Some("v1".into()),
                ..Default::default()
            },
        };
        let v1 = store
            .validate_candidate(store.fetch_candidate(&transport).unwrap(), report(BODY_V1))
            .unwrap();
        assert!(store.preview_activation(&v1).unwrap().old.is_none());
        let first = store.activate_snapshot(&v1, None).unwrap();
        assert_eq!(first.change.freshness, VerificationFreshness::NeedsRecheck);
        let v2 = store
            .validate_candidate(store.fetch_candidate(&transport).unwrap(), report(BODY_V2))
            .unwrap();
        let preview = store.preview_activation(&v2).unwrap();
        assert!(preview.changed);
        let second = store
            .activate_snapshot(&v2, Some(&v1.snapshot.sha256))
            .unwrap();
        assert_eq!(
            second.change.previous_sha256.as_deref(),
            Some(v1.snapshot.sha256.as_str())
        );
        assert_eq!(store.list_snapshots().unwrap().len(), 2);
        let rolled = store.rollback_snapshot(&v1.snapshot.sha256).unwrap();
        assert_eq!(rolled.active.sha256, v1.snapshot.sha256);
    }

    #[test]
    fn failed_validation_never_becomes_active_and_duplicate_is_deduplicated() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("source.bin");
        fs::write(&local, BODY_V1).unwrap();
        let mut source = descriptor();
        source.source_kind = ManagedSourceKind::Local;
        source.source = ManagedSourceReference::LocalPath(local);
        let store = ManagedSourceStore::new(root.path().join("snapshots"), source).unwrap();
        let staged = store.stage_local(ManagedSourceMetadata::default()).unwrap();
        assert!(
            store
                .validate_candidate(
                    staged,
                    ValidationReport {
                        valid: false,
                        summary: "bad fixture".into(),
                        record_count: None,
                        warnings: Vec::new()
                    }
                )
                .is_err()
        );
        assert!(store.active_snapshot().unwrap().is_none());
        let a = store
            .validate_candidate(
                store.stage_local(ManagedSourceMetadata::default()).unwrap(),
                report(BODY_V1),
            )
            .unwrap();
        let b = store
            .validate_candidate(
                store.stage_local(ManagedSourceMetadata::default()).unwrap(),
                report(BODY_V1),
            )
            .unwrap();
        assert_eq!(a.snapshot.sha256, b.snapshot.sha256);
        assert_eq!(store.list_snapshots().unwrap().len(), 1);
    }

    #[test]
    fn expected_hash_mismatch_is_rejected_before_publication() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("source.bin");
        fs::write(&local, BODY_V1).unwrap();
        let mut source = descriptor();
        source.source_kind = ManagedSourceKind::Local;
        source.source = ManagedSourceReference::LocalPath(local);
        let store = ManagedSourceStore::new(root.path().join("snapshots"), source).unwrap();
        let staged = store.stage_local(ManagedSourceMetadata {
            expected_sha256: Some("00".repeat(32)),
            ..Default::default()
        });
        assert!(staged.is_err());
        assert!(store.list_snapshots().unwrap().is_empty());
    }

    #[test]
    fn stale_activation_and_lock_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let store = ManagedSourceStore::new(root.path().join("snapshots"), descriptor()).unwrap();
        let transport = FixtureTransport {
            bodies: Arc::new(Mutex::new(VecDeque::from([BODY_V1.to_vec()]))),
            metadata: SourceResponseMetadata::default(),
        };
        let candidate = store
            .validate_candidate(store.fetch_candidate(&transport).unwrap(), report(BODY_V1))
            .unwrap();
        assert!(
            store
                .activate_snapshot(&candidate, Some("00".repeat(32).as_str()))
                .is_err()
        );
        let lock = store.lock().unwrap();
        assert!(store.lock().is_err());
        drop(lock);
    }

    #[test]
    fn descriptor_rejects_unsafe_remote_references() {
        let mut source = descriptor();
        source.source = ManagedSourceReference::RemoteUrl("http://example.test/source".into());
        assert!(source.validate().is_err());
        source.source = ManagedSourceReference::RemoteUrl("https://example.test/../source".into());
        assert!(source.validate().is_ok());
    }
}
