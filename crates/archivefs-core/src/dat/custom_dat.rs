//! Explicitly user-supplied DAT snapshots.
//!
//! A valid DAT is useful evidence, but its syntax and header do not establish
//! upstream authority. This adapter stores a user-selected DAT through the
//! existing immutable managed-source lifecycle and keeps that trust boundary
//! visible to callers.

use crate::dat::limits::DatLimits;
use crate::dat::model::{DatEcosystem, ParsedDat};
use crate::dat::parsers::parse_dat_file;
use crate::identity_source::managed_snapshot::{
    ActivationPreview, ActivationResult, ManagedSourceDescriptor, ManagedSourceKind,
    ManagedSourceMetadata, ManagedSourceReference, ManagedSourceSnapshot, ManagedSourceStore,
    ManagedSourceTrust, ValidatedCandidate, ValidationReport,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub type CustomDatResult<T> = Result<T, String>;
const PARSER_SCHEMA_VERSION: &str = "custom-dat-v1";
const MAX_CUSTOM_DAT_BYTES: u64 = 256 * 1024 * 1024;

/// Trust is an explicit interpretation of a user DAT, never a claim about its
/// publisher. Official evidence is represented for callers that compare
/// sources, but this module can never produce it from a local import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatEvidenceTrust {
    OfficialAuthoritative,
    RecognizedEcosystem,
    UserSuppliedTrusted,
    UserSuppliedUntrusted,
    Unknown,
}

impl DatEvidenceTrust {
    pub fn label(self) -> &'static str {
        match self {
            Self::OfficialAuthoritative => "Official authoritative",
            Self::RecognizedEcosystem => "Recognized ecosystem",
            Self::UserSuppliedTrusted => "User-supplied trusted",
            Self::UserSuppliedUntrusted => "User-supplied untrusted",
            Self::Unknown => "Unknown",
        }
    }
}

