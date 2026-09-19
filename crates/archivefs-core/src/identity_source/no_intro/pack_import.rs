//! Transactional, browser-assisted No-Intro pack import.
//!
//! DAT-o-MATIC is intentionally transport-free here: the user supplies a ZIP
//! downloaded through the official site. This module validates the ZIP and
//! its DAT contents, then publishes one content-addressed snapshot. It never
//! performs HTTP, scrapes a page, or interprets a filename as authority.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use super::import::{ImportedNoIntroSource, NoIntroImportError, import_no_intro_dat};
use crate::identity_source::managed_snapshot::{
    ManagedSourceDescriptor, ManagedSourceKind, ManagedSourceMetadata, ManagedSourceReference,
    ManagedSourceStore, ManagedSourceTrust, ValidationReport,
};

/// Official download page for the user/browser handoff. This is metadata
/// only; no request is made by this module.
pub const NO_INTRO_DATOMATIC_DOWNLOAD_PAGE: &str = "https://datomatic.no-intro.org/";
pub const NO_INTRO_PACK_SCHEMA_VERSION: u32 = 1;
pub const NO_INTRO_PACK_MAX_BYTES: u64 = 512 * 1024 * 1024;
pub const NO_INTRO_PACK_MAX_MEMBERS: usize = 10_000;
pub const NO_INTRO_PACK_MAX_MEMBER_NAME_BYTES: usize = 4 * 1024;
pub const NO_INTRO_PACK_MAX_DAT_BYTES: u64 = 64 * 1024 * 1024;
/// Aggregate decompressed DAT budget.  The official 2026 packs observed in
/// live QA are about 460--466 MiB; 640 MiB leaves bounded headroom while the
/// compressed archive and per-member limits remain unchanged.
pub const NO_INTRO_PACK_MAX_TOTAL_DAT_BYTES: u64 = 640 * 1024 * 1024;
const PACK_DIRECTORY: &str = "no_intro_pack";
const MANAGED_STORE_DIRECTORY: &str = "managed";
const MANAGED_PROVIDER_ID: &str = "no-intro-pack";
const MANAGED_MEDIA_TYPE: &str = "application/vnd.emuwiz.no-intro-snapshot-state";
const MANAGED_PARSER_SCHEMA: &str = "no-intro-managed-snapshot-v2";

#[derive(Debug)]
pub enum NoIntroPackImportError {
    Io {
        path: PathBuf,
        error: io::Error,
    },
    InvalidArchive {
        detail: String,
    },
    LimitExceeded {
        detail: String,
    },
    Traversal {
        member: String,
    },
    CorruptDat {
        member: String,
        error: NoIntroImportError,
    },
    IncompleteDat {
        member: String,
        detail: String,
    },
    State(String),
}

impl fmt::Display for NoIntroPackImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::InvalidArchive { detail } => write!(f, "invalid No-Intro pack ZIP: {detail}"),
            Self::LimitExceeded { detail } => write!(f, "No-Intro pack limit exceeded: {detail}"),
            Self::Traversal { member } => write!(f, "unsafe ZIP member path: {member}"),
            Self::CorruptDat { member, error } => {
                write!(
                    f,
                    "No-Intro DAT member {member} could not be validated: {error}"
                )
            }
            Self::IncompleteDat { member, detail } => {
                write!(f, "No-Intro DAT member {member} is incomplete: {detail}")
            }
            Self::State(detail) => write!(f, "No-Intro pack state error: {detail}"),
        }
    }
}

