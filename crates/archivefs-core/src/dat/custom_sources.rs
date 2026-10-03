//! User-registered remote and local DAT sources.
//!
//! A URL a person supplies is a *source*, never authority. This module adds the
//! missing layer above the existing single-URL acquisition adapter
//! ([`crate::dat::acquisition`]) and the immutable managed snapshot store
//! ([`crate::identity_source::managed_snapshot`]):
//!
//! * a persisted, bounded registry of sources (GitHub, plain HTTPS, local file);
//! * an explicit lifecycle that never silently replaces an active DAT:
//!
//!   ```text
//!   register -> check update -> download to staging -> validate -> preview
//!            -> activate (explicit, stale-checked) -> history / rollback
//!   ```
//!
//! * bounded extraction of a DAT from a downloaded ZIP release asset.
//!
//! Network rules are not re-implemented here. Every remote request goes through
//! [`validate_public_https_url`] (HTTPS only, no embedded credentials, no
//! private/loopback/link-local/metadata destinations) and the managed HTTPS
//! transport, which refuses ambient proxy settings, follows only validated
//! redirects, and streams under a size limit. Downloaded bytes are data only;
//! nothing is ever executed.
//!
//! Trust is always [`DatAcquisitionTrust::UserProvided`]. A GitHub host does
//! not make a source official, and a DAT header cannot promote it.
//!
//! This module has no GUI and no migration. A later GUI-v2 page can drive
//! [`CustomDatSourceRegistry`] and [`CustomDatLifecycle`] directly.

use std::fs;
use std::io::{Cursor, Read};
use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::dat::acquisition::{
    DatAcquisitionSource, DatAcquisitionTrust, MAX_REMOTE_DAT_BYTES, parse_source_url,
};
use crate::dat::limits::DatLimits;
use crate::dat::model::DatEcosystem;
use crate::dat::parsers::parse_dat_file;
use crate::identity_source::managed_snapshot::{
    ActivationPreview, ActivationResult, HttpsManagedSourceTransport, ManagedSourceDescriptor,
    ManagedSourceKind, ManagedSourceMetadata, ManagedSourceReference, ManagedSourceSnapshot,
    ManagedSourceStore, ManagedSourceTransport, ManagedSourceTrust, StagedCandidate, UpdateCheck,
    ValidatedCandidate, ValidationReport,
};
use crate::identity_source::net_policy::{
    HostResolver, StaticResolver, SystemResolver, validate_public_https_url,
};
use crate::{ArchiveFsError, Result};

/// Bound for a downloaded DAT or DAT release pack (same ceiling as acquisition).
pub const CUSTOM_DAT_MAX_BYTES: u64 = MAX_REMOTE_DAT_BYTES;
/// Maximum number of registered custom sources.
pub const CUSTOM_DAT_MAX_SOURCES: usize = 256;
/// Maximum number of members a downloaded ZIP may contain.
pub const CUSTOM_DAT_MAX_ZIP_MEMBERS: usize = 4096;
/// Maximum uncompressed size of the DAT member extracted from a ZIP.
pub const CUSTOM_DAT_MAX_MEMBER_BYTES: u64 = CUSTOM_DAT_MAX_BYTES;
const MAX_NAME_BYTES: usize = 128;
const MAX_URL_BYTES: usize = 2048;
const REGISTRY_SCHEMA: u32 = 1;
const PARSER_SCHEMA: &str = "custom-dat-source-v1";

/// How a source is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomDatSourceKind {
    Github,
    Https,
    Local,
}

/// A registered source. Registration is metadata only: it never downloads,
/// activates, or touches a local file.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomDatSource {
    /// Stable ID derived from the source address, never from a filename.
    pub id: String,
    pub display_name: String,
    pub kind: CustomDatSourceKind,
    pub reference: ManagedSourceReference,
    /// Always [`DatAcquisitionTrust::UserProvided`] for a custom source.
    pub trust: DatAcquisitionTrust,
    /// Parsed GitHub/host provenance for remote sources.
    pub acquisition: Option<DatAcquisitionSource>,
}

