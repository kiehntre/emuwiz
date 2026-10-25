//! Offline HackHash detailed-export evidence.
//!
//! HackHash is deliberately treated as a supplemental provider.  This module
//! imports a user-selected `format=detailed` JSON export, validates the fields
//! that EmuWiz can safely consume, and stores the original bytes in the common
//! immutable managed-snapshot store.  It never contacts HackHash and never
//! changes native identity.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::hackhash_identity::HackHashOutputHashes;
use crate::standalone_patch::StandalonePatchFormat;
use url::Url;

use super::managed_snapshot::{
    ActivationPreview, ActivationResult, ManagedSourceDescriptor, ManagedSourceKind,
    ManagedSourceMetadata, ManagedSourceReference, ManagedSourceSnapshot, ManagedSourceStore,
    ManagedSourceTransport, ManagedSourceTrust, SourceResponseMetadata, UpdateCheck,
    ValidatedCandidate, ValidationReport,
};
use crate::{ArchiveFsError, Result};

pub const HACKHASH_PROVIDER_ID: &str = "hackhash";
pub const HACKHASH_PARSER_SCHEMA_VERSION: &str = "1";
pub const HACKHASH_MAX_EXPORT_BYTES: u64 = 64 * 1024 * 1024;
pub const HACKHASH_MAX_RECORDS: usize = 100_000;
const MAX_TEXT: usize = 8 * 1024;
const MAX_LIST_ITEMS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashExport {
    #[serde(default)]
    pub header: Option<serde_json::Value>,
    pub machines: Vec<HackHashRecord>,
    #[serde(default)]
    pub generated: Option<String>,
    #[serde(default)]
    pub count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HackHashRecord {
    pub machine_name: String,
    pub description: String,
    pub rom_name: String,
    pub platform: String,
    pub file_size: String,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
    pub details: HackHashDetails,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HackHashDetails {
    pub hack_name: String,
    pub version: String,
    #[serde(default)]
    pub version_changelog: Option<String>,
    #[serde(default)]
    pub translation_languages: Vec<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub release_year: Option<i32>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<HackHashTag>,
    #[serde(default)]
    pub hack_family: Option<HackHashFamily>,
    #[serde(default)]
    pub patch: HackHashPatch,
    #[serde(default)]
    pub base_rom: Option<HackHashBaseRom>,
    #[serde(default)]
    pub alternate_formats: Vec<HackHashAlternateFormat>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub release_page_url: Option<String>,
    #[serde(default)]
    pub github_url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub game_database_links: Option<HackHashGameDatabaseLinks>,
    pub approved_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HackHashPatch {
    #[serde(rename = "type")]
    pub patch_type: Option<String>,
    pub filename: Option<String>,
    pub sha1: Option<String>,
}

/// Native inspection state recorded after a HackHash-derived output is
/// prepared.  HackHash remains external evidence; this state is the result of
/// EmuWiz inspecting the produced bytes independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HackHashVerificationState {
    ProviderOutputAndNativeInspection,
    ProviderOutputNativeIdentityUnavailable,
    ProviderOutputNativeIdentityConflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashNativeInspection {
    pub complete: bool,
    pub platform: String,
    pub format: String,
    pub evidence: Vec<String>,
    pub warnings: Vec<String>,
    pub hashes: HackHashOutputHashes,
}

/// Durable transformation chain for one generated HackHash output.  This is
/// stored in the shared transaction journal, alongside the rollback receipt,
/// rather than in the transient GUI state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashPatchProvenance {
    pub base_path: std::path::PathBuf,
    pub base_identity: Option<String>,
    pub base_sha256: String,
    pub patch_path: std::path::PathBuf,
    pub patch_sha256: String,
    pub patch_format: StandalonePatchFormat,
    pub patch_source: String,
    pub expected_output: HackHashOutputHashes,
    pub actual_output: HackHashOutputHashes,
    pub output_path: std::path::PathBuf,
    pub output_sha256: String,
    pub hack_titles: Vec<String>,
    pub family_versions: Vec<String>,
    pub provider_snapshot_sha256: String,
    pub provider_provenance: String,
    pub transaction_id: String,
    pub timestamp_unix_seconds: u64,
    pub verification_state: HackHashVerificationState,
    pub native_inspection: HackHashNativeInspection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HackHashBaseRom {
    pub name: String,
    pub platform: String,
    #[serde(default)]
    pub file_extension: Option<String>,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HackHashAlternateFormat {
    pub format: String,
    pub filename: String,
    pub file_size: String,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashTag {
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HackHashFamily {
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub release_year: Option<i32>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HackHashGameDatabaseLinks {
    #[serde(default)]
    pub canonical_name: Option<String>,
    #[serde(default)]
    pub igdb: Option<HackHashIgdbLink>,
    #[serde(default)]
    #[serde(rename = "theGamesDB")]
    pub the_games_db: Option<String>,
    #[serde(default)]
    pub launchbox: Option<String>,
    #[serde(default)]
    pub giant_bomb: Option<String>,
    #[serde(default)]
    pub screen_scraper: Option<String>,
    #[serde(default)]
    #[serde(rename = "steamGridDB")]
    pub steam_grid_db: Option<String>,
    #[serde(default)]
    pub retro_achievements: Option<String>,
    #[serde(default)]
    pub steam: Option<String>,
    #[serde(default)]
    pub gog: Option<String>,
    #[serde(default)]
    pub epic_games: Option<String>,
    #[serde(default)]
    pub wikipedia: Option<String>,
    #[serde(default)]
    pub hasheous_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HackHashIgdbLink {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HackHashValidation {
    pub export: HackHashExport,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HackHashIndex {
    pub by_patched_sha1: BTreeMap<String, Vec<usize>>,
    pub by_md5: BTreeMap<String, Vec<usize>>,
    pub by_crc32: BTreeMap<String, Vec<usize>>,
    pub by_base_sha1: BTreeMap<String, Vec<usize>>,
    pub by_patch_sha1: BTreeMap<String, Vec<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HackHashEvidenceClaim {
    pub record_index: usize,
    pub patched_output_sha1: String,
    pub hack_name: String,
    pub version: String,
    pub platform: String,
    pub native_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HackHashValidatedImport {
    pub candidate: ValidatedCandidate,
    pub validation: HackHashValidation,
    pub index: HackHashIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HackHashFetchState {
    Changed,
    AlreadyCurrent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HackHashFetchResult {
    pub import: HackHashValidatedImport,
    pub state: HackHashFetchState,
}

#[derive(Debug, Clone)]
pub struct HackHashStore {
    store: ManagedSourceStore,
}

impl HackHashStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        let descriptor = ManagedSourceDescriptor {
            provider_id: HACKHASH_PROVIDER_ID.into(),
            display_name: "HackHash detailed JSON export".into(),
            source_kind: ManagedSourceKind::Local,
            // The actual selected path is user input and is deliberately not
            // used as the managed-store identity.  This lets a later import
            // load and validate the existing active snapshot without needing
            // the original source path to still exist.
            source: ManagedSourceReference::LocalPath(PathBuf::from(
                "hackhash-detailed-json-import",
            )),
            expected_media_type: "application/json".into(),
            maximum_size_bytes: HACKHASH_MAX_EXPORT_BYTES,
            attribution_url: Some("https://github.com/darkblood159/HackHash".into()),
            parser_schema_version: HACKHASH_PARSER_SCHEMA_VERSION.into(),
            trust: ManagedSourceTrust::UserProvided,
        };
        ManagedSourceStore::new(root, descriptor).map(|store| Self { store })
    }

    pub fn import_file(&self, path: &Path) -> Result<HackHashValidatedImport> {
        let metadata =
            fs::symlink_metadata(path).map_err(|error| ArchiveFsError::io(path, error))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(config("HackHash export must be a regular non-symlink file"));
        }
        if metadata.len() > HACKHASH_MAX_EXPORT_BYTES {
            return Err(config("HackHash export exceeds the maximum import size"));
        }
        let bytes = fs::read(path).map_err(|error| ArchiveFsError::io(path, error))?;
        self.import_bytes(&bytes)
    }

    pub fn import_bytes(&self, bytes: &[u8]) -> Result<HackHashValidatedImport> {
        if bytes.len() as u64 > HACKHASH_MAX_EXPORT_BYTES {
            return Err(config("HackHash export exceeds the maximum import size"));
        }
        let validation = parse_detailed_export(bytes).map_err(|error| config(error.to_string()))?;
        let record_count = validation.export.machines.len();
        let candidate = self
            .store
            .stage_bytes(bytes, ManagedSourceMetadata::default())
            .and_then(|staged| {
                self.store.validate_candidate(
                    staged,
                    ValidationReport {
                        valid: true,
                        summary: format!("HackHash detailed export with {record_count} records"),
                        record_count: Some(record_count as u64),
                        warnings: validation.warnings.clone(),
                    },
                )
            })?;
        let index = HackHashIndex::build(&validation.export);
        Ok(HackHashValidatedImport {
            candidate,
            validation,
            index,
        })
    }

    /// Performs a metadata-only check after an explicit user action. It does
    /// not download or stage a response.
    pub fn check_for_update(
        &self,
        url: &str,
        transport: &dyn ManagedSourceTransport,
    ) -> Result<UpdateCheck> {
        let active = self.active_snapshot()?;
        let metadata = transport
            .metadata(url, &[])
            .map_err(|detail| config(format!("HackHash update check failed: {detail}")))?;
        validate_response_metadata(&metadata)?;
        if metadata.status == 304 {
            return Ok(UpdateCheck::Unchanged {
                metadata: metadata.into(),
            });
        }
        if !(200..300).contains(&metadata.status) {
            return Err(config(format!(
                "HackHash update check returned HTTP {}",
                metadata.status
            )));
        }
        let metadata: ManagedSourceMetadata = metadata.into();
        let unchanged = active.as_ref().is_some_and(|snapshot| {
            metadata.etag.is_some() && metadata.etag == snapshot.etag
                || metadata.last_modified.is_some()
                    && metadata.last_modified == snapshot.last_modified
        });
        Ok(if unchanged {
            UpdateCheck::Unchanged { metadata }
        } else {
            UpdateCheck::Available { metadata }
        })
    }

    /// Downloads only after the caller explicitly asks, validates the whole
    /// detailed JSON document, and publishes an immutable candidate. The
    /// active snapshot is never changed here.
    pub fn fetch_candidate(
        &self,
        url: &str,
        transport: &dyn ManagedSourceTransport,
    ) -> Result<HackHashFetchResult> {
        let staged = self.store.fetch_candidate_from(url, transport)?;
        if let Some(content_type) = staged.metadata.content_type.clone() {
            if !is_json_content_type(&content_type) {
                self.store.discard_staged(staged);
                return Err(config(format!(
                    "HackHash response has non-JSON Content-Type: {content_type}"
                )));
            }
        }
        let staged_path = staged.path.clone();
        let bytes = match fs::read(&staged_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.store.discard_staged(staged);
                return Err(ArchiveFsError::io("HackHash staged candidate", error));
            }
        };
        let validation = match parse_detailed_export(&bytes) {
            Ok(validation) => validation,
            Err(error) => {
                self.store.discard_staged(staged);
                return Err(config(error.to_string()));
            }
        };
        let record_count = validation.export.machines.len();
        let candidate = match self.store.validate_candidate(
            staged,
            ValidationReport {
                valid: true,
                summary: format!("HackHash detailed export with {record_count} records"),
                record_count: Some(record_count as u64),
                warnings: validation.warnings.clone(),
            },
        ) {
            Ok(candidate) => candidate,
            Err(error) => return Err(error),
        };
        let active = self.active_snapshot()?;
        let state = if active
            .as_ref()
            .is_some_and(|snapshot| snapshot.sha256 == candidate.snapshot.sha256)
        {
            HackHashFetchState::AlreadyCurrent
        } else {
            HackHashFetchState::Changed
        };
        let index = HackHashIndex::build(&validation.export);
        Ok(HackHashFetchResult {
            import: HackHashValidatedImport {
                candidate,
                validation,
                index,
            },
            state,
        })
    }

    pub fn preview_activation(
        &self,
        import: &HackHashValidatedImport,
    ) -> Result<ActivationPreview> {
        self.store.preview_activation(&import.candidate)
    }

    pub fn activate_snapshot(
        &self,
        import: &HackHashValidatedImport,
        expected_active: Option<&str>,
    ) -> Result<ActivationResult> {
        self.store
            .activate_snapshot(&import.candidate, expected_active)
    }

    pub fn active_snapshot(&self) -> Result<Option<ManagedSourceSnapshot>> {
        self.store.active_snapshot()
    }

    pub fn list_snapshots(&self) -> Result<Vec<ManagedSourceSnapshot>> {
        self.store.list_snapshots()
    }

    pub fn active_export(
        &self,
    ) -> Result<Option<(ManagedSourceSnapshot, HackHashExport, HackHashIndex)>> {
        let Some(snapshot) = self.store.active_snapshot()? else {
            return Ok(None);
        };
        let bytes = self.store.snapshot_bytes(&snapshot)?;
        let validation =
            parse_detailed_export(&bytes).map_err(|error| config(error.to_string()))?;
        let index = HackHashIndex::build(&validation.export);
        Ok(Some((snapshot, validation.export, index)))
    }
}

fn is_json_content_type(value: &str) -> bool {
    let media_type = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    media_type == "application/json" || media_type.ends_with("+json")
}

fn validate_response_metadata(metadata: &SourceResponseMetadata) -> Result<()> {
    if let Some(content_type) = metadata.content_type.as_deref() {
        if !is_json_content_type(content_type) {
            return Err(config(format!(
                "HackHash response has non-JSON Content-Type: {content_type}"
            )));
        }
    }
    Ok(())
}

impl HackHashIndex {
    pub fn build(export: &HackHashExport) -> Self {
        let mut index = Self {
            by_patched_sha1: BTreeMap::new(),
            by_md5: BTreeMap::new(),
            by_crc32: BTreeMap::new(),
            by_base_sha1: BTreeMap::new(),
            by_patch_sha1: BTreeMap::new(),
        };
        for (position, record) in export.machines.iter().enumerate() {
            add(&mut index.by_patched_sha1, &record.sha1, position);
            add(&mut index.by_md5, &record.md5, position);
            add(&mut index.by_crc32, &record.crc32, position);
            if let Some(base) = &record.details.base_rom {
                add(&mut index.by_base_sha1, &base.sha1, position);
            }
            if let Some(patch) = record.details.patch.sha1.as_deref() {
                add(&mut index.by_patch_sha1, patch, position);
            }
        }
        index
    }

    pub fn evidence_for_patched_sha1(
        &self,
        export: &HackHashExport,
        sha1: &str,
    ) -> Vec<HackHashEvidenceClaim> {
        let key = sha1.trim().to_ascii_lowercase();
        self.by_patched_sha1
            .get(&key)
            .into_iter()
            .flatten()
            .filter_map(|index| {
                export
                    .machines
                    .get(*index)
                    .map(|record| HackHashEvidenceClaim {
                        record_index: *index,
                        patched_output_sha1: record.sha1.clone(),
                        hack_name: record.details.hack_name.clone(),
                        version: record.details.version.clone(),
                        platform: record.platform.clone(),
                        native_verified: false,
                    })
            })
            .collect()
    }
}

pub fn parse_detailed_export(
    bytes: &[u8],
) -> std::result::Result<HackHashValidation, HackHashParseError> {
    if bytes.len() as u64 > HACKHASH_MAX_EXPORT_BYTES {
        return Err(HackHashParseError::Bounds("input exceeds 64 MiB".into()));
    }
    let export: HackHashExport = serde_json::from_slice(bytes)
        .map_err(|error| HackHashParseError::Malformed(error.to_string()))?;
    if export.machines.is_empty() {
        return Err(HackHashParseError::Invalid(
            "machines must not be empty".into(),
        ));
    }
    if export.machines.len() > HACKHASH_MAX_RECORDS {
        return Err(HackHashParseError::Bounds(
            "record count exceeds 100000".into(),
        ));
    }
    if export
        .count
        .is_some_and(|count| count != export.machines.len())
    {
        return Err(HackHashParseError::Invalid(
            "count does not match machines length".into(),
        ));
    }
    let mut warnings = Vec::new();
    for (index, record) in export.machines.iter().enumerate() {
        validate_record(record, index, &mut warnings)?;
    }
    let mut output_hashes = BTreeMap::<String, Vec<usize>>::new();
    for (index, record) in export.machines.iter().enumerate() {
        output_hashes
            .entry(record.sha1.to_ascii_lowercase())
            .or_default()
            .push(index);
    }
    for (sha1, positions) in output_hashes {
        if positions.len() > 1 && warnings.len() < 128 {
            warnings.push(format!(
                "duplicate patched-output SHA-1 {sha1} at records {positions:?}; lookup remains ambiguous"
            ));
        }
    }
    Ok(HackHashValidation { export, warnings })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HackHashParseError {
    Malformed(String),
    Invalid(String),
    Bounds(String),
}

impl std::fmt::Display for HackHashParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(detail) => write!(formatter, "malformed HackHash JSON: {detail}"),
            Self::Invalid(detail) => {
                write!(formatter, "invalid HackHash detailed export: {detail}")
            }
            Self::Bounds(detail) => write!(formatter, "HackHash import refused: {detail}"),
        }
    }
}

fn validate_record(
    record: &HackHashRecord,
    index: usize,
    warnings: &mut Vec<String>,
) -> std::result::Result<(), HackHashParseError> {
    text(&record.machine_name, "machineName", index)?;
    text(&record.description, "description", index)?;
    text(&record.rom_name, "romName", index)?;
    text(&record.platform, "platform", index)?;
    text(&record.details.hack_name, "details.hackName", index)?;
    text(&record.details.version, "details.version", index)?;
    text(&record.details.approved_at, "details.approvedAt", index)?;
    hash(&record.crc32, 8, "crc32", index)?;
    hash(&record.md5, 32, "md5", index)?;
    hash(&record.sha1, 40, "sha1", index)?;
    size(&record.file_size, "fileSize", index)?;
    validate_languages(&record.details.translation_languages, index)?;
    validate_patch(&record.details.patch, index)?;
    if let Some(base) = &record.details.base_rom {
        text(&base.name, "details.baseRom.name", index)?;
        text(&base.platform, "details.baseRom.platform", index)?;
        text(&base.status, "details.baseRom.status", index)?;
        hash(&base.crc32, 8, "details.baseRom.crc32", index)?;
        hash(&base.md5, 32, "details.baseRom.md5", index)?;
        hash(&base.sha1, 40, "details.baseRom.sha1", index)?;
    }
    for alternate in &record.details.alternate_formats {
        text(&alternate.format, "alternateFormats.format", index)?;
        text(&alternate.filename, "alternateFormats.filename", index)?;
        size(&alternate.file_size, "alternateFormats.fileSize", index)?;
        hash(&alternate.crc32, 8, "alternateFormats.crc32", index)?;
        hash(&alternate.md5, 32, "alternateFormats.md5", index)?;
        hash(&alternate.sha1, 40, "alternateFormats.sha1", index)?;
    }
    if record.details.alternate_formats.len() > MAX_LIST_ITEMS {
        return Err(HackHashParseError::Bounds(format!(
            "record {index} has too many alternate formats"
        )));
    }
    if record.details.translation_languages.len() > MAX_LIST_ITEMS
        || record.details.tags.len() > MAX_LIST_ITEMS
    {
        return Err(HackHashParseError::Bounds(format!(
            "record {index} has an oversized metadata list"
        )));
    }
    for tag in &record.details.tags {
        text(&tag.slug, "details.tags.slug", index)?;
        text(&tag.name, "details.tags.name", index)?;
    }
    if let Some(family) = &record.details.hack_family {
        text(&family.name, "details.hackFamily.name", index)?;
    }
    for (label, url) in [
        ("sourceUrl", record.details.source_url.as_deref()),
        ("releasePageUrl", record.details.release_page_url.as_deref()),
        ("githubUrl", record.details.github_url.as_deref()),
    ] {
        if let Some(value) = url {
            validate_url(value, label, index)?;
        }
    }
    if let Some(other) = record.details.patch.sha1.as_deref() {
        if warnings.len() < 128 && duplicate_hash_warning(record, other) {
            warnings.push(format!(
                "record {index}: patch SHA-1 equals a published field; verify provider data"
            ));
        }
    }
    Ok(())
}

fn validate_patch(
    patch: &HackHashPatch,
    index: usize,
) -> std::result::Result<(), HackHashParseError> {
    let any = patch.patch_type.is_some() || patch.filename.is_some() || patch.sha1.is_some();
    if !any {
        return Ok(());
    }
    if patch.patch_type.is_none() || patch.filename.is_none() || patch.sha1.is_none() {
        return Err(HackHashParseError::Invalid(format!(
            "record {index} has incomplete patch metadata"
        )));
    }
    text(
        patch.patch_type.as_deref().unwrap_or_default(),
        "details.patch.type",
        index,
    )?;
    text(
        patch.filename.as_deref().unwrap_or_default(),
        "details.patch.filename",
        index,
    )?;
    hash(
        patch.sha1.as_deref().unwrap_or_default(),
        40,
        "details.patch.sha1",
        index,
    )
    .map(|_| ())
}

fn validate_languages(
    languages: &[String],
    index: usize,
) -> std::result::Result<(), HackHashParseError> {
    for language in languages {
        text(language, "details.translationLanguages", index)?;
    }
    Ok(())
}

fn text(value: &str, field: &str, index: usize) -> std::result::Result<(), HackHashParseError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT || value.contains('\0') {
        return Err(HackHashParseError::Invalid(format!(
            "record {index} has invalid {field}"
        )));
    }
    Ok(())
}

fn size(value: &str, field: &str, index: usize) -> std::result::Result<u64, HackHashParseError> {
    value
        .parse::<u64>()
        .map_err(|_| HackHashParseError::Invalid(format!("record {index} has invalid {field}")))
}

fn hash(
    value: &str,
    length: usize,
    field: &str,
    index: usize,
) -> std::result::Result<String, HackHashParseError> {
    let normalised = value.trim().to_ascii_lowercase();
    if normalised.len() != length
        || !normalised
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(HackHashParseError::Invalid(format!(
            "record {index} has invalid {field}"
        )));
    }
    Ok(normalised)
}

fn validate_url(
    value: &str,
    field: &str,
    index: usize,
) -> std::result::Result<(), HackHashParseError> {
    let parsed = Url::parse(value)
        .map_err(|_| HackHashParseError::Invalid(format!("record {index} has invalid {field}")))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(HackHashParseError::Invalid(format!(
            "record {index} has invalid {field}"
        )));
    }
    Ok(())
}

fn duplicate_hash_warning(record: &HackHashRecord, patch_sha1: &str) -> bool {
    record.sha1.eq_ignore_ascii_case(patch_sha1)
}

fn add(index: &mut BTreeMap<String, Vec<usize>>, value: &str, position: usize) {
    index
        .entry(value.to_ascii_lowercase())
        .or_default()
        .push(position);
}

fn config(message: impl Into<String>) -> ArchiveFsError {
    ArchiveFsError::Config(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn record(sha1: &str, version: &str) -> HackHashRecord {
        HackHashRecord {
            machine_name: format!("Example {version}"),
            description: format!("Example {version}"),
            rom_name: format!("example-{version}.sfc"),
            platform: "SNES".into(),
            file_size: "4".into(),
            crc32: "1234abcd".into(),
            md5: "0123456789abcdef0123456789abcdef".into(),
            sha1: sha1.into(),
            details: HackHashDetails {
                hack_name: "Example Hack".into(),
                version: version.into(),
                version_changelog: Some("changes".into()),
                translation_languages: vec!["es".into()],
                author: Some("Author".into()),
                release_year: Some(2026),
                release_date: Some("2026-01-02".into()),
                description: Some("desc".into()),
                tags: vec![],
                hack_family: Some(HackHashFamily {
                    name: "Example Family".into(),
                    author: None,
                    release_year: None,
                    release_date: None,
                    description: None,
                }),
                patch: HackHashPatch {
                    patch_type: Some("ips".into()),
                    filename: Some("example.ips".into()),
                    sha1: Some("abcdefabcdefabcdefabcdefabcdefabcdefabcd".into()),
                },
                base_rom: Some(HackHashBaseRom {
                    name: "Base".into(),
                    platform: "SNES".into(),
                    file_extension: Some("sfc".into()),
                    crc32: "1234abcd".into(),
                    md5: "0123456789abcdef0123456789abcdef".into(),
                    sha1: "1111111111111111111111111111111111111111".into(),
                    status: "APPROVED".into(),
                }),
                alternate_formats: vec![HackHashAlternateFormat {
                    format: "RVZ".into(),
                    filename: "example.rvz".into(),
                    file_size: "9".into(),
                    crc32: "1234abcd".into(),
                    md5: "0123456789abcdef0123456789abcdef".into(),
                    sha1: "2222222222222222222222222222222222222222".into(),
                }],
                source_url: Some("https://example.com/source".into()),
                release_page_url: None,
                github_url: Some("https://github.com/example/project".into()),
                notes: None,
                game_database_links: None,
                approved_at: "2026-01-02T00:00:00.000Z".into(),
            },
        }
    }

    fn bytes(records: Vec<HackHashRecord>) -> Vec<u8> {
        let count = records.len();
        serde_json::to_vec(&serde_json::json!({ "machines": records, "count": count })).unwrap()
    }

    #[test]
    fn parses_base_patch_alternates_and_indexes_without_native_verification() {
        let export = parse_detailed_export(&bytes(vec![record(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "1.0",
        )]))
        .unwrap();
        let index = HackHashIndex::build(&export.export);
        assert_eq!(
            index.by_base_sha1["1111111111111111111111111111111111111111"],
            vec![0]
        );
        assert_eq!(
            index.by_patch_sha1["abcdefabcdefabcdefabcdefabcdefabcdefabcd"],
            vec![0]
        );
        let claims = index.evidence_for_patched_sha1(&export.export, &"A".repeat(40));
        assert!(!claims[0].native_verified);
    }

    #[test]
    fn rejects_invalid_hash_and_duplicate_output_is_preserved() {
        let mut first = record(&"a".repeat(40), "1.0");
        first.md5 = "bad".into();
        assert!(parse_detailed_export(&bytes(vec![first])).is_err());
        let parsed = parse_detailed_export(&bytes(vec![
            record(&"a".repeat(40), "1.0"),
            record(&"a".repeat(40), "1.1"),
        ]))
        .unwrap();
        assert!(
            parsed
                .warnings
                .iter()
                .any(|warning| warning.contains("duplicate patched-output SHA-1"))
        );
        let index = HackHashIndex::build(&parsed.export);
        assert_eq!(index.by_patched_sha1[&"a".repeat(40)], vec![0, 1]);
    }

    #[test]
    fn bounds_are_enforced() {
        let too_large = vec![b' '; (HACKHASH_MAX_EXPORT_BYTES + 1) as usize];
        assert!(matches!(
            parse_detailed_export(&too_large),
            Err(HackHashParseError::Bounds(_))
        ));
        let mut many = Vec::new();
        for position in 0..=HACKHASH_MAX_RECORDS {
            many.push(record(&format!("{position:040x}"), "1"));
        }
        assert!(matches!(
            parse_detailed_export(&bytes(many)),
            Err(HackHashParseError::Bounds(_))
        ));
    }

    #[test]
    fn malformed_json_is_rejected_before_snapshot_staging() {
        assert!(matches!(
            parse_detailed_export(b"{not json"),
            Err(HackHashParseError::Malformed(_))
        ));
    }

    #[test]
    fn store_retains_active_snapshot_across_imports() {
        let root = std::env::temp_dir().join(format!(
            "hackhash-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = HackHashStore::new(root.clone()).unwrap();
        let first = store
            .import_bytes(&bytes(vec![record(&"a".repeat(40), "1.0")]))
            .unwrap();
        let first_result = store.activate_snapshot(&first, None).unwrap();
        let second = store
            .import_bytes(&bytes(vec![record(&"b".repeat(40), "2.0")]))
            .unwrap();
        let preview = store.preview_activation(&second).unwrap();
        assert_eq!(
            preview.old.as_ref().unwrap().sha256,
            first_result.active.sha256
        );
        store
            .activate_snapshot(&second, Some(&first_result.active.sha256))
            .unwrap();
        assert_eq!(
            store.active_snapshot().unwrap().unwrap().sha256,
            second.candidate.snapshot.sha256
        );
        let _ = fs::remove_dir_all(root);
    }

    #[derive(Clone)]
    struct FetchFixture {
        body: Vec<u8>,
        status: u16,
        content_type: Option<String>,
        failure: Option<String>,
    }

    impl ManagedSourceTransport for FetchFixture {
        fn metadata(
            &self,
            _: &str,
            _: &[(String, String)],
        ) -> std::result::Result<SourceResponseMetadata, String> {
            if let Some(failure) = &self.failure {
                return Err(failure.clone());
            }
            Ok(SourceResponseMetadata {
                status: self.status,
                content_length: Some(self.body.len() as u64),
                content_type: self.content_type.clone(),
                ..Default::default()
            })
        }

        fn fetch(
            &self,
            _: &str,
            _: &[(String, String)],
            maximum_size: u64,
            destination: &mut dyn Write,
        ) -> std::result::Result<SourceResponseMetadata, String> {
            if let Some(failure) = &self.failure {
                return Err(failure.clone());
            }
            if !(200..300).contains(&self.status) {
                return Err(format!("HTTP {}", self.status));
            }
            if self.body.len() as u64 > maximum_size {
                return Err("response exceeded configured size limit".into());
            }
            destination
                .write_all(&self.body)
                .map_err(|error| error.to_string())?;
            Ok(SourceResponseMetadata {
                status: self.status,
                content_length: Some(self.body.len() as u64),
                content_type: self.content_type.clone(),
                ..Default::default()
            })
        }
    }

    fn fixture(body: Vec<u8>) -> FetchFixture {
        FetchFixture {
            body,
            status: 200,
            content_type: Some("application/json; charset=utf-8".into()),
            failure: None,
        }
    }

    #[test]
    fn explicit_fetch_validates_before_candidate_and_activation() {
        let root = std::env::temp_dir().join(format!(
            "hackhash-fetch-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = HackHashStore::new(root.clone()).unwrap();
        let result = store
            .fetch_candidate(
                "https://example.test/hackhash.json",
                &fixture(bytes(vec![record(&"a".repeat(40), "1.0")])),
            )
            .unwrap();
        assert_eq!(result.state, HackHashFetchState::Changed);
        assert!(store.active_snapshot().unwrap().is_none());
        let preview = store.preview_activation(&result.import).unwrap();
        assert!(preview.changed);
        assert!(
            !result
                .import
                .index
                .evidence_for_patched_sha1(&result.import.validation.export, &"a".repeat(40))[0]
                .native_verified
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn identical_fetch_is_content_addressed_and_keeps_activation_explicit() {
        let root = std::env::temp_dir().join(format!(
            "hackhash-dedup-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = HackHashStore::new(root.clone()).unwrap();
        let body = bytes(vec![record(&"a".repeat(40), "1.0")]);
        let first = store
            .fetch_candidate("https://example.test/hackhash.json", &fixture(body.clone()))
            .unwrap();
        let first_hash = first.import.candidate.snapshot.sha256.clone();
        store.activate_snapshot(&first.import, None).unwrap();
        let second = store
            .fetch_candidate("https://example.test/hackhash.json", &fixture(body))
            .unwrap();
        assert_eq!(second.state, HackHashFetchState::AlreadyCurrent);
        assert_eq!(second.import.candidate.snapshot.sha256, first_hash);
        assert_eq!(store.list_snapshots().unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_http_oversized_and_timeout_failures_preserve_active_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "hackhash-failure-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = HackHashStore::new(root.clone()).unwrap();
        let active = store
            .import_bytes(&bytes(vec![record(&"a".repeat(40), "1.0")]))
            .unwrap();
        let active = store.activate_snapshot(&active, None).unwrap().active;
        assert!(
            store
                .fetch_candidate("https://example.test/x", &fixture(b"{truncated".to_vec()))
                .is_err()
        );
        assert!(
            store
                .fetch_candidate(
                    "https://example.test/x",
                    &FetchFixture {
                        body: vec![b'x'; (HACKHASH_MAX_EXPORT_BYTES + 1) as usize],
                        status: 200,
                        content_type: Some("application/json".into()),
                        failure: None,
                    }
                )
                .is_err()
        );
        assert!(
            store
                .fetch_candidate(
                    "https://example.test/x",
                    &FetchFixture {
                        body: Vec::new(),
                        status: 503,
                        content_type: None,
                        failure: None,
                    }
                )
                .is_err()
        );
        assert!(
            store
                .fetch_candidate(
                    "https://example.test/x",
                    &FetchFixture {
                        body: Vec::new(),
                        status: 200,
                        content_type: None,
                        failure: Some("timeout".into()),
                    }
                )
                .is_err()
        );
        assert_eq!(
            store.active_snapshot().unwrap().unwrap().sha256,
            active.sha256
        );
        let _ = fs::remove_dir_all(root);
    }
}