impl std::error::Error for NoIntroPackImportError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoIntroPackImportStatus {
    Unchanged,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedNoIntroPackMember {
    pub member: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct NoIntroPackImportReport {
    pub status: NoIntroPackImportStatus,
    pub pack_sha256: String,
    pub snapshot_path: PathBuf,
    pub accepted: Vec<ImportedNoIntroSource>,
    pub rejected: Vec<RejectedNoIntroPackMember>,
}

/// The result of activating a staged pack.  Activation always invalidates
/// previously calculated verification results; callers must run verification
/// again before presenting the new source as checked.
#[derive(Debug, Clone)]
pub struct NoIntroPackActivationReport {
    pub import: NoIntroPackImportReport,
    pub verification: crate::identity_source::managed_snapshot::VerificationFreshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoIntroPackComparison {
    NoActiveSnapshot,
    SameSnapshot,
    DifferentSnapshot,
}

/// The non-mutating result of validating a user-supplied pack.  This contains
/// only metadata; the staged DAT files are removed before this is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoIntroPackInspection {
    pub pack_sha256: String,
    pub accepted: Vec<NoIntroPackMemberInspection>,
    pub rejected: Vec<RejectedNoIntroPackMember>,
    pub classification: NoIntroPackClassification,
}

/// Metadata for the atomically published pack, suitable for a GUI status
/// card without exposing the stored DAT contents.
pub type NoIntroPackInstalledSummary = NoIntroPackInspection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoIntroPackMemberInspection {
    pub member: String,
    pub system_name: String,
    pub variant: super::import::NoIntroVariant,
    pub upstream_version: Option<String>,
    pub artifact_sha256: String,
    pub entry_count: usize,
    pub rom_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoIntroPackClassification {
    Standard,
    Aftermarket,
    Bios,
    Mixed,
    Unknown,
}

impl NoIntroPackClassification {
    pub fn from_variants(
        variants: impl IntoIterator<Item = super::import::NoIntroVariant>,
    ) -> Self {
        let mut standard = false;
        let mut aftermarket = false;
        let mut bios = false;
        let mut unknown = false;
        for variant in variants {
            match variant {
                super::import::NoIntroVariant::Headered
                | super::import::NoIntroVariant::Headerless => standard = true,
                super::import::NoIntroVariant::Aftermarket => aftermarket = true,
                super::import::NoIntroVariant::Bios => bios = true,
                super::import::NoIntroVariant::Unknown => unknown = true,
            }
        }
        let families = standard as u8 + aftermarket as u8 + bios as u8 + unknown as u8;
        match (families, standard, aftermarket, bios, unknown) {
            (0, _, _, _, _) => Self::Unknown,
            (1, true, false, false, false) => Self::Standard,
            (1, false, true, false, false) => Self::Aftermarket,
            (1, false, false, true, false) => Self::Bios,
            (1, false, false, false, true) => Self::Unknown,
            _ => Self::Mixed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoIntroPackState {
    schema_version: u32,
    pack_sha256: String,
    #[serde(default)]
    pack_sha256s: Vec<String>,
    #[serde(default)]
    snapshot_sha256: Option<String>,
    accepted_members: Vec<NoIntroPackStateMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoIntroPackStagedState {
    schema_version: u32,
    pack_sha256: String,
    snapshot_sha256: String,
    accepted_members: Vec<NoIntroPackStateMember>,
    rejected: Vec<RejectedNoIntroPackMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoIntroPackStateMember {
    member: String,
    artifact_sha256: String,
    system_name: String,
    variant: super::import::NoIntroVariant,
    upstream_version: Option<String>,
    #[serde(default)]
    entry_count: usize,
    #[serde(default)]
    rom_count: usize,
}

fn managed_store(storage_root: &Path) -> Result<ManagedSourceStore, NoIntroPackImportError> {
    let descriptor = ManagedSourceDescriptor {
        provider_id: MANAGED_PROVIDER_ID.to_string(),
        display_name: "No-Intro managed pack lifecycle".to_string(),
        source_kind: ManagedSourceKind::Local,
        source: ManagedSourceReference::LocalPath(PathBuf::from("/emuwiz/no-intro-pack")),
        expected_media_type: MANAGED_MEDIA_TYPE.to_string(),
        maximum_size_bytes: NO_INTRO_PACK_MAX_BYTES,
        attribution_url: None,
        parser_schema_version: MANAGED_PARSER_SCHEMA.to_string(),
        trust: ManagedSourceTrust::Official,
    };
    ManagedSourceStore::new(storage_root.join(MANAGED_STORE_DIRECTORY), descriptor)
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))
}

fn encode_managed_state(state: &NoIntroPackState) -> Result<Vec<u8>, NoIntroPackImportError> {
    serde_json::to_vec(state).map_err(|error| NoIntroPackImportError::State(error.to_string()))
}

fn decode_managed_state(bytes: &[u8]) -> Result<NoIntroPackState, NoIntroPackImportError> {
    serde_json::from_slice(bytes).map_err(|error| NoIntroPackImportError::State(error.to_string()))
}

fn active_managed_state(
    storage_root: &Path,
) -> Result<Option<(NoIntroPackState, String)>, NoIntroPackImportError> {
    let store = managed_store(storage_root)?;
    let Some(snapshot) = store
        .active_snapshot()
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?
    else {
        return Ok(None);
    };
    let bytes = store
        .snapshot_bytes(&snapshot)
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?;
    Ok(Some((decode_managed_state(&bytes)?, snapshot.sha256)))
}

fn activate_managed_state(
    storage_root: &Path,
    state: &NoIntroPackState,
    expected_active: Option<&str>,
) -> Result<String, NoIntroPackImportError> {
    let store = managed_store(storage_root)?;
    let bytes = encode_managed_state(state)?;
    let staged = store
        .stage_bytes(&bytes, ManagedSourceMetadata::default())
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?;
    let candidate = store
        .validate_candidate(
            staged,
            ValidationReport {
                valid: true,
                summary: "validated No-Intro managed snapshot state".to_string(),
                record_count: Some(state.accepted_members.len() as u64),
                warnings: Vec::new(),
            },
        )
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?;
    let result = store
        .activate_snapshot(&candidate, expected_active)
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?;
    Ok(result.active.sha256)
}

#[derive(Debug, Clone, Copy)]
struct PackLimits;

impl PackLimits {
    fn validate_archive(&self, path: &Path, bytes: u64) -> Result<(), NoIntroPackImportError> {
        if bytes > NO_INTRO_PACK_MAX_BYTES {
            return Err(NoIntroPackImportError::LimitExceeded {
                detail: format!("ZIP is {bytes} bytes; maximum is {NO_INTRO_PACK_MAX_BYTES}"),
            });
        }
        if !path.is_absolute() {
            return Err(NoIntroPackImportError::InvalidArchive {
                detail: "pack path must be absolute".to_string(),
            });
        }
        Ok(())
    }
}

/// Imports a user-provided No-Intro ZIP into the production app-data area.
pub fn import_no_intro_pack(
    path: &Path,
) -> Result<NoIntroPackImportReport, NoIntroPackImportError> {
    let root = crate::app_dirs::data_path(PACK_DIRECTORY).map_err(|error| {
        NoIntroPackImportError::State(format!("cannot resolve application data path: {error}"))
    })?;
    import_no_intro_pack_at(path, &root)
}

/// Browser-assisted import seam: validate and retain a candidate, but leave
/// the active source untouched until explicit activation.
pub fn stage_no_intro_pack(path: &Path) -> Result<NoIntroPackImportReport, NoIntroPackImportError> {
    let root = crate::app_dirs::data_path(PACK_DIRECTORY).map_err(|error| {
        NoIntroPackImportError::State(format!("cannot resolve application data path: {error}"))
    })?;
    stage_no_intro_pack_at(path, &root)
}

/// Testable/local-pack seam. `storage_root` is the complete app-owned store
/// for this source and may point at a temporary directory in tests.
pub fn import_no_intro_pack_at(
    path: &Path,
    storage_root: &Path,
) -> Result<NoIntroPackImportReport, NoIntroPackImportError> {
    publish_no_intro_pack_at(path, storage_root, true)
}

/// Validates and publishes a content-addressed candidate without changing
/// the active No-Intro source.  The candidate is retained in a separate
/// staged record until [`activate_staged_no_intro_pack_at`] is called.
pub fn stage_no_intro_pack_at(
    path: &Path,
    storage_root: &Path,
) -> Result<NoIntroPackImportReport, NoIntroPackImportError> {
    publish_no_intro_pack_at(path, storage_root, false)
}

fn publish_no_intro_pack_at(
    path: &Path,
    storage_root: &Path,
    activate: bool,
) -> Result<NoIntroPackImportReport, NoIntroPackImportError> {
    let pack_sha256 = validate_pack(path)?;

    ensure_directory(storage_root)?;
    let state_path = storage_root.join("state.json");
    let managed_active = active_managed_state(storage_root)?;
    let active_state = managed_active
        .as_ref()
        .map(|(state, _)| state.clone())
        .or_else(|| load_state(&state_path).ok());
    if let Some(state) = active_state.as_ref()
        && state.schema_version == NO_INTRO_PACK_SCHEMA_VERSION
        && (state.pack_sha256 == pack_sha256
            || state.pack_sha256s.iter().any(|sha| sha == &pack_sha256))
    {
        let snapshot_path = storage_root.join("snapshots").join(
            state
                .snapshot_sha256
                .as_deref()
                .unwrap_or(&state.pack_sha256),
        );
        if snapshot_is_complete(&snapshot_path, &state.accepted_members) {
            let accepted = load_sources(&snapshot_path, &state.accepted_members)?;
            let report = NoIntroPackImportReport {
                status: NoIntroPackImportStatus::Unchanged,
                pack_sha256,
                snapshot_path,
                accepted,
                rejected: Vec::new(),
            };
            if !activate {
                write_staged_state(storage_root, &report)?;
            }
            return Ok(report);
        }
    }

    let staging = storage_root.join(format!(".staging-{}-{}", std::process::id(), unique_id()));
    let result = build_staged_snapshot(path, &staging);
    let (new_members, rejected) = match result {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };

    let mut accepted_members = new_members;
    let mut pack_sha256s = vec![pack_sha256.clone()];
    if let Some(previous) = active_state.as_ref()
        && previous.schema_version == NO_INTRO_PACK_SCHEMA_VERSION
    {
        let previous_snapshot = storage_root.join("snapshots").join(
            previous
                .snapshot_sha256
                .as_deref()
                .unwrap_or(&previous.pack_sha256),
        );
        if snapshot_is_complete(&previous_snapshot, &previous.accepted_members) {
            let old_members = &previous.accepted_members;
            for (index, member) in old_members.iter().enumerate() {
                if accepted_members
                    .iter()
                    .any(|candidate| candidate.artifact_sha256 == member.artifact_sha256)
                {
                    continue;
                }
                let source = previous_snapshot.join("dats").join(format!("{index}.dat"));
                let target_index = accepted_members.len();
                fs::copy(
                    &source,
                    staging.join("dats").join(format!("{target_index}.dat")),
                )
                .map_err(|error| io_error(&source, error))?;
                accepted_members.push(member.clone());
            }
            pack_sha256s.extend(previous.pack_sha256s.clone());
            pack_sha256s.push(previous.pack_sha256.clone());
            pack_sha256s.sort();
            pack_sha256s.dedup();
        }
    }
    let snapshot_id = snapshot_id(&accepted_members);
    let snapshot_path = storage_root.join("snapshots").join(&snapshot_id);
    fs::create_dir_all(snapshot_path.parent().unwrap())
        .map_err(|error| io_error(&snapshot_path, error))?;
    if snapshot_path.exists() {
        if !snapshot_matches_members(&snapshot_path, &accepted_members) {
            let _ = fs::remove_dir_all(&staging);
            return Err(NoIntroPackImportError::State(
                "content-addressed snapshot directory already exists but does not match the \
                 imported pack"
                    .to_string(),
            ));
        }
        let _ = fs::remove_dir_all(&staging);
    } else if let Err(error) = fs::rename(&staging, &snapshot_path) {
        let _ = fs::remove_dir_all(&staging);
        return Err(io_error(&snapshot_path, error));
    }

    let state = NoIntroPackState {
        schema_version: NO_INTRO_PACK_SCHEMA_VERSION,
        pack_sha256: pack_sha256.clone(),
        pack_sha256s: pack_sha256s.clone(),
        snapshot_sha256: Some(snapshot_id.clone()),
        accepted_members: accepted_members.clone(),
    };
    let accepted = load_sources(&snapshot_path, &state.accepted_members)?;
    let report = NoIntroPackImportReport {
        status: NoIntroPackImportStatus::Updated,
        pack_sha256: pack_sha256.clone(),
        snapshot_path,
        accepted,
        rejected: rejected.clone(),
    };
    if activate {
        let expected_active = managed_active.as_ref().map(|(_, hash)| hash.as_str());
        activate_managed_state(storage_root, &state, expected_active)?;
        // Keep the pre-managed pointer as a read-only migration mirror for
        // older installations. New reads prefer the provider-neutral store.
        write_active_state(storage_root, &state)?;
        prune_old_snapshots(storage_root, &snapshot_id);
        let _ = super::managed_lifecycle::register_no_intro_pack_at(
            storage_root,
            &pack_sha256,
            &snapshot_id,
            &report.accepted,
            &rejected,
        )?;
    } else {
        write_staged_state(storage_root, &report)?;
    }
    Ok(report)
}

fn write_active_state(
    storage_root: &Path,
    state: &NoIntroPackState,
) -> Result<(), NoIntroPackImportError> {
    let body = serde_json::to_string_pretty(state)
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?;
    crate::atomic_write_text(&storage_root.join("state.json"), &format!("{body}\n"))
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))
}

fn write_staged_state(
    storage_root: &Path,
    report: &NoIntroPackImportReport,
) -> Result<(), NoIntroPackImportError> {
    let accepted_members = report
        .accepted
        .iter()
        .map(|source| NoIntroPackStateMember {
            member: source.artifact_name.clone(),
            artifact_sha256: source.artifact_sha256.clone(),
            system_name: source.system_name.clone(),
            variant: source.variant,
            upstream_version: source.upstream_version.clone(),
            entry_count: source.entry_count,
            rom_count: source.rom_count,
        })
        .collect();
    let staged = NoIntroPackStagedState {
        schema_version: NO_INTRO_PACK_SCHEMA_VERSION,
        pack_sha256: report.pack_sha256.clone(),
        snapshot_sha256: report
            .snapshot_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| NoIntroPackImportError::State("staged snapshot has no id".into()))?
            .to_string(),
        accepted_members,
        rejected: report.rejected.clone(),
    };
    let body = serde_json::to_string_pretty(&staged)
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?;
    crate::atomic_write_text(&storage_root.join("staged.json"), &format!("{body}\n"))
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))
}

/// Activates the staged candidate, if its content-addressed payload is still
/// complete.  No source DAT is rewritten and no network is involved.
pub fn activate_staged_no_intro_pack_at(
    storage_root: &Path,
) -> Result<NoIntroPackActivationReport, NoIntroPackImportError> {
    let staged_path = storage_root.join("staged.json");
    let staged: NoIntroPackStagedState = serde_json::from_str(
        &fs::read_to_string(&staged_path).map_err(|error| io_error(&staged_path, error))?,
    )
    .map_err(|error| NoIntroPackImportError::State(error.to_string()))?;
    if staged.schema_version != NO_INTRO_PACK_SCHEMA_VERSION {
        return Err(NoIntroPackImportError::State(
            "unsupported staged No-Intro pack schema".into(),
        ));
    }
    let snapshot_path = storage_root.join("snapshots").join(&staged.snapshot_sha256);
    if !snapshot_is_complete(&snapshot_path, &staged.accepted_members) {
        return Err(NoIntroPackImportError::State(
            "staged No-Intro snapshot is incomplete".into(),
        ));
    }
    let managed_active = active_managed_state(storage_root)?;
    let previous = managed_active
        .as_ref()
        .map(|(state, _)| state.clone())
        .or_else(|| load_state(&storage_root.join("state.json")).ok());
    let mut pack_sha256s = previous
        .as_ref()
        .map(|state| state.pack_sha256s.clone())
        .unwrap_or_default();
    if let Some(state) = previous.as_ref()
        && !state.pack_sha256.is_empty()
    {
        pack_sha256s.push(state.pack_sha256.clone());
    }
    pack_sha256s.push(staged.pack_sha256.clone());
    pack_sha256s.sort();
    pack_sha256s.dedup();
    let state = NoIntroPackState {
        schema_version: NO_INTRO_PACK_SCHEMA_VERSION,
        pack_sha256: staged.pack_sha256.clone(),
        pack_sha256s,
        snapshot_sha256: Some(staged.snapshot_sha256.clone()),
        accepted_members: staged.accepted_members.clone(),
    };
    let expected_active = managed_active.as_ref().map(|(_, hash)| hash.as_str());
    activate_managed_state(storage_root, &state, expected_active)?;
    // Migration mirror; authoritative active/history state is managed/.
    write_active_state(storage_root, &state)?;
    let accepted = load_sources(&snapshot_path, &state.accepted_members)?;
    let _ = super::managed_lifecycle::register_no_intro_pack_at(
        storage_root,
        &staged.pack_sha256,
        &staged.snapshot_sha256,
        &accepted,
        &staged.rejected,
    )?;
    fs::remove_file(&staged_path).map_err(|error| io_error(&staged_path, error))?;
    Ok(NoIntroPackActivationReport {
        import: NoIntroPackImportReport {
            status: NoIntroPackImportStatus::Updated,
            pack_sha256: staged.pack_sha256,
            snapshot_path,
            accepted,
            rejected: staged.rejected,
        },
        verification: crate::identity_source::managed_snapshot::VerificationFreshness::NeedsRecheck,
    })
}

/// Restores the lifecycle-selected predecessor without deleting the newer
/// snapshot. The returned verification marker is intentionally the same as a
/// fresh activation: ROM verification must be rerun against the restored
/// source.
pub fn rollback_no_intro_pack_at(
    storage_root: &Path,
) -> Result<NoIntroPackActivationReport, NoIntroPackImportError> {
    let store = managed_store(storage_root)?;
    let current = active_managed_state(storage_root)?
        .ok_or_else(|| NoIntroPackImportError::State("no active No-Intro snapshot".into()))?;
    let target_record = store
        .history_snapshots()
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?
        .into_iter()
        .next()
        .ok_or_else(|| {
            NoIntroPackImportError::State("no safe No-Intro rollback is available".into())
        })?;
    let target = decode_managed_state(
        &store
            .snapshot_bytes(&target_record)
            .map_err(|error| NoIntroPackImportError::State(error.to_string()))?,
    )?;
    let members: Vec<_> = target
        .accepted_members
        .iter()
        .map(|member| NoIntroPackStateMember {
            member: member.member.clone(),
            artifact_sha256: member.artifact_sha256.clone(),
            system_name: member.system_name.clone(),
            variant: member.variant,
            upstream_version: member.upstream_version.clone(),
            entry_count: member.entry_count,
            rom_count: member.rom_count,
        })
        .collect();
    let target_snapshot = target
        .snapshot_sha256
        .clone()
        .ok_or_else(|| NoIntroPackImportError::State("rollback target has no snapshot".into()))?;
    let snapshot_path = storage_root.join("snapshots").join(&target_snapshot);
    if !snapshot_is_complete(&snapshot_path, &members) {
        return Err(NoIntroPackImportError::State(
            "rollback target snapshot is incomplete".into(),
        ));
    }
    let state = NoIntroPackState {
        schema_version: NO_INTRO_PACK_SCHEMA_VERSION,
        pack_sha256: target.pack_sha256.clone(),
        pack_sha256s: current.0.pack_sha256s,
        snapshot_sha256: Some(target_snapshot.clone()),
        accepted_members: members,
    };
    activate_managed_state(storage_root, &state, Some(current.1.as_str()))?;
    write_active_state(storage_root, &state)?;
    let accepted = load_sources(&snapshot_path, &state.accepted_members)?;
    let rejected = super::managed_lifecycle::load_no_intro_pack_snapshots_at(storage_root)
        .map_err(|error| NoIntroPackImportError::State(error.to_string()))?
        .into_iter()
        .find(|snapshot| snapshot.pack_sha256 == target.pack_sha256)
        .map(|snapshot| snapshot.rejected)
        .unwrap_or_default();
    let _ = super::managed_lifecycle::register_no_intro_pack_at(
        storage_root,
        &target.pack_sha256,
        &target_snapshot,
        &accepted,
        &rejected,
    )?;
    Ok(NoIntroPackActivationReport {
        import: NoIntroPackImportReport {
            status: NoIntroPackImportStatus::Updated,
            pack_sha256: target.pack_sha256.clone(),
            snapshot_path,
            accepted,
            rejected,
        },
        verification: crate::identity_source::managed_snapshot::VerificationFreshness::NeedsRecheck,
    })
}

/// Compares a staged content-addressed snapshot with the active pointer.
pub fn compare_staged_no_intro_pack_at(
    storage_root: &Path,
) -> Result<Option<NoIntroPackComparison>, NoIntroPackImportError> {
    let staged_path = storage_root.join("staged.json");
    let staged: NoIntroPackStagedState = match fs::read_to_string(&staged_path) {
        Ok(body) => serde_json::from_str(&body)
            .map_err(|error| NoIntroPackImportError::State(error.to_string()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(&staged_path, error)),
    };
    let Some(active) = active_managed_state(storage_root)?
        .map(|(state, _)| state)
        .or_else(|| load_state(&storage_root.join("state.json")).ok())
    else {
        return Ok(Some(NoIntroPackComparison::NoActiveSnapshot));
    };
    Ok(Some(
        if active.snapshot_sha256.as_deref() == Some(staged.snapshot_sha256.as_str()) {
            NoIntroPackComparison::SameSnapshot
        } else {
            NoIntroPackComparison::DifferentSnapshot
        },
    ))
}

/// Loads a persisted staged candidate without activating or reparsing the
/// active source. A damaged candidate is reported rather than shown as ready.
pub fn load_staged_no_intro_pack_summary_at(
    storage_root: &Path,
) -> Result<Option<NoIntroPackInspection>, NoIntroPackImportError> {
    let staged_path = storage_root.join("staged.json");
    let staged: NoIntroPackStagedState = match fs::read_to_string(&staged_path) {
        Ok(body) => serde_json::from_str(&body)
            .map_err(|error| NoIntroPackImportError::State(error.to_string()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(&staged_path, error)),
    };
    let snapshot_path = storage_root.join("snapshots").join(&staged.snapshot_sha256);
    if !snapshot_is_complete(&snapshot_path, &staged.accepted_members) {
        return Err(NoIntroPackImportError::State(
            "staged No-Intro snapshot is incomplete".into(),
        ));
    }
    Ok(Some(NoIntroPackInspection {
        pack_sha256: staged.pack_sha256,
        classification: NoIntroPackClassification::from_variants(
            staged.accepted_members.iter().map(|member| member.variant),
        ),
        accepted: staged
            .accepted_members
            .into_iter()
            .map(|member| NoIntroPackMemberInspection {
                member: member.member,
                system_name: member.system_name,
                variant: member.variant,
                upstream_version: member.upstream_version,
                artifact_sha256: member.artifact_sha256,
                entry_count: member.entry_count,
                rom_count: member.rom_count,
            })
            .collect(),
        rejected: staged.rejected,
    }))
}

/// Validates a pack without publishing a snapshot or changing any managed
/// state. The same bounded ZIP traversal and DAT parser used by import are
/// used here; only the temporary staging directory is created and removed.
pub fn inspect_no_intro_pack_at(
    path: &Path,
) -> Result<NoIntroPackInspection, NoIntroPackImportError> {
    let pack_sha256 = validate_pack(path)?;
    let staging = std::env::temp_dir().join(format!(
        "archivefs-no-intro-inspect-{}-{}",
        std::process::id(),
        unique_id()
    ));
    let result = build_staged_snapshot(path, &staging);
    let (accepted_members, rejected) = match result {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    let inspection = NoIntroPackInspection {
        pack_sha256,
        classification: NoIntroPackClassification::from_variants(
            accepted_members.iter().map(|member| member.variant),
        ),
        accepted: accepted_members
            .iter()
            .map(|member| NoIntroPackMemberInspection {
                member: member.member.clone(),
                system_name: member.system_name.clone(),
                variant: member.variant,
                upstream_version: member.upstream_version.clone(),
                artifact_sha256: member.artifact_sha256.clone(),
                entry_count: member.entry_count,
                rom_count: member.rom_count,
            })
            .collect(),
        rejected,
    };
    let _ = fs::remove_dir_all(&staging);
    Ok(inspection)
}

pub fn inspect_no_intro_pack(path: &Path) -> Result<NoIntroPackInspection, NoIntroPackImportError> {
    inspect_no_intro_pack_at(path)
}

pub fn load_current_no_intro_pack_summary_at(
    storage_root: &Path,
) -> Result<Option<NoIntroPackInstalledSummary>, NoIntroPackImportError> {
    let state = match active_managed_state(storage_root)? {
        Some((state, _)) => state,
        None => match load_state(&storage_root.join("state.json")) {
            Ok(state) => state,
            Err(NoIntroPackImportError::Io { error, .. })
                if error.kind() == io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        },
    };
    if state.schema_version != NO_INTRO_PACK_SCHEMA_VERSION {
        return Err(NoIntroPackImportError::State(format!(
            "unsupported No-Intro pack schema version {}",
            state.schema_version
        )));
    }
    let snapshot = storage_root.join("snapshots").join(
        state
            .snapshot_sha256
            .as_deref()
            .unwrap_or(&state.pack_sha256),
    );
    if !snapshot_is_complete(&snapshot, &state.accepted_members) {
        return Err(NoIntroPackImportError::State(
            "published No-Intro pack snapshot is incomplete".to_string(),
        ));
    }
    Ok(Some(NoIntroPackInspection {
        pack_sha256: state.pack_sha256,
        classification: NoIntroPackClassification::from_variants(
            state.accepted_members.iter().map(|member| member.variant),
        ),
        accepted: state
            .accepted_members
            .iter()
            .map(|member| NoIntroPackMemberInspection {
                member: member.member.clone(),
                system_name: member.system_name.clone(),
                variant: member.variant,
                upstream_version: member.upstream_version.clone(),
                artifact_sha256: member.artifact_sha256.clone(),
                entry_count: member.entry_count,
                rom_count: member.rom_count,
            })
            .collect(),
        rejected: Vec::new(),
    }))
}

pub fn load_current_no_intro_pack_summary()
-> Result<Option<NoIntroPackInstalledSummary>, NoIntroPackImportError> {
    let root = crate::app_dirs::data_path(PACK_DIRECTORY).map_err(|error| {
        NoIntroPackImportError::State(format!("cannot resolve application data path: {error}"))
    })?;
    load_current_no_intro_pack_summary_at(&root)
}

/// Loads the currently published pack after a process restart. The files are
/// re-parsed through the ordinary No-Intro importer, so a damaged snapshot is
/// never silently returned as usable evidence.
pub fn load_current_no_intro_pack_at(
    storage_root: &Path,
) -> Result<Option<Vec<ImportedNoIntroSource>>, NoIntroPackImportError> {
    let state = match active_managed_state(storage_root)? {
        Some((state, _)) => state,
        None => match load_state(&storage_root.join("state.json")) {
            Ok(state) => state,
            Err(NoIntroPackImportError::Io { error, .. })
                if error.kind() == io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        },
    };
    if state.schema_version != NO_INTRO_PACK_SCHEMA_VERSION {
        return Err(NoIntroPackImportError::State(format!(
            "unsupported No-Intro pack schema version {}",
            state.schema_version
        )));
    }
    let snapshot = storage_root.join("snapshots").join(
        state
            .snapshot_sha256
            .as_deref()
            .unwrap_or(&state.pack_sha256),
    );
    if !snapshot_is_complete(&snapshot, &state.accepted_members) {
        return Err(NoIntroPackImportError::State(
            "published No-Intro pack snapshot is incomplete".to_string(),
        ));
    }
    Ok(Some(load_sources(&snapshot, &state.accepted_members)?))
}

pub fn load_current_no_intro_pack()
-> Result<Option<Vec<ImportedNoIntroSource>>, NoIntroPackImportError> {
    let root = crate::app_dirs::data_path(PACK_DIRECTORY).map_err(|error| {
        NoIntroPackImportError::State(format!("cannot resolve application data path: {error}"))
    })?;
    load_current_no_intro_pack_at(&root)
}

fn build_staged_snapshot(
    pack_path: &Path,
    staging: &Path,
) -> Result<(Vec<NoIntroPackStateMember>, Vec<RejectedNoIntroPackMember>), NoIntroPackImportError> {
    fs::create_dir_all(staging.join("dats")).map_err(|error| io_error(staging, error))?;
    let file = File::open(pack_path).map_err(|error| io_error(pack_path, error))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| NoIntroPackImportError::InvalidArchive {
            detail: error.to_string(),
        })?;
    if archive.len() > NO_INTRO_PACK_MAX_MEMBERS {
        return Err(NoIntroPackImportError::LimitExceeded {
            detail: format!(
                "archive has {} members; maximum is {NO_INTRO_PACK_MAX_MEMBERS}",
                archive.len()
            ),
        });
    }

    let mut total_dat_bytes = 0_u64;
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for index in 0..archive.len() {
        let (member, is_dir, size) = {
            let raw = archive.by_index_raw(index).map_err(|error| {
                NoIntroPackImportError::InvalidArchive {
                    detail: error.to_string(),
                }
            })?;
            (raw.name().to_string(), raw.is_dir(), raw.size())
        };
        validate_member_name(&member)?;
        if is_dir || !is_dat_name(&member) {
            continue;
        }
        if size > NO_INTRO_PACK_MAX_DAT_BYTES {
            rejected.push(RejectedNoIntroPackMember {
                member,
                reason: format!(
                    "DAT member is {size} bytes; maximum is {NO_INTRO_PACK_MAX_DAT_BYTES}"
                ),
            });
            continue;
        }
        total_dat_bytes = total_dat_bytes.saturating_add(size);
        validate_total_dat_bytes(total_dat_bytes)?;
        // Accepted members are stored contiguously. ZIP indices include
        // ignored/rejected members and therefore cannot be used as snapshot
        // ordinals for restart/reload.
        let output_name = format!("{}.dat", accepted.len());
        let output_path = staging.join("dats").join(&output_name);
        let mut input =
            archive
                .by_index(index)
                .map_err(|error| NoIntroPackImportError::InvalidArchive {
                    detail: error.to_string(),
                })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)
            .map_err(|error| io_error(&output_path, error))?;
        let mut limited = (&mut input).take(NO_INTRO_PACK_MAX_DAT_BYTES + 1);
        io::copy(&mut limited, &mut output).map_err(|error| io_error(&output_path, error))?;
        output
            .flush()
            .map_err(|error| io_error(&output_path, error))?;
        output
            .sync_all()
            .map_err(|error| io_error(&output_path, error))?;

        match import_no_intro_dat(&output_path) {
            Ok(mut source) => {
                if source
                    .dat
                    .source
                    .parse_warnings
                    .iter()
                    .any(|warning| warning.to_ascii_lowercase().contains("truncated"))
                {
                    return Err(NoIntroPackImportError::IncompleteDat {
                        member,
                        detail: "parser reported a truncated document".to_string(),
                    });
                }
                source.artifact_name = member.clone();
                source.artifact_path = output_path;
                accepted.push(NoIntroPackStateMember {
                    member,
                    artifact_sha256: source.artifact_sha256,
                    system_name: source.system_name,
                    variant: source.variant,
                    upstream_version: source.upstream_version,
                    entry_count: source.entry_count,
                    rom_count: source.rom_count,
                });
            }
            Err(NoIntroImportError::NotNoIntro {
                detected_ecosystem, ..
            }) => {
                let _ = fs::remove_file(&output_path);
                rejected.push(RejectedNoIntroPackMember {
                    member,
                    reason: format!("content identifies as {detected_ecosystem:?}, not No-Intro"),
                });
            }
            Err(error) => {
                return Err(NoIntroPackImportError::CorruptDat { member, error });
            }
        }
    }
    Ok((accepted, rejected))
}

fn validate_pack(path: &Path) -> Result<String, NoIntroPackImportError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| NoIntroPackImportError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(NoIntroPackImportError::InvalidArchive {
            detail: "pack must be a regular, non-symlink file".to_string(),
        });
    }
    PackLimits.validate_archive(path, metadata.len())?;
    sha256_file(path).map_err(|error| NoIntroPackImportError::Io {
        path: path.to_path_buf(),
        error,
    })
}