impl std::fmt::Debug for CustomDatSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomDatSource")
            .field("id", &self.id)
            .field("display_name", &self.display_name)
            .field("kind", &self.kind)
            .field("address", &self.display_address())
            .field("trust", &self.trust)
            .finish()
    }
}

impl CustomDatSource {
    /// Registers a GitHub-hosted DAT (raw file, repository file or release
    /// asset). GitHub hosting records provenance only; it is not authority.
    pub fn github(url: &str, display_name: impl Into<String>) -> Result<Self> {
        let parsed = validate_registration_url(url)?;
        let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
        if !matches!(
            host.as_str(),
            "github.com" | "www.github.com" | "raw.githubusercontent.com"
        ) && !host.ends_with(".githubusercontent.com")
        {
            return Err(config("a GitHub source must use a GitHub host"));
        }
        Self::remote(url, display_name, CustomDatSourceKind::Github)
    }

    /// Registers a plain HTTPS DAT URL.
    pub fn https(url: &str, display_name: impl Into<String>) -> Result<Self> {
        validate_registration_url(url)?;
        Self::remote(url, display_name, CustomDatSourceKind::Https)
    }

    /// Registers a local DAT file. The file is read-only input and is never
    /// moved, modified or deleted by this module.
    pub fn local(path: &Path, display_name: impl Into<String>) -> Result<Self> {
        if path.as_os_str().is_empty() {
            return Err(config("DAT path is empty"));
        }
        Ok(Self {
            id: stable_id(&format!("local:{}", path.display())),
            display_name: bounded_name(display_name.into())?,
            kind: CustomDatSourceKind::Local,
            reference: ManagedSourceReference::LocalPath(path.to_path_buf()),
            trust: DatAcquisitionTrust::UserProvided,
            acquisition: None,
        })
    }

    fn remote(
        url: &str,
        display_name: impl Into<String>,
        kind: CustomDatSourceKind,
    ) -> Result<Self> {
        let acquisition = parse_source_url(url, DatAcquisitionTrust::UserProvided)?;
        Ok(Self {
            id: stable_id(url),
            display_name: bounded_name(display_name.into())?,
            kind,
            reference: ManagedSourceReference::RemoteUrl(url.to_string()),
            trust: DatAcquisitionTrust::UserProvided,
            acquisition: Some(acquisition),
        })
    }

    /// The address with credentials and query values removed, safe to show or log.
    pub fn display_address(&self) -> String {
        match &self.reference {
            ManagedSourceReference::RemoteUrl(url) => redact_url(url),
            ManagedSourceReference::LocalPath(path) => path.display().to_string(),
        }
    }

    /// Validates a deserialised source. A registry file is untrusted input.
    fn validate(&self) -> Result<()> {
        if self.trust != DatAcquisitionTrust::UserProvided {
            return Err(config("a custom DAT source can only be user-provided"));
        }
        bounded_name(self.display_name.clone())?;
        match (&self.kind, &self.reference) {
            (CustomDatSourceKind::Local, ManagedSourceReference::LocalPath(path)) => {
                if path.as_os_str().is_empty() {
                    return Err(config("DAT path is empty"));
                }
                if self.id != stable_id(&format!("local:{}", path.display())) {
                    return Err(config("custom DAT source ID does not match its path"));
                }
            }
            (
                CustomDatSourceKind::Github | CustomDatSourceKind::Https,
                ManagedSourceReference::RemoteUrl(url),
            ) => {
                validate_registration_url(url)?;
                if self.id != stable_id(url) {
                    return Err(config("custom DAT source ID does not match its URL"));
                }
            }
            _ => return Err(config("custom DAT source kind and reference do not match")),
        }
        Ok(())
    }

    fn provider_id(&self) -> String {
        format!("custom-dat-{}", self.id)
    }

