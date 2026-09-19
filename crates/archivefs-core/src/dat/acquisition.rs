//! Explicit user-selected network DAT acquisition.
//!
//! This adapter reuses the managed snapshot store for staging, hashing,
//! validation, activation, history, and rollback. Transport trust is kept
//! separate from DAT validity: a header claiming an ecosystem never promotes
//! a user URL to official authority.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::dat::limits::DatLimits;
use crate::dat::model::DatEcosystem;
use crate::dat::parsers::parse_dat_file;
use crate::identity_source::managed_snapshot::{
    ActivationPreview, ActivationResult, HttpsManagedSourceTransport, ManagedSourceDescriptor,
    ManagedSourceKind, ManagedSourceMetadata, ManagedSourceReference, ManagedSourceStore,
    ManagedSourceTrust, StagedCandidate, ValidatedCandidate, ValidationReport,
};
use crate::identity_source::net_policy::{HostResolver, SystemResolver, validate_public_https_url};
use crate::{ArchiveFsError, Result};

pub const DAT_ACQUISITION_SCHEMA: &str = "dat-acquisition-v1";
pub const MAX_REMOTE_DAT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatAcquisitionTrust {
    Official,
    ReviewedMirror,
    UserProvided,
    Unknown,
}

impl DatAcquisitionTrust {
    fn managed(self) -> ManagedSourceTrust {
        match self {
            Self::Official => ManagedSourceTrust::Official,
            Self::ReviewedMirror => ManagedSourceTrust::Community,
            Self::UserProvided => ManagedSourceTrust::UserProvided,
            Self::Unknown => ManagedSourceTrust::Unverified,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatAcquisitionTransport {
    GithubRaw,
    GithubReleaseAsset,
    CustomHttps,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatAcquisitionSource {
    pub url: String,
    pub transport: DatAcquisitionTransport,
    pub trust: DatAcquisitionTrust,
    pub host: String,
    pub repository: Option<String>,
    pub release_or_tag: Option<String>,
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatAcquisitionRecord {
    pub schema: String,
    pub source: DatAcquisitionSource,
    pub sha256: String,
    pub retrieved_at_unix_seconds: u64,
    pub ecosystem: DatEcosystem,
    pub internal_revision: Option<String>,
    pub header_name: Option<String>,
    pub valid: bool,
}

#[derive(Debug, Clone)]
pub struct DatAcquisitionStore {
    source: DatAcquisitionSource,
    store: ManagedSourceStore,
    provenance_root: PathBuf,
}

impl DatAcquisitionStore {
    pub fn new(root: PathBuf, source: DatAcquisitionSource) -> Result<Self> {
        Self::new_with_resolver(root, source, &SystemResolver)
    }

    pub fn new_with_resolver(
        root: PathBuf,
        source: DatAcquisitionSource,
        resolver: &impl HostResolver,
    ) -> Result<Self> {
        validate_source_url_with_resolver(&source.url, resolver)
            .map_err(|error| ArchiveFsError::Config(error.to_string()))?;
        let id = provider_id(&source.url);
        let descriptor = ManagedSourceDescriptor {
            provider_id: id,
            display_name: "User-selected DAT source".into(),
            source_kind: ManagedSourceKind::Remote,
            source: ManagedSourceReference::RemoteUrl(source.url.clone()),
            expected_media_type: "application/octet-stream".into(),
            maximum_size_bytes: MAX_REMOTE_DAT_BYTES,
            attribution_url: Some(source.url.clone()),
            parser_schema_version: DAT_ACQUISITION_SCHEMA.into(),
            trust: source.trust.managed(),
        };
        let provenance_root = root.join("provenance");
        Ok(Self {
            source,
            store: ManagedSourceStore::new(root.join("snapshots"), descriptor)?,
            provenance_root,
        })
    }

    pub fn source(&self) -> &DatAcquisitionSource {
        &self.source
    }

    pub fn store(&self) -> &ManagedSourceStore {
        &self.store
    }

    pub fn fetch_and_stage(&self) -> Result<ValidatedCandidate> {
        let staged = self
            .store
            .fetch_candidate(&HttpsManagedSourceTransport::default())?;
        self.validate_staged(staged)
    }

    pub fn stage_bytes(&self, bytes: &[u8]) -> Result<ValidatedCandidate> {
        let staged = self
            .store
            .stage_bytes(bytes, ManagedSourceMetadata::default())?;
        self.validate_staged(staged)
    }

    fn validate_staged(&self, staged: StagedCandidate) -> Result<ValidatedCandidate> {
        let parsed = parse_staged_dat(&staged.path)?;
        let record = DatAcquisitionRecord {
            schema: DAT_ACQUISITION_SCHEMA.into(),
            source: self.source.clone(),
            sha256: staged.sha256.clone(),
            retrieved_at_unix_seconds: now(),
            ecosystem: parsed.ecosystem,
            internal_revision: parsed.version.clone(),
            header_name: parsed.name.clone(),
            valid: true,
        };
        let candidate = self.store.validate_candidate(
            staged,
            ValidationReport {
                valid: parsed.entry_count > 0,
                summary: format!(
                    "{} DAT with {} entries",
                    parsed.ecosystem.label(),
                    parsed.entry_count
                ),
                record_count: Some(parsed.entry_count as u64),
                warnings: parsed.warnings,
            },
        )?;
        persist_record(&self.provenance_root, &record)?;
        Ok(candidate)
    }

    pub fn preview_activation(&self, candidate: &ValidatedCandidate) -> Result<ActivationPreview> {
        self.store.preview_activation(candidate)
    }

    pub fn activate(
        &self,
        candidate: &ValidatedCandidate,
        expected_active: Option<&str>,
    ) -> Result<ActivationResult> {
        self.store.activate_snapshot(candidate, expected_active)
    }

    pub fn rollback(&self, hash: &str) -> Result<ActivationResult> {
        self.store.rollback_snapshot(hash)
    }

    pub fn active_record(&self) -> Result<Option<DatAcquisitionRecord>> {
        let Some(snapshot) = self.store.active_snapshot()? else {
            return Ok(None);
        };
        let path = record_path(&self.provenance_root, &snapshot.sha256);
        let bytes = fs::read(&path).map_err(|error| ArchiveFsError::io(path, error))?;
        let record: DatAcquisitionRecord = serde_json::from_slice(&bytes)
            .map_err(|error| ArchiveFsError::Config(error.to_string()))?;
        if record.schema != DAT_ACQUISITION_SCHEMA
            || record.source != self.source
            || record.sha256 != snapshot.sha256
            || !record.valid
        {
            return Err(ArchiveFsError::Config(
                "DAT acquisition provenance is stale or does not match the active snapshot".into(),
            ));
        }
        Ok(Some(record))
    }
}

#[derive(Debug)]
struct ParsedDatSummary {
    ecosystem: DatEcosystem,
    entry_count: usize,
    version: Option<String>,
    name: Option<String>,
    warnings: Vec<String>,
}

fn parse_staged_dat(path: &Path) -> Result<ParsedDatSummary> {
    let parsed = parse_dat_file(path, DatLimits::default())
        .map_err(|error| ArchiveFsError::Config(error.to_string()))?;
    let crate::dat::parser::ParseOutcome { dat, warnings } = parsed;
    Ok(ParsedDatSummary {
        ecosystem: dat.source.ecosystem,
        entry_count: dat.source.entry_count,
        version: dat.source.version,
        name: dat.source.name,
        warnings: warnings
            .into_iter()
            .map(|warning| warning.message)
            .collect(),
    })
}

pub fn parse_source_url(url: &str, trust: DatAcquisitionTrust) -> Result<DatAcquisitionSource> {
    let parsed = Url::parse(url).map_err(|error| ArchiveFsError::Config(error.to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| ArchiveFsError::Config("source URL has no host".into()))?;
    if host == "github.com" || host == "raw.githubusercontent.com" {
        let parts: Vec<_> = parsed
            .path_segments()
            .map(|segments| segments.collect())
            .unwrap_or_else(Vec::new);
        if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(ArchiveFsError::Config(
                "GitHub DAT URL must identify a repository".into(),
            ));
        }
        let release_or_tag = parts
            .windows(2)
            .find_map(|pair| (pair[0] == "download").then(|| pair[1].to_string()));
        let transport = if parsed.path().contains("/releases/download/") {
            DatAcquisitionTransport::GithubReleaseAsset
        } else {
            DatAcquisitionTransport::GithubRaw
        };
        return Ok(DatAcquisitionSource {
            url: url.into(),
            transport,
            trust,
            host: host.into(),
            repository: Some(format!("{}/{}", parts[0], parts[1])),
            release_or_tag,
            commit: None,
        });
    }
    Ok(DatAcquisitionSource {
        url: url.into(),
        transport: DatAcquisitionTransport::CustomHttps,
        trust,
        host: host.into(),
        repository: None,
        release_or_tag: None,
        commit: None,
    })
}

pub fn validate_source_url_with_resolver(
    url: &str,
    resolver: &impl HostResolver,
) -> std::result::Result<(), crate::identity_source::net_policy::EndpointRefusal> {
    validate_public_https_url(url, resolver)
}

fn provider_id(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let suffix = digest
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("dat-source-{suffix}")
}

fn record_path(root: &Path, hash: &str) -> PathBuf {
    root.join(format!("{hash}.json"))
}

fn persist_record(root: &Path, record: &DatAcquisitionRecord) -> Result<()> {
    fs::create_dir_all(root).map_err(|error| ArchiveFsError::io(root.to_path_buf(), error))?;
    let path = record_path(root, &record.sha256);
    let body = serde_json::to_string_pretty(record)
        .map_err(|error| ArchiveFsError::Config(error.to_string()))?;
    crate::atomic_write_text(&path, &format!("{body}\n"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    struct Resolver(Vec<IpAddr>);
    impl HostResolver for Resolver {
        fn resolve(&self, _host: &str, _port: u16) -> std::result::Result<Vec<IpAddr>, String> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn github_provenance_is_explicit_and_not_official_by_default() {
        let release = parse_source_url(
            "https://github.com/acme/catalogue/releases/download/v1/all.dat",
            DatAcquisitionTrust::UserProvided,
        )
        .unwrap();
        assert_eq!(
            release.transport,
            DatAcquisitionTransport::GithubReleaseAsset
        );
        assert_eq!(release.repository.as_deref(), Some("acme/catalogue"));
        assert_eq!(release.release_or_tag.as_deref(), Some("v1"));
        assert_eq!(release.trust, DatAcquisitionTrust::UserProvided);
    }

    #[test]
    fn public_source_policy_rejects_credentials_and_private_destinations() {
        let public = Resolver(vec!["8.8.8.8".parse().unwrap()]);
        assert!(
            validate_source_url_with_resolver("https://user:password@example.test/a.dat", &public)
                .is_err()
        );
        assert!(
            validate_source_url_with_resolver(
                "https://example.test/a.dat",
                &Resolver(vec!["127.0.0.1".parse().unwrap()])
            )
            .is_err()
        );
        assert!(validate_source_url_with_resolver("http://example.test/a.dat", &public).is_err());
    }

    #[test]
    fn valid_bytes_persist_hash_provenance_and_reload_after_restart() {
        let root = tempfile::tempdir().unwrap();
        let source = parse_source_url(
            "https://raw.githubusercontent.com/acme/catalogue/abc123/catalogue.dat",
            DatAcquisitionTrust::UserProvided,
        )
        .unwrap();
        let store = DatAcquisitionStore::new_with_resolver(
            root.path().join("acquisition"),
            source.clone(),
            &Resolver(vec!["8.8.8.8".parse().unwrap()]),
        )
        .unwrap();
        let bytes = br#"<?xml version="1.0"?><datafile><header><name>Acme</name><version>7</version></header><game name="One"><rom name="one.bin" size="1" sha1="86f7e437fa060d3f29a2f2c2e1f7b8f7d3b2e0d4"/></game></datafile>"#;
        let candidate = store.stage_bytes(bytes).unwrap();
        store.activate(&candidate, None).unwrap();
        let record = store.active_record().unwrap().unwrap();
        assert_eq!(record.source, source);
        assert_eq!(record.internal_revision.as_deref(), Some("7"));
        assert_eq!(record.sha256, candidate.snapshot.sha256);
        assert_eq!(record.source.trust, DatAcquisitionTrust::UserProvided);
        let reopened = DatAcquisitionStore::new_with_resolver(
            root.path().join("acquisition"),
            record.source.clone(),
            &Resolver(vec!["8.8.8.8".parse().unwrap()]),
        )
        .unwrap();
        assert_eq!(reopened.active_record().unwrap(), Some(record));
        assert_eq!(
            reopened.store().active_snapshot_bytes().unwrap(),
            Some(bytes.to_vec())
        );
    }
}