fn load_sources(
    snapshot: &Path,
    members: &[NoIntroPackStateMember],
) -> Result<Vec<ImportedNoIntroSource>, NoIntroPackImportError> {
    let mut sources = Vec::new();
    for (index, member) in members.iter().enumerate() {
        let path = snapshot.join("dats").join(format!("{index}.dat"));
        let mut source =
            import_no_intro_dat(&path).map_err(|error| NoIntroPackImportError::CorruptDat {
                member: member.member.clone(),
                error,
            })?;
        source.artifact_name = member.member.clone();
        source.artifact_path = path;
        sources.push(source);
    }
    Ok(sources)
}

fn snapshot_is_complete(snapshot: &Path, members: &[NoIntroPackStateMember]) -> bool {
    snapshot.is_dir()
        && members.iter().enumerate().all(|(index, member)| {
            let path = snapshot.join("dats").join(format!("{index}.dat"));
            path.is_file() && sha256_file(&path).is_ok_and(|sha| sha == member.artifact_sha256)
        })
}

fn snapshot_matches_members(snapshot: &Path, members: &[NoIntroPackStateMember]) -> bool {
    snapshot.is_dir()
        && members.iter().enumerate().all(|(index, member)| {
            let path = snapshot.join("dats").join(format!("{index}.dat"));
            path.is_file() && sha256_file(&path).is_ok_and(|sha| sha == member.artifact_sha256)
        })
}