    fn descriptor(&self) -> ManagedSourceDescriptor {
        ManagedSourceDescriptor {
            provider_id: self.provider_id(),
            display_name: self.display_name.clone(),
            source_kind: if self.kind == CustomDatSourceKind::Local {
                ManagedSourceKind::Local
            } else {
                ManagedSourceKind::Remote
            },
            source: self.reference.clone(),
            expected_media_type: "application/octet-stream".into(),
            maximum_size_bytes: CUSTOM_DAT_MAX_BYTES,
            attribution_url: match &self.reference {
                ManagedSourceReference::RemoteUrl(url) => Some(url.clone()),
                ManagedSourceReference::LocalPath(_) => None,
            },
            parser_schema_version: PARSER_SCHEMA.into(),
            // UserProvided is the only value a custom source can carry, so a
            // GitHub host can never reach `Official`.
            trust: ManagedSourceTrust::UserProvided,
        }
    }
}

/// Persisted registry of user-supplied sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomDatSourceRegistry {
    schema: u32,
    sources: Vec<CustomDatSource>,
}

impl Default for CustomDatSourceRegistry {
    fn default() -> Self {
        Self {
            schema: REGISTRY_SCHEMA,
            sources: Vec::new(),
        }
    }
}

impl CustomDatSourceRegistry {
    pub fn sources(&self) -> &[CustomDatSource] {
        &self.sources
    }

    pub fn get(&self, id: &str) -> Option<&CustomDatSource> {
        self.sources.iter().find(|source| source.id == id)
    }

    pub fn add(&mut self, source: CustomDatSource) -> Result<()> {
        source.validate()?;
        if self.sources.iter().any(|item| item.id == source.id) {
            return Err(config("that custom DAT source is already registered"));
        }
        if self.sources.len() >= CUSTOM_DAT_MAX_SOURCES {
            return Err(config("the custom DAT source limit has been reached"));
        }
        self.sources.push(source);
        self.sources.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(())
    }

    /// Removes the registration only. Snapshots already stored stay in their
    /// store, and a local DAT file is never touched.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.sources.len();
        self.sources.retain(|source| source.id != id);
        before != self.sources.len()
    }

    /// Loads the registry. A missing file is an empty registry; a corrupt or
    /// tampered file is refused rather than partially trusted.
    pub fn load(path: &Path) -> Result<Self> {
        let body = match fs::read_to_string(path) {
            Ok(body) => body,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(ArchiveFsError::io(path.to_path_buf(), error)),
        };
        if body.len() > 4 * 1024 * 1024 {
            return Err(config("custom DAT registry is too large"));
        }
        let registry: Self =
            serde_json::from_str(&body).map_err(|_| config("custom DAT registry is not valid"))?;
        if registry.schema != REGISTRY_SCHEMA {
            return Err(config("custom DAT registry has an unsupported schema"));
        }
        if registry.sources.len() > CUSTOM_DAT_MAX_SOURCES {
            return Err(config("custom DAT registry exceeds the source limit"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for source in &registry.sources {
            source.validate()?;
            if !seen.insert(source.id.clone()) {
                return Err(config("custom DAT registry contains a duplicate source"));
            }
        }
        Ok(registry)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let body = serde_json::to_string_pretty(self)
            .map_err(|error| ArchiveFsError::Config(error.to_string()))?;
        crate::atomic_write_text(path, &format!("{body}\n"))
    }
}

/// What a validated, staged DAT looks like before the person activates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomDatValidation {
    pub ecosystem: DatEcosystem,
    pub format: String,
    pub name: Option<String>,
    pub revision: Option<String>,
    pub entry_count: usize,
    pub warnings: Vec<String>,
    /// True when the DAT was extracted from a ZIP release asset.
    pub extracted_from_archive: bool,
}

/// A staged and validated snapshot, bound to the active snapshot that existed
/// when it was created so a later activation can be refused as stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomDatCandidate {
    pub candidate: ValidatedCandidate,
    pub validation: CustomDatValidation,
    pub source_id: String,
    pub trust: DatAcquisitionTrust,
    pub expected_active: Option<String>,
}