/// The authority assigned to a local import. A header can identify an
/// ecosystem, but it cannot promote the bytes to official authority.
pub fn import_trust(ecosystem: DatEcosystem, user_trusted: bool) -> DatEvidenceTrust {
    if user_trusted {
        return DatEvidenceTrust::UserSuppliedTrusted;
    }
    if matches!(
        ecosystem,
        DatEcosystem::GenericLogiqx | DatEcosystem::GenericClrMamePro
    ) {
        DatEvidenceTrust::UserSuppliedUntrusted
    } else {
        DatEvidenceTrust::RecognizedEcosystem
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomDatEnvelope {
    pub imported_filename: String,
    pub source_sha256: String,
    pub format: crate::dat::model::DatFormat,
    pub ecosystem: DatEcosystem,
    pub trust: DatEvidenceTrust,
    pub header_name: Option<String>,
    pub header_description: Option<String>,
    pub header_version: Option<String>,
    pub header_author: Option<String>,
    pub parsed: ParsedDat,
    /// The original DAT bytes are retained so the managed object is a
    /// self-contained offline snapshot, independent of the source path.
    pub raw_dat: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomDatVerificationDecision {
    CustomEvidenceOnly,
    PreserveStrongerOfficialEvidence,
    SurfaceConflict,
}

/// A local DAT never wins over stronger official evidence. A disagreement is
/// returned to the caller for presentation rather than silently resolved.
pub fn resolve_against_official(
    custom: &CustomDatEnvelope,
    official_source: &str,
    hashes_conflict: bool,
) -> CustomDatVerificationDecision {
    let _ = (custom, official_source);
    if hashes_conflict {
        CustomDatVerificationDecision::SurfaceConflict
    } else {
        CustomDatVerificationDecision::PreserveStrongerOfficialEvidence
    }
}

#[derive(Debug, Clone)]
pub struct CustomDatStore {
    source_path: PathBuf,
    store: ManagedSourceStore,
}

impl CustomDatStore {
    pub fn new(root: PathBuf, source_path: PathBuf) -> CustomDatResult<Self> {
        if source_path.as_os_str().is_empty() {
            return Err("custom DAT source path is empty".into());
        }
        let descriptor = ManagedSourceDescriptor {
            provider_id: "custom-dat".into(),
            display_name: "User-supplied DAT snapshot".into(),
            source_kind: ManagedSourceKind::Local,
            source: ManagedSourceReference::LocalPath(source_path.clone()),
            expected_media_type: "application/vnd.emuwiz.custom-dat+json".into(),
            maximum_size_bytes: MAX_CUSTOM_DAT_BYTES,
            attribution_url: None,
            parser_schema_version: PARSER_SCHEMA_VERSION.into(),
            trust: ManagedSourceTrust::UserProvided,
        };
        Ok(Self {
            source_path,
            store: ManagedSourceStore::new(root, descriptor).map_err(|error| error.to_string())?,
        })
    }

    pub fn store(&self) -> &ManagedSourceStore {
        &self.store
    }

    pub fn stage_path(
        &self,
        path: &Path,
        user_trusted: bool,
    ) -> CustomDatResult<ValidatedCandidate> {
        let bytes = read_source(path)?;
        self.stage_bytes(path, &bytes, user_trusted)
    }

    pub fn stage_source(&self, user_trusted: bool) -> CustomDatResult<ValidatedCandidate> {
        self.stage_path(&self.source_path, user_trusted)
    }

    pub fn stage_bytes(
        &self,
        source_path: &Path,
        bytes: &[u8],
        user_trusted: bool,
    ) -> CustomDatResult<ValidatedCandidate> {
        if bytes.len() as u64 > MAX_CUSTOM_DAT_BYTES {
            return Err("custom DAT exceeds the bounded snapshot size".into());
        }
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let input = temp.path().join("import.dat");
        let mut file = File::create(&input).map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        let parsed =
            parse_dat_file(&input, DatLimits::default()).map_err(|error| error.to_string())?;
        let crate::dat::parser::ParseOutcome { mut dat, warnings } = parsed;
        if dat.source.entry_count == 0 {
            return Err("custom DAT contains no catalogue entries".into());
        }
        let imported_filename = source_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "imported.dat".into());
        dat.source.file_path = imported_filename.clone();
        let source_sha256 = sha256(bytes);
        let envelope = CustomDatEnvelope {
            imported_filename,
            source_sha256,
            format: dat.source.format,
            ecosystem: dat.source.ecosystem,
            trust: import_trust(dat.source.ecosystem, user_trusted),
            header_name: dat.source.name.clone(),
            header_description: dat.source.description.clone(),
            header_version: dat.source.version.clone(),
            header_author: dat.source.author.clone(),
            parsed: dat,
            raw_dat: bytes.to_vec(),
        };
        let payload = serde_json::to_vec(&envelope).map_err(|error| error.to_string())?;
        let candidate = self
            .store
            .stage_bytes(
                &payload,
                ManagedSourceMetadata {
                    provider_version: Some(
                        envelope
                            .header_version
                            .clone()
                            .unwrap_or_else(|| "unversioned".into()),
                    ),
                    ..ManagedSourceMetadata::default()
                },
            )
            .map_err(|error| error.to_string())?;
        self.store
            .validate_candidate(
                candidate,
                ValidationReport {
                    valid: true,
                    summary: format!(
                        "User-supplied {} DAT with {} entries",
                        envelope.ecosystem.label(),
                        envelope.parsed.games.len()
                    ),
                    record_count: Some(envelope.parsed.games.len() as u64),
                    warnings: warnings
                        .into_iter()
                        .map(|warning| warning.message)
                        .collect(),
                },
            )
            .map_err(|error| error.to_string())
    }

    pub fn preview_activation(
        &self,
        candidate: &ValidatedCandidate,
    ) -> CustomDatResult<ActivationPreview> {
        self.store
            .preview_activation(candidate)
            .map_err(|error| error.to_string())
    }

    pub fn activate_snapshot(
        &self,
        candidate: &ValidatedCandidate,
        expected_active: Option<&str>,
    ) -> CustomDatResult<ActivationResult> {
        self.store
            .activate_snapshot(candidate, expected_active)
            .map_err(|error| error.to_string())
    }

    pub fn rollback_snapshot(&self, hash: &str) -> CustomDatResult<ActivationResult> {
        self.store
            .rollback_snapshot(hash)
            .map_err(|error| error.to_string())
    }

    pub fn active(&self) -> CustomDatResult<Option<CustomDatEnvelope>> {
        let Some(bytes) = self
            .store
            .active_snapshot_bytes()
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let envelope: CustomDatEnvelope =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if envelope.source_sha256 != sha256(&envelope.raw_dat) {
            return Err("custom DAT snapshot source hash does not match retained bytes".into());
        }
        Ok(Some(envelope))
    }

    pub fn active_managed_snapshot(&self) -> CustomDatResult<Option<ManagedSourceSnapshot>> {
        self.store
            .active_snapshot()
            .map_err(|error| error.to_string())
    }
}

fn read_source(path: &Path) -> CustomDatResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("custom DAT source must be a regular non-symlink file".into());
    }
    if metadata.len() > MAX_CUSTOM_DAT_BYTES {
        return Err("custom DAT exceeds the bounded snapshot size".into());
    }
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dat() -> &'static [u8] {
        br#"<?xml version="1.0"?><datafile><header><name>Custom</name><version>1</version><author>User</author></header><game name="Example"><rom name="example.bin" size="1" sha1="86f7e437fa060d3f29a2f2c2e1f7b8f7d3b2e0d4"/></game></datafile>"#
    }

    fn store(root: &Path, source: &Path) -> CustomDatStore {
        CustomDatStore::new(root.to_path_buf(), source.to_path_buf()).unwrap()
    }

    #[test]
    fn valid_import_retains_provenance_and_is_not_official() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("custom.dat");
        fs::write(&source, dat()).unwrap();
        let original = fs::read(&source).unwrap();
        let managed = store(&dir.path().join("managed"), &source);
        let candidate = managed.stage_source(false).unwrap();
        assert!(candidate.snapshot.sha256.len() == 64);
        managed.activate_snapshot(&candidate, None).unwrap();
        let active = managed.active().unwrap().unwrap();
        assert_eq!(active.imported_filename, "custom.dat");
        assert_eq!(active.trust, DatEvidenceTrust::UserSuppliedUntrusted);
        assert_eq!(active.source_sha256, sha256(dat()));
        assert_eq!(
            managed.active_managed_snapshot().unwrap().unwrap().trust,
            ManagedSourceTrust::UserProvided
        );
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn user_trusted_is_distinct_and_header_never_grants_officiality() {
        assert_eq!(
            import_trust(DatEcosystem::NoIntro, true),
            DatEvidenceTrust::UserSuppliedTrusted
        );
        assert_eq!(
            import_trust(DatEcosystem::NoIntro, false),
            DatEvidenceTrust::RecognizedEcosystem
        );
        assert_ne!(
            import_trust(DatEcosystem::NoIntro, true),
            DatEvidenceTrust::OfficialAuthoritative
        );
    }

    #[test]
    fn conflict_preserves_stronger_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("custom.dat");
        fs::write(&source, dat()).unwrap();
        let managed = store(&dir.path().join("managed"), &source);
        let candidate = managed.stage_source(true).unwrap();
        managed.activate_snapshot(&candidate, None).unwrap();
        let active = managed.active().unwrap().unwrap();
        assert_eq!(
            resolve_against_official(&active, "official:redump", true),
            CustomDatVerificationDecision::SurfaceConflict
        );
        assert_eq!(
            resolve_against_official(&active, "official:mame", false),
            CustomDatVerificationDecision::PreserveStrongerOfficialEvidence
        );
    }

    #[test]
    fn staging_is_explicit_and_rollback_is_offline() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("custom.dat");
        fs::write(&source, dat()).unwrap();
        let managed = store(&dir.path().join("managed"), &source);
        let first = managed.stage_source(false).unwrap();
        assert!(managed.active().unwrap().is_none());
        managed.activate_snapshot(&first, None).unwrap();
        let second_bytes = [dat(), b" "].concat();
        let second = managed.stage_bytes(&source, &second_bytes, false).unwrap();
        assert_eq!(
            managed.active().unwrap().unwrap().source_sha256,
            sha256(dat())
        );
        managed
            .activate_snapshot(&second, Some(&first.snapshot.sha256))
            .unwrap();
        let rolled = managed.rollback_snapshot(&first.snapshot.sha256).unwrap();
        assert_eq!(rolled.active.sha256, first.snapshot.sha256);
        assert!(managed.active().unwrap().is_some());
    }

    #[test]
    fn malformed_import_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bad.dat");
        fs::write(&source, b"not a DAT").unwrap();
        let managed = store(&dir.path().join("managed"), &source);
        assert!(managed.stage_source(false).is_err());
        assert!(managed.active().unwrap().is_none());
    }

    #[test]
    fn identical_imports_have_one_content_identity() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("custom.dat");
        fs::write(&source, dat()).unwrap();
        let managed = store(&dir.path().join("managed"), &source);
        let first = managed.stage_source(false).unwrap();
        let second = managed.stage_source(false).unwrap();
        assert_eq!(first.snapshot.sha256, second.snapshot.sha256);
    }
}