fn snapshot_id(members: &[NoIntroPackStateMember]) -> String {
    let mut hashes: Vec<&str> = members
        .iter()
        .map(|member| member.artifact_sha256.as_str())
        .collect();
    hashes.sort_unstable();
    let mut digest = Sha256::new();
    for hash in hashes {
        digest.update(hash.as_bytes());
        digest.update([0]);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// Historical snapshots are deliberately retained. A later read-only
// lifecycle classification may identify a safe prune candidate, but this
// import path never deletes content-addressed evidence.
fn prune_old_snapshots(_storage_root: &Path, _current_sha256: &str) {}

fn validate_member_name(member: &str) -> Result<(), NoIntroPackImportError> {
    if member.is_empty() || member.len() > NO_INTRO_PACK_MAX_MEMBER_NAME_BYTES {
        return Err(NoIntroPackImportError::Traversal {
            member: member.to_string(),
        });
    }
    if member.contains('\\') {
        return Err(NoIntroPackImportError::Traversal {
            member: member.to_string(),
        });
    }
    let path = Path::new(member);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(NoIntroPackImportError::Traversal {
            member: member.to_string(),
        });
    }
    Ok(())
}

fn is_dat_name(member: &str) -> bool {
    Path::new(member)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("dat"))
}

fn validate_total_dat_bytes(total: u64) -> Result<(), NoIntroPackImportError> {
    if total > NO_INTRO_PACK_MAX_TOTAL_DAT_BYTES {
        return Err(NoIntroPackImportError::LimitExceeded {
            detail: format!("DAT members exceed {NO_INTRO_PACK_MAX_TOTAL_DAT_BYTES} bytes"),
        });
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<(), NoIntroPackImportError> {
    fs::create_dir_all(path).map_err(|error| io_error(path, error))?;
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(NoIntroPackImportError::State(format!(
            "storage path is not a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn load_state(path: &Path) -> Result<NoIntroPackState, NoIntroPackImportError> {
    let text = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
    serde_json::from_str(&text).map_err(|error| NoIntroPackImportError::State(error.to_string()))
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn unique_id() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn io_error(path: &Path, error: io::Error) -> NoIntroPackImportError {
    NoIntroPackImportError::Io {
        path: path.to_path_buf(),
        error,
    }
}

#[cfg(test)]
mod tests;