/// The explicit lifecycle for one registered source.
pub struct CustomDatLifecycle<'a> {
    source: &'a CustomDatSource,
    store: ManagedSourceStore,
    limits: DatLimits,
}

impl<'a> CustomDatLifecycle<'a> {
    /// `root` is the directory that holds this source's immutable snapshots.
    pub fn new(source: &'a CustomDatSource, root: &Path) -> Result<Self> {
        source.validate()?;
        Ok(Self {
            source,
            store: ManagedSourceStore::new(root.to_path_buf(), source.descriptor())?,
            limits: DatLimits::default(),
        })
    }

    pub fn store(&self) -> &ManagedSourceStore {
        &self.store
    }

    pub fn active(&self) -> Result<Option<ManagedSourceSnapshot>> {
        self.store.active_snapshot()
    }

    /// Metadata-only update check against the active snapshot's validators.
    /// Offline never touches the network. Nothing is downloaded or activated.
    pub fn check_update(&self, offline: bool) -> Result<UpdateCheck> {
        self.check_update_with(
            offline,
            &SystemResolver,
            &HttpsManagedSourceTransport::default(),
        )
    }

    pub fn check_update_with(
        &self,
        offline: bool,
        resolver: &impl HostResolver,
        transport: &dyn ManagedSourceTransport,
    ) -> Result<UpdateCheck> {
        if self.source.kind == CustomDatSourceKind::Local {
            return Err(config("a local source has no remote update to check"));
        }
        if !offline {
            self.require_public_destination(resolver)?;
        }
        self.store
            .check_for_update(offline, transport)
            .map_err(redact_error)
    }

    /// Downloads to staging and validates. The active snapshot is not touched,
    /// and a failure leaves no staged file behind.
    pub fn download(&self) -> Result<CustomDatCandidate> {
        self.download_with(&SystemResolver, &HttpsManagedSourceTransport::default())
    }

    pub fn download_with(
        &self,
        resolver: &impl HostResolver,
        transport: &dyn ManagedSourceTransport,
    ) -> Result<CustomDatCandidate> {
        if self.source.kind == CustomDatSourceKind::Local {
            return self.import_local();
        }
        self.require_public_destination(resolver)?;
        let expected_active = self.active_hash()?;
        let staged = self
            .store
            .fetch_candidate(transport)
            .map_err(redact_error)?;
        self.validate_staged(staged, expected_active)
    }

    /// Stages the registered local file (regular, non-symlink, size-bounded).
    pub fn import_local(&self) -> Result<CustomDatCandidate> {
        let expected_active = self.active_hash()?;
        let staged = self.store.stage_local(ManagedSourceMetadata::default())?;
        self.validate_staged(staged, expected_active)
    }

    /// Side-effect-free preview of what activation would change.
    pub fn preview(&self, candidate: &CustomDatCandidate) -> Result<ActivationPreview> {
        self.check_candidate_source(candidate)?;
        self.store.preview_activation(&candidate.candidate)
    }

    /// Activates explicitly. Refuses if the active snapshot changed since the
    /// candidate was created, so an old download can never replace newer state.
    pub fn activate(&self, candidate: &CustomDatCandidate) -> Result<ActivationResult> {
        self.check_candidate_source(candidate)?;
        self.store
            .activate_snapshot(&candidate.candidate, candidate.expected_active.as_deref())
    }

    /// Snapshots eligible for rollback, in the store's own history order.
    pub fn history(&self) -> Result<Vec<ManagedSourceSnapshot>> {
        self.store.history_snapshots()
    }

    pub fn rollback(&self, sha256: &str) -> Result<ActivationResult> {
        self.store.rollback_snapshot(sha256)
    }

    fn active_hash(&self) -> Result<Option<String>> {
        Ok(self
            .store
            .active_snapshot()?
            .map(|snapshot| snapshot.sha256))
    }

    fn check_candidate_source(&self, candidate: &CustomDatCandidate) -> Result<()> {
        if candidate.source_id != self.source.id {
            return Err(config("this candidate belongs to a different source"));
        }
        Ok(())
    }

    fn require_public_destination(&self, resolver: &impl HostResolver) -> Result<()> {
        let ManagedSourceReference::RemoteUrl(url) = &self.source.reference else {
            return Err(config("source is local, not remote"));
        };
        validate_public_https_url(url, resolver)
            .map_err(|refusal| config(format!("source refused: {}", refusal.detail())))
    }

    fn validate_staged(
        &self,
        staged: StagedCandidate,
        expected_active: Option<String>,
    ) -> Result<CustomDatCandidate> {
        match self.build_candidate(&staged, expected_active) {
            Ok(built) => Ok(built),
            Err(error) => {
                self.store.discard_staged(staged);
                Err(error)
            }
        }
    }

    fn build_candidate(
        &self,
        staged: &StagedCandidate,
        expected_active: Option<String>,
    ) -> Result<CustomDatCandidate> {
        let bytes = read_bounded(&staged.path)?;
        let extracted = bytes.starts_with(b"PK\x03\x04");
        let dat_bytes = if extracted {
            extract_dat_payload(&bytes, &ExtractLimits::default())?
        } else {
            bytes.clone()
        };
        let scratch = tempfile::NamedTempFile::new()
            .map_err(|error| ArchiveFsError::io(staged.path.clone(), error))?;
        fs::write(scratch.path(), &dat_bytes)
            .map_err(|error| ArchiveFsError::io(scratch.path().to_path_buf(), error))?;
        let parsed = parse_dat_file(scratch.path(), self.limits)
            .map_err(|error| ArchiveFsError::Config(format!("not a valid DAT: {error}")))?;
        if parsed.dat.source.entry_count == 0 {
            return Err(config("the DAT contains no entries"));
        }
        let validation = CustomDatValidation {
            ecosystem: parsed.dat.source.ecosystem,
            format: parsed.dat.source.format.label().to_string(),
            name: parsed.dat.source.name.clone(),
            revision: parsed.dat.source.version.clone(),
            entry_count: parsed.dat.source.entry_count,
            warnings: parsed.warnings.iter().map(ToString::to_string).collect(),
            extracted_from_archive: extracted,
        };
        // Publish the extracted DAT bytes, not the wrapper archive, so the
        // snapshot is exactly the data later consumers parse.
        let publish = if extracted {
            self.store
                .stage_bytes(&dat_bytes, staged.metadata.clone())?
        } else {
            staged.clone()
        };
        let report = ValidationReport {
            valid: true,
            summary: format!(
                "{} DAT with {} entries ({})",
                validation.format,
                validation.entry_count,
                match self.source.kind {
                    CustomDatSourceKind::Github => "user-provided GitHub source",
                    CustomDatSourceKind::Https => "user-provided HTTPS source",
                    CustomDatSourceKind::Local => "user-provided local file",
                }
            ),
            record_count: Some(validation.entry_count as u64),
            warnings: validation.warnings.clone(),
        };
        let candidate = self.store.validate_candidate(publish, report)?;
        if extracted {
            self.store.discard_staged(staged.clone());
        }
        Ok(CustomDatCandidate {
            candidate,
            validation,
            source_id: self.source.id.clone(),
            trust: self.source.trust,
            expected_active,
        })
    }
}

/// Bounds for extracting a DAT from a ZIP release asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractLimits {
    pub max_members: usize,
    pub max_member_bytes: u64,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self {
            max_members: CUSTOM_DAT_MAX_ZIP_MEMBERS,
            max_member_bytes: CUSTOM_DAT_MAX_MEMBER_BYTES,
        }
    }
}

/// Extracts the single `.dat`/`.xml` member of a ZIP, in memory and bounded.
/// Nothing is written to disk by name, so traversal names cannot escape; they
/// are refused anyway. The declared size is never trusted: reading is capped.
pub fn extract_dat_payload(bytes: &[u8], limits: &ExtractLimits) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| config("the DAT archive is not a valid ZIP"))?;
    if archive.len() > limits.max_members {
        return Err(config("the DAT archive contains too many members"));
    }
    let mut found: Option<Vec<u8>> = None;
    for index in 0..archive.len() {
        let mut member = archive
            .by_index(index)
            .map_err(|_| config("a DAT archive member could not be read"))?;
        let name = member.name().to_string();
        if name.starts_with('/')
            || name.starts_with('\\')
            || name.contains('\\')
            || name.split('/').any(|part| part == "..")
            || name.contains('\0')
        {
            return Err(config("the DAT archive contains an unsafe member name"));
        }
        let lower = name.to_ascii_lowercase();
        if !member.is_file() || !(lower.ends_with(".dat") || lower.ends_with(".xml")) {
            continue;
        }
        if member.size() > limits.max_member_bytes {
            return Err(config("a DAT archive member is too large"));
        }
        if found.is_some() {
            return Err(config("the DAT archive contains more than one DAT"));
        }
        let mut data = Vec::new();
        member
            .by_ref()
            .take(limits.max_member_bytes.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(|_| config("a DAT archive member could not be read"))?;
        if data.len() as u64 > limits.max_member_bytes {
            return Err(config("a DAT archive member is too large"));
        }
        found = Some(data);
    }
    found.ok_or_else(|| config("the DAT archive contains no DAT or XML file"))
}

/// Removes credentials and query values so an address is safe to show or log.
pub fn redact_url(value: &str) -> String {
    match Url::parse(value) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_fragment(None);
            if url.query().is_some() {
                url.set_query(Some("redacted"));
            }
            url.to_string()
        }
        Err(_) => "<unparseable address>".to_string(),
    }
}

/// Registration is offline-friendly: it validates shape and literal addresses
/// but does not resolve DNS. Resolution happens when a source is contacted.
fn validate_registration_url(value: &str) -> Result<Url> {
    if value.len() > MAX_URL_BYTES {
        return Err(config("the DAT URL is too long"));
    }
    let url = Url::parse(value).map_err(|_| config("the DAT URL is not valid"))?;
    if url.scheme() != "https" {
        return Err(config("custom DAT sources require HTTPS"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(config("a DAT URL must not contain credentials"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| config("the DAT URL has no host"))?
        .to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(config("loopback hosts are not allowed"));
    }
    if let Ok(address) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        let resolver = StaticResolver::new().with(&host, &[address]);
        validate_public_https_url(url.as_str(), &resolver)
            .map_err(|refusal| config(format!("source refused: {}", refusal.detail())))?;
    }
    Ok(url)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(config("staged DAT is not a regular file"));
    }
    if metadata.len() > CUSTOM_DAT_MAX_BYTES {
        return Err(config("staged DAT exceeds the size limit"));
    }
    let file =
        fs::File::open(path).map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
    let mut bytes = Vec::new();
    file.take(CUSTOM_DAT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ArchiveFsError::io(path.to_path_buf(), error))?;
    if bytes.len() as u64 > CUSTOM_DAT_MAX_BYTES {
        return Err(config("staged DAT exceeds the size limit"));
    }
    Ok(bytes)
}

fn stable_id(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn bounded_name(name: String) -> Result<String> {
    if name.trim().is_empty() || name.len() > MAX_NAME_BYTES || name.contains('\0') {
        return Err(config("the DAT source name is empty or too long"));
    }
    Ok(name)
}

fn config(message: impl Into<String>) -> ArchiveFsError {
    ArchiveFsError::Config(message.into())
}

/// Transport and store errors can carry the request URL. Re-render them with
/// anything URL-shaped redacted before they reach a caller.
fn redact_error(error: ArchiveFsError) -> ArchiveFsError {
    let text = error.to_string();
    let redacted: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            if word.starts_with("https://") || word.starts_with("http://") {
                redact_url(word.trim_matches(|c| matches!(c, '"' | '\'' | ',' | ')' | '(')))
            } else {
                word.to_string()
            }
        })
        .collect();
    ArchiveFsError::Config(redacted.join(" "))
}

#[cfg(test)]
mod tests;
