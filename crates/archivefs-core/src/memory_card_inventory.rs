//! Read-only PS1/PS2 memory-card inventory.
//!
//! The card image is always the shared preservation unit. Entries below are
//! observations only; this module never extracts, rewrites, repairs, or
//! attributes a complete card to one game.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const PS1_CARD_BYTES: usize = 128 * 1024;
pub const PS1_SLOT_COUNT: usize = 15;
pub const PS1_HEADER_BYTES: usize = 128;
pub const PS1_BLOCK_BYTES: usize = 8192;
pub const PS2_MAX_CARD_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_TITLE_BYTES: usize = 64;
pub const PS2_PAGE_DATA_BYTES: usize = 512;
pub const PS2_SPARE_BYTES: usize = 16;
pub const PS2_PHYSICAL_PAGE_BYTES: usize = PS2_PAGE_DATA_BYTES + PS2_SPARE_BYTES;
pub const PS2_SUPERBLOCK_BYTES: usize = 340;
pub const PS2_DIRECTORY_ENTRY_BYTES: usize = 512;
pub const PS2_NAME_BYTES: usize = 32;
pub const PS2_MAX_DIRECTORY_DEPTH: usize = 8;
pub const PS2_MAX_INVENTORY_ENTRIES: usize = 16_384;
pub const PS2_PSU_CLUSTER_BYTES: u64 = 1024;
pub const PS2_PSU_MAX_FILES: usize = PS2_MAX_INVENTORY_ENTRIES;
pub const PS2_PSU_MAX_OUTPUT_BYTES: u64 = PS2_MAX_CARD_BYTES as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Ps2PageRepresentation {
    RawDataOnly,
    RawWithSpare,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2Geometry {
    pub file_size_bytes: u64,
    pub page_stride_bytes: u32,
    pub page_data_bytes: u32,
    pub spare_bytes: u32,
    pub pages_per_cluster: u32,
    pub pages_per_block: u32,
    pub clusters_per_card: u32,
    pub alloc_offset: u32,
    pub alloc_end: u32,
    pub rootdir_cluster: u32,
    pub backup_block1: u32,
    pub backup_block2: u32,
    pub superblock_offset: u64,
    pub version: Option<String>,
    /// The two bytes at 0x150/0x151 are preserved without assigning the
    /// disputed card-type/flags names from conflicting documentation.
    pub marker_bytes: [u8; 2],
    pub representation: Ps2PageRepresentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Ps2DirectoryEntryKind {
    Directory,
    RegularFile,
    Special,
    Unused,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Ps2CorruptionKind {
    FatLoop,
    ClusterOutOfRange,
    InvalidFatReference,
    InvalidDirectoryEntry,
    DirectoryTooDeep,
    TooManyEntries,
    InvalidFilename,
    InvalidTimestamp,
    FileSizeExceedsChain,
    TruncatedCard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2InventoryWarning {
    pub kind: Ps2CorruptionKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2Timestamp {
    pub raw: [u8; 8],
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub timezone: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2DirectoryEntry {
    pub raw_entry_offset: u64,
    pub raw_mode: u32,
    pub kind: Ps2DirectoryEntryKind,
    pub raw_name: Vec<u8>,
    pub display_name: String,
    pub length: u32,
    pub start_cluster: u32,
    pub parent_entry: u32,
    pub attributes: u32,
    pub created: Option<Ps2Timestamp>,
    pub modified: Option<Ps2Timestamp>,
    pub warnings: Vec<Ps2InventoryWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2ClusterChainHealth {
    pub clusters: Vec<u32>,
    pub complete: bool,
    pub warnings: Vec<Ps2InventoryWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2SaveFile {
    pub entry: Ps2DirectoryEntry,
    pub declared_size_bytes: u64,
    pub chain_health: Ps2ClusterChainHealth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2SaveDirectory {
    pub entry: Ps2DirectoryEntry,
    pub chain_health: Ps2ClusterChainHealth,
    pub children: Vec<Ps2DirectoryEntry>,
    pub files: Vec<Ps2SaveFile>,
    pub warnings: Vec<Ps2InventoryWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2MemoryCardInventory {
    pub root_chain_health: Ps2ClusterChainHealth,
    pub root_entries: Vec<Ps2DirectoryEntry>,
    pub save_directories: Vec<Ps2SaveDirectory>,
    pub warnings: Vec<Ps2InventoryWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardFormat {
    Ps1Raw,
    Ps2,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardFormatConfidence {
    ConfirmedFormat,
    LikelyFormat,
    UnknownFormat,
    Malformed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardHealth {
    Healthy,
    StructuralWarning,
    Malformed,
    Truncated,
    UnsupportedVariant,
    CorruptionSuspected,
    OutOfRangeMetadata,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryCardIdentityConfidence {
    ConfirmedIdentity,
    SupportingEvidence,
    HeuristicOnly,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCardEntryIdentity {
    pub product_code: Option<String>,
    pub display_title: Option<String>,
    pub confidence: MemoryCardIdentityConfidence,
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCardEntry {
    pub slot: Option<u32>,
    pub name: String,
    pub size_bytes: u64,
    pub used_blocks: Option<u32>,
    pub identity: MemoryCardEntryIdentity,
    pub deleted: bool,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCardInventory {
    pub path: PathBuf,
    pub format: MemoryCardFormat,
    pub format_confidence: MemoryCardFormatConfidence,
    pub health: MemoryCardHealth,
    pub card_size_bytes: u64,
    pub entries: Vec<MemoryCardEntry>,
    pub used_blocks: Option<u32>,
    pub free_blocks: Option<u32>,
    pub warnings: Vec<String>,
    pub shared_container: bool,
    pub ps2_geometry: Option<Ps2Geometry>,
    pub ps2_inventory: Option<Ps2MemoryCardInventory>,
}

/// Immutable review evidence for exporting one PS2 regular file.
///
/// The card remains the preservation unit. This plan only authorises a new
/// destination file and never represents a card or whole-save mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2FileExportPlan {
    pub source_card_path: PathBuf,
    pub source_card_sha256: String,
    pub file_raw_entry_offset: u64,
    pub raw_name: Vec<u8>,
    pub display_name: String,
    pub declared_size_bytes: u64,
    pub chain: Vec<u32>,
    pub cluster_data_bytes: u64,
    pub destination: PathBuf,
    pub overwrite: bool,
    pub warnings: Vec<String>,
    pub provenance: String,
    geometry: Ps2Geometry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ps2FileExportError {
    InvalidPlan(String),
    SourceChanged,
    DestinationExists(PathBuf),
    UnsafeDestination(PathBuf),
    Io(String),
}

impl std::fmt::Display for Ps2FileExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlan(detail) => {
                write!(formatter, "invalid PS2 file export plan: {detail}")
            }
            Self::SourceChanged => write!(formatter, "PS2 memory-card source changed after review"),
            Self::DestinationExists(path) => {
                write!(
                    formatter,
                    "export destination already exists: {}",
                    path.display()
                )
            }
            Self::UnsafeDestination(path) => {
                write!(
                    formatter,
                    "export destination is unsafe: {}",
                    path.display()
                )
            }
            Self::Io(detail) => write!(formatter, "PS2 file export I/O failed: {detail}"),
        }
    }
}

impl std::error::Error for Ps2FileExportError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2FileExportResult {
    pub destination: PathBuf,
    pub bytes_written: u64,
    pub sha256: String,
    pub source_card_path: PathBuf,
    pub source_card_sha256: String,
    pub provenance: String,
}

/// Immutable review evidence for exporting one validated, single-level PS2
/// save directory as an EMS/uLaunchELF PSU container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2PsuExportPlan {
    pub source_card_path: PathBuf,
    pub source_card_sha256: String,
    pub save_raw_name: Vec<u8>,
    pub save_display_name: String,
    save_entry: Ps2DirectoryEntry,
    pub files: Vec<Ps2SaveFile>,
    pub destination: PathBuf,
    pub expected_output_bytes: u64,
    pub provenance: String,
    geometry: Ps2Geometry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ps2PsuExportError {
    InvalidPlan(String),
    SourceChanged,
    DestinationExists(PathBuf),
    UnsafeDestination(PathBuf),
    Io(String),
}

impl std::fmt::Display for Ps2PsuExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlan(detail) => write!(formatter, "invalid PS2 PSU export plan: {detail}"),
            Self::SourceChanged => write!(formatter, "PS2 memory-card source changed after review"),
            Self::DestinationExists(path) => {
                write!(
                    formatter,
                    "PSU export destination already exists: {}",
                    path.display()
                )
            }
            Self::UnsafeDestination(path) => {
                write!(
                    formatter,
                    "PSU export destination is unsafe: {}",
                    path.display()
                )
            }
            Self::Io(detail) => write!(formatter, "PS2 PSU export I/O failed: {detail}"),
        }
    }
}

impl std::error::Error for Ps2PsuExportError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2PsuExportResult {
    pub destination: PathBuf,
    pub output_bytes: u64,
    pub sha256: String,
    pub file_count: usize,
    pub source_card_path: PathBuf,
    pub source_card_sha256: String,
    pub provenance: String,
}

/// Builds a create-only PSU plan from one already-inspected save directory.
/// PSU export is intentionally bounded to the direct regular files represented
/// by the current inventory; nested directories and incomplete evidence fail.
pub fn plan_ps2_psu_export(
    card: &MemoryCardInventory,
    save: &Ps2SaveDirectory,
    destination: &Path,
) -> Result<Ps2PsuExportPlan, Ps2PsuExportError> {
    let geometry = card
        .ps2_geometry
        .clone()
        .ok_or_else(|| Ps2PsuExportError::InvalidPlan("PS2 geometry is unavailable".into()))?;
    if card.format != MemoryCardFormat::Ps2 || !card.shared_container {
        return Err(Ps2PsuExportError::InvalidPlan(
            "source is not a shared PS2 memory-card container".into(),
        ));
    }
    if card.ps2_inventory.is_none() {
        return Err(Ps2PsuExportError::InvalidPlan(
            "source PS2 inventory is unavailable".into(),
        ));
    }
    if !save.warnings.is_empty() || !save.entry.warnings.is_empty() {
        return Err(Ps2PsuExportError::InvalidPlan(
            "save directory metadata or structure has warnings".into(),
        ));
    }
    if !save.chain_health.complete || !save.chain_health.warnings.is_empty() {
        return Err(Ps2PsuExportError::InvalidPlan(
            "save directory FAT chain is not complete".into(),
        ));
    }
    if save
        .children
        .iter()
        .any(|entry| entry.kind != Ps2DirectoryEntryKind::Unused)
    {
        return Err(Ps2PsuExportError::InvalidPlan(
            "nested or non-file save entries are not supported by this PSU writer".into(),
        ));
    }
    validate_psu_name(&save.entry.raw_name)?;
    if save.entry.created.is_none() || save.entry.modified.is_none() {
        return Err(Ps2PsuExportError::InvalidPlan(
            "PSU save directory timestamps are unavailable".into(),
        ));
    }
    if save.files.len() > PS2_PSU_MAX_FILES {
        return Err(Ps2PsuExportError::InvalidPlan(
            "save contains too many files".into(),
        ));
    }
    let mut expected_output_bytes =
        (PS2_DIRECTORY_ENTRY_BYTES as u64)
            .checked_mul((save.files.len() as u64).checked_add(3).ok_or_else(|| {
                Ps2PsuExportError::InvalidPlan("PSU entry count overflowed".into())
            })?)
            .ok_or_else(|| Ps2PsuExportError::InvalidPlan("PSU header size overflowed".into()))?;
    let mut names = HashSet::new();
    for file in &save.files {
        validate_psu_file(file)?;
        let name = psu_name_bytes(&file.entry.raw_name)?;
        if !names.insert(name.to_vec()) {
            return Err(Ps2PsuExportError::InvalidPlan(
                "PSU save contains duplicate file names".into(),
            ));
        }
        let padded = file
            .declared_size_bytes
            .checked_add(PS2_PSU_CLUSTER_BYTES - 1)
            .ok_or_else(|| Ps2PsuExportError::InvalidPlan("PSU file padding overflowed".into()))?
            / PS2_PSU_CLUSTER_BYTES
            * PS2_PSU_CLUSTER_BYTES;
        expected_output_bytes = expected_output_bytes
            .checked_add(padded)
            .ok_or_else(|| Ps2PsuExportError::InvalidPlan("PSU output size overflowed".into()))?;
    }
    if expected_output_bytes > PS2_PSU_MAX_OUTPUT_BYTES {
        return Err(Ps2PsuExportError::InvalidPlan(
            "PSU output exceeds the bounded export limit".into(),
        ));
    }
    validate_source_path(&card.path).map_err(psu_error)?;
    validate_destination_path(destination, &card.path).map_err(psu_error)?;
    let source_card_sha256 = sha256_hex(&read_source_card(&card.path).map_err(psu_error)?);
    Ok(Ps2PsuExportPlan {
        source_card_path: card.path.clone(),
        source_card_sha256,
        save_raw_name: save.entry.raw_name.clone(),
        save_display_name: save.entry.display_name.clone(),
        save_entry: save.entry.clone(),
        files: save.files.clone(),
        destination: destination.to_path_buf(),
        expected_output_bytes,
        provenance: "PS2 validated save directory; PSU 512-byte entries, 1024-byte logical-file padding, data pages only".into(),
        geometry,
    })
}

/// Applies a reviewed PSU plan without modifying the source card.
pub fn apply_ps2_psu_export(
    plan: &Ps2PsuExportPlan,
) -> Result<Ps2PsuExportResult, Ps2PsuExportError> {
    validate_source_path(&plan.source_card_path).map_err(psu_error)?;
    validate_destination_path(&plan.destination, &plan.source_card_path).map_err(psu_error)?;
    let source_bytes = read_source_card(&plan.source_card_path).map_err(psu_error)?;
    if sha256_hex(&source_bytes) != plan.source_card_sha256 {
        return Err(Ps2PsuExportError::SourceChanged);
    }
    let mut temporary = create_export_temporary(&plan.destination).map_err(psu_error)?;
    (|| {
        let file_count = plan.files.len();
        write_psu_entry(
            &mut temporary,
            &plan.save_raw_name,
            plan.save_entry.raw_mode,
            (file_count as u32).checked_add(2).ok_or_else(|| {
                Ps2PsuExportError::InvalidPlan("PSU entry count overflowed".into())
            })?,
            &plan.save_entry,
            true,
        )?;
        write_psu_dot_entry(&mut temporary, b".", &plan.save_entry)?;
        write_psu_dot_entry(&mut temporary, b"..", &plan.save_entry)?;
        for file in &plan.files {
            write_psu_entry(
                &mut temporary,
                &file.entry.raw_name,
                file.entry.raw_mode,
                file.declared_size_bytes as u32,
                &file.entry,
                false,
            )?;
        }
        for file in &plan.files {
            let mut remaining = file.declared_size_bytes;
            for &cluster in &file.chain_health.clusters {
                if remaining == 0 {
                    break;
                }
                let data = ps2_relative_cluster(&source_bytes, &plan.geometry, cluster)
                    .ok_or_else(|| {
                        Ps2PsuExportError::InvalidPlan("FAT chain points outside card data".into())
                    })?;
                let count = remaining.min(data.len() as u64) as usize;
                temporary
                    .write_all(&data[..count])
                    .map_err(|error| Ps2PsuExportError::Io(error.to_string()))?;
                remaining -= count as u64;
            }
            if remaining != 0 {
                return Err(Ps2PsuExportError::InvalidPlan(
                    "validated chain did not provide the declared logical length".into(),
                ));
            }
            let padding = (PS2_PSU_CLUSTER_BYTES
                - (file.declared_size_bytes % PS2_PSU_CLUSTER_BYTES))
                % PS2_PSU_CLUSTER_BYTES;
            temporary
                .write_all(&vec![0; padding as usize])
                .map_err(|error| Ps2PsuExportError::Io(error.to_string()))?;
        }
        temporary
            .sync_all()
            .map_err(|error| Ps2PsuExportError::Io(error.to_string()))?;
        let bytes_written = temporary
            .metadata()
            .map_err(|error| Ps2PsuExportError::Io(error.to_string()))?
            .len();
        if bytes_written != plan.expected_output_bytes {
            return Err(Ps2PsuExportError::InvalidPlan(
                "staged PSU length did not match the reviewed output size".into(),
            ));
        }
        let output_sha256 = sha256_hex(&read_path(temporary.path())?);
        if sha256_hex(&read_source_card(&plan.source_card_path).map_err(psu_error)?)
            != plan.source_card_sha256
        {
            return Err(Ps2PsuExportError::SourceChanged);
        }
        if fs::symlink_metadata(&plan.destination).is_ok() {
            return Err(Ps2PsuExportError::DestinationExists(
                plan.destination.clone(),
            ));
        }
        fs::hard_link(temporary.path(), &plan.destination)
            .map_err(|error| Ps2PsuExportError::Io(error.to_string()))?;
        let final_bytes = read_path(&plan.destination)?;
        if final_bytes.len() as u64 != bytes_written || sha256_hex(&final_bytes) != output_sha256 {
            return Err(Ps2PsuExportError::Io(
                "published PSU failed output verification".into(),
            ));
        }
        if sha256_hex(&read_source_card(&plan.source_card_path).map_err(psu_error)?)
            != plan.source_card_sha256
        {
            return Err(Ps2PsuExportError::SourceChanged);
        }
        fs::remove_file(temporary.path())
            .map_err(|error| Ps2PsuExportError::Io(error.to_string()))?;
        Ok(Ps2PsuExportResult {
            destination: plan.destination.clone(),
            output_bytes: bytes_written,
            sha256: output_sha256,
            file_count,
            source_card_path: plan.source_card_path.clone(),
            source_card_sha256: plan.source_card_sha256.clone(),
            provenance: plan.provenance.clone(),
        })
    })()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2PsuRestorePlan {
    pub source_card_path: PathBuf,
    pub source_psu_path: PathBuf,
    pub source_psu_sha256: String,
    pub target_card_sha256: String,
    pub target_card_size_bytes: u64,
    pub save_raw_name: Vec<u8>,
    pub save_display_name: String,
    pub file_count: usize,
    pub required_clusters: u32,
    pub free_clusters: u32,
    pub backup_path: PathBuf,
    pub existing_save: bool,
    pub replace_existing: bool,
    #[serde(skip)]
    parsed: Ps2PsuRestoreInput,
    #[serde(skip)]
    geometry: Ps2Geometry,
    #[serde(skip)]
    existing_entry_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Ps2PsuRestoreInput {
    root: Vec<u8>,
    dot: Vec<u8>,
    dotdot: Vec<u8>,
    files: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ps2PsuRestoreResult {
    pub source_card_path: PathBuf,
    pub source_psu_path: PathBuf,
    pub backup_path: PathBuf,
    pub backup_sha256: String,
    pub original_card_sha256: String,
    pub post_restore_card_sha256: String,
    pub card_size_bytes: u64,
    pub save_display_name: String,
    pub file_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ps2PsuRestoreError {
    InvalidPlan(String),
    SourceChanged,
    CardChanged,
    ExistingSave(String),
    InsufficientSpace { required: u32, available: u32 },
    BackupExists(PathBuf),
    BackupVerificationFailed,
    VerificationFailed(String),
    StaleUndo,
    Io(String),
}

impl std::fmt::Display for Ps2PsuRestoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlan(detail) => write!(formatter, "invalid PS2 restore plan: {detail}"),
            Self::SourceChanged => write!(formatter, "PSU source changed after review"),
            Self::CardChanged => write!(formatter, "memory card changed after review"),
            Self::ExistingSave(name) => {
                write!(formatter, "save already exists on the card: {name}")
            }
            Self::InsufficientSpace {
                required,
                available,
            } => write!(
                formatter,
                "not enough free memory-card space: requires {required} clusters, only {available} available"
            ),
            Self::BackupExists(path) => {
                write!(formatter, "backup already exists: {}", path.display())
            }
            Self::BackupVerificationFailed => {
                write!(formatter, "memory-card backup failed verification")
            }
            Self::VerificationFailed(detail) => write!(
                formatter,
                "restored memory card failed verification: {detail}"
            ),
            Self::StaleUndo => write!(
                formatter,
                "undo refused because the card changed after restore"
            ),
            Self::Io(detail) => write!(formatter, "PS2 restore I/O failed: {detail}"),
        }
    }
}

impl std::error::Error for Ps2PsuRestoreError {}

fn restore_error(error: impl std::fmt::Display) -> Ps2PsuRestoreError {
    Ps2PsuRestoreError::Io(error.to_string())
}

fn validate_restore_path(path: &Path, card: Option<&Path>) -> Result<(), Ps2PsuRestoreError> {
    let metadata = fs::symlink_metadata(path).map_err(restore_error)?;
    if !path.is_absolute() || !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "path must be an absolute regular non-symlink file".into(),
        ));
    }
    validate_restore_parent(path)?;
    if card.is_some_and(|card| path == card) {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "restore source and target card must be different files".into(),
        ));
    }
    Ok(())
}

fn validate_restore_parent(path: &Path) -> Result<(), Ps2PsuRestoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("path has no parent directory".into()))?;
    let mut current = PathBuf::from(std::path::MAIN_SEPARATOR.to_string());
    for component in parent.components() {
        if matches!(component, std::path::Component::RootDir) {
            continue;
        }
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(restore_error)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Ps2PsuRestoreError::InvalidPlan(
                "path parent contains a non-directory or symlink component".into(),
            ));
        }
    }
    Ok(())
}

fn validate_backup_path(path: &Path, card: &Path) -> Result<(), Ps2PsuRestoreError> {
    if !path.is_absolute()
        || path == card
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "backup path must be an absolute, separate, normalized path".into(),
        ));
    }
    validate_restore_parent(path)?;
    if fs::symlink_metadata(path).is_ok() {
        return Err(Ps2PsuRestoreError::BackupExists(path.to_path_buf()));
    }
    Ok(())
}

fn parse_ps2_psu(path: &Path) -> Result<(Ps2PsuRestoreInput, String), Ps2PsuRestoreError> {
    validate_restore_path(path, None)?;
    let metadata = fs::metadata(path).map_err(restore_error)?;
    if metadata.len() > PS2_PSU_MAX_OUTPUT_BYTES
        || metadata.len() < (PS2_DIRECTORY_ENTRY_BYTES * 4) as u64
    {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "PSU size is outside the bounded restore range".into(),
        ));
    }
    let bytes = fs::read(path).map_err(restore_error)?;
    let source_sha256 = sha256_hex(&bytes);
    let entry = |index: usize| -> Result<Vec<u8>, Ps2PsuRestoreError> {
        let start = index
            .checked_mul(PS2_DIRECTORY_ENTRY_BYTES)
            .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("PSU entry offset overflowed".into()))?;
        let end = start
            .checked_add(PS2_DIRECTORY_ENTRY_BYTES)
            .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("PSU entry range overflowed".into()))?;
        bytes
            .get(start..end)
            .map(|value| value.to_vec())
            .ok_or_else(|| {
                Ps2PsuRestoreError::InvalidPlan("PSU directory entries are truncated".into())
            })
    };
    let root = entry(0)?;
    let root_entry = ps2_entry(&root, 0);
    if root_entry.kind != Ps2DirectoryEntryKind::Directory || root_entry.length < 2 {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "PSU root entry is not a valid directory".into(),
        ));
    }
    let count = root_entry.length as usize;
    if count > PS2_PSU_MAX_FILES + 2 {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "PSU contains too many entries".into(),
        ));
    }
    let dot = entry(1)?;
    let dotdot = entry(2)?;
    for (raw, expected) in [(&dot, "."), (&dotdot, "..")] {
        let parsed = ps2_entry(raw, 0);
        if parsed.kind != Ps2DirectoryEntryKind::Directory || parsed.display_name != expected {
            return Err(Ps2PsuRestoreError::InvalidPlan(format!(
                "PSU is missing the {expected} directory entry"
            )));
        }
    }
    let mut files = Vec::new();
    let mut names = HashSet::new();
    let mut data_offset = (count + 1)
        .checked_mul(PS2_DIRECTORY_ENTRY_BYTES)
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("PSU data offset overflowed".into()))?;
    for index in 3..=count {
        let raw = entry(index)?;
        let parsed = ps2_entry(&raw, 0);
        if parsed.kind != Ps2DirectoryEntryKind::RegularFile || !parsed.warnings.is_empty() {
            return Err(Ps2PsuRestoreError::InvalidPlan(
                "PSU contains an invalid or unsupported file entry".into(),
            ));
        }
        let name = psu_name_bytes(&parsed.raw_name)
            .map_err(|error| Ps2PsuRestoreError::InvalidPlan(error.to_string()))?;
        if !names.insert(name.to_vec()) {
            return Err(Ps2PsuRestoreError::InvalidPlan(
                "PSU contains duplicate file names".into(),
            ));
        }
        let length = parsed.length as usize;
        let padded =
            length.div_ceil(PS2_PSU_CLUSTER_BYTES as usize) * PS2_PSU_CLUSTER_BYTES as usize;
        let end = data_offset
            .checked_add(padded)
            .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("PSU data range overflowed".into()))?;
        let data_end = data_offset
            .checked_add(length)
            .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("PSU file range overflowed".into()))?;
        if end > bytes.len() {
            return Err(Ps2PsuRestoreError::InvalidPlan(
                "PSU file data is truncated".into(),
            ));
        }
        files.push((raw, bytes[data_offset..data_end].to_vec()));
        data_offset = end;
    }
    if data_offset != bytes.len() {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "PSU contains unexpected trailing bytes".into(),
        ));
    }
    Ok((
        Ps2PsuRestoreInput {
            root,
            dot,
            dotdot,
            files,
        },
        source_sha256,
    ))
}

fn restore_chain_set(occupied: &mut HashSet<u32>, chain: &Ps2ClusterChainHealth) {
    occupied.extend(chain.clusters.iter().copied());
}

fn restore_occupied_clusters(
    inventory: &Ps2MemoryCardInventory,
    replacing: Option<&Ps2SaveDirectory>,
) -> HashSet<u32> {
    let mut occupied = HashSet::new();
    restore_chain_set(&mut occupied, &inventory.root_chain_health);
    for entry in &inventory.root_entries {
        if entry.kind != Ps2DirectoryEntryKind::Directory
            || entry.display_name == "."
            || entry.display_name == ".."
            || replacing.is_some_and(|save| save.entry.raw_entry_offset == entry.raw_entry_offset)
        {
            continue;
        }
        if let Some(save) = inventory
            .save_directories
            .iter()
            .find(|save| save.entry.raw_entry_offset == entry.raw_entry_offset)
        {
            restore_chain_set(&mut occupied, &save.chain_health);
            for file in &save.files {
                restore_chain_set(&mut occupied, &file.chain_health);
            }
        }
    }
    occupied
}

fn restore_write_logical_cluster(
    bytes: &mut [u8],
    geometry: &Ps2Geometry,
    absolute: u32,
    data: &[u8],
) -> Result<(), Ps2PsuRestoreError> {
    let cluster_bytes = ps2_cluster_bytes(geometry);
    if data.len() != cluster_bytes || absolute >= geometry.clusters_per_card {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "logical cluster write is outside card geometry".into(),
        ));
    }
    let stride = geometry.page_stride_bytes as usize;
    let page_data = geometry.page_data_bytes as usize;
    let first_page = absolute as usize * geometry.pages_per_cluster as usize;
    for page in 0..geometry.pages_per_cluster as usize {
        let source = page * page_data;
        let offset = (first_page + page) * stride;
        let target = bytes.get_mut(offset..offset + page_data).ok_or_else(|| {
            Ps2PsuRestoreError::InvalidPlan("logical cluster write is truncated".into())
        })?;
        target.copy_from_slice(&data[source..source + page_data]);
    }
    Ok(())
}

fn restore_set_fat(
    bytes: &mut [u8],
    geometry: &Ps2Geometry,
    relative: u32,
    value: u32,
) -> Result<(), Ps2PsuRestoreError> {
    let available = geometry.alloc_end.saturating_sub(geometry.alloc_offset);
    if relative >= available {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "FAT update is outside allocation area".into(),
        ));
    }
    let entries_per_cluster = ps2_cluster_bytes(geometry) / 4;
    let indirect_index = relative as usize / entries_per_cluster;
    let indirect_slot = indirect_index / entries_per_cluster;
    let ifc_offset = 0x50usize + indirect_slot * 4;
    let ifc_cluster = le_u32(bytes, ifc_offset)
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("FAT index is unavailable".into()))?;
    let fat_cluster = ps2_logical_cluster(bytes, geometry, ifc_cluster)
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("FAT cluster is unavailable".into()))?;
    let fat_offset = (relative as usize % entries_per_cluster) * 4;
    let _ = fat_cluster
        .get(fat_offset..fat_offset + 4)
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("FAT entry is unavailable".into()))?;
    let mut updated = fat_cluster;
    updated[fat_offset..fat_offset + 4].copy_from_slice(&value.to_le_bytes());
    restore_write_logical_cluster(bytes, geometry, ifc_cluster, &updated)
}

fn restore_set_entry(
    bytes: &mut [u8],
    geometry: &Ps2Geometry,
    offset: u64,
    raw: &[u8],
) -> Result<(), Ps2PsuRestoreError> {
    if raw.len() != PS2_DIRECTORY_ENTRY_BYTES {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "directory entry has the wrong size".into(),
        ));
    }
    let cluster_bytes = ps2_cluster_bytes(geometry) as u64;
    let absolute = offset / cluster_bytes;
    let within = (offset % cluster_bytes) as usize;
    let mut cluster = ps2_logical_cluster(bytes, geometry, absolute as u32).ok_or_else(|| {
        Ps2PsuRestoreError::InvalidPlan("directory cluster is unavailable".into())
    })?;
    let end = within + raw.len();
    if end > cluster.len() {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "directory entry crosses cluster boundary".into(),
        ));
    }
    cluster[within..end].copy_from_slice(raw);
    restore_write_logical_cluster(bytes, geometry, absolute as u32, &cluster)
}

fn restore_set_entry_u32(raw: &mut [u8], offset: usize, value: u32) {
    raw[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn restore_link_chain(
    bytes: &mut [u8],
    geometry: &Ps2Geometry,
    chain: &[u32],
) -> Result<(), Ps2PsuRestoreError> {
    for (index, &cluster) in chain.iter().enumerate() {
        let value = chain
            .get(index + 1)
            .map_or(0x7fff_ffff, |next| 0x8000_0000 | *next);
        restore_set_fat(bytes, geometry, cluster, value)?;
    }
    Ok(())
}

fn restore_write_card_atomically(path: &Path, bytes: &[u8]) -> Result<(), Ps2PsuRestoreError> {
    validate_restore_path(path, None)?;
    let parent = path
        .parent()
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("card has no parent".into()))?;
    let temporary = create_restore_temporary(parent)?;
    if let Err(error) = fs::write(&temporary, bytes) {
        let _ = fs::remove_file(&temporary);
        return Err(restore_error(error));
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temporary)
        .map_err(restore_error)?;
    if let Err(error) = file.sync_all() {
        let _ = fs::remove_file(&temporary);
        return Err(restore_error(error));
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(restore_error(error));
    }
    Ok(())
}

fn create_restore_temporary(parent: &Path) -> Result<PathBuf, Ps2PsuRestoreError> {
    for number in 0..32u32 {
        let path = parent.join(format!(
            ".emuwiz-ps2-restore-{}-{number}.tmp",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(restore_error(error)),
        }
    }
    Err(Ps2PsuRestoreError::Io(
        "could not allocate an owned restore temporary".into(),
    ))
}

pub fn plan_ps2_psu_restore(
    card: &MemoryCardInventory,
    source_psu: &Path,
    backup_path: &Path,
    replace_existing: bool,
) -> Result<Ps2PsuRestorePlan, Ps2PsuRestoreError> {
    if card.format != MemoryCardFormat::Ps2
        || !card.shared_container
        || card.health != MemoryCardHealth::Healthy
    {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "only a healthy, supported PS2 card can be modified".into(),
        ));
    }
    let geometry = card.ps2_geometry.clone().ok_or_else(|| {
        Ps2PsuRestoreError::InvalidPlan("PS2 card geometry is unavailable".into())
    })?;
    let inventory = card.ps2_inventory.as_ref().ok_or_else(|| {
        Ps2PsuRestoreError::InvalidPlan("PS2 card inventory is unavailable".into())
    })?;
    validate_source_path(&card.path)
        .map_err(|error| Ps2PsuRestoreError::InvalidPlan(error.to_string()))?;
    validate_backup_path(backup_path, &card.path)?;
    let (parsed, source_psu_sha256) = parse_ps2_psu(source_psu)?;
    let root_entry = ps2_entry(&parsed.root, 0);
    let save_name = psu_name_bytes(&root_entry.raw_name)
        .map_err(|error| Ps2PsuRestoreError::InvalidPlan(error.to_string()))?
        .to_vec();
    let existing = inventory
        .save_directories
        .iter()
        .find(|save| psu_name_bytes(&save.entry.raw_name).ok() == Some(save_name.as_slice()));
    if existing.is_some() && !replace_existing {
        return Err(Ps2PsuRestoreError::ExistingSave(root_entry.display_name));
    }
    let occupied = restore_occupied_clusters(inventory, existing);
    let available = geometry.alloc_end.saturating_sub(geometry.alloc_offset);
    let records = parsed.files.len() + 2;
    let cluster_bytes = ps2_cluster_bytes(&geometry) as u64;
    let directory_clusters =
        (records as u64 * PS2_DIRECTORY_ENTRY_BYTES as u64).div_ceil(cluster_bytes);
    let file_clusters = parsed
        .files
        .iter()
        .map(|(_, data)| (data.len() as u64).div_ceil(cluster_bytes))
        .sum::<u64>();
    let root_capacity = inventory.root_chain_health.clusters.len() as u64
        * (cluster_bytes as usize / PS2_DIRECTORY_ENTRY_BYTES) as u64;
    let root_needs_cluster =
        existing.is_none() && inventory.root_entries.len() as u64 >= root_capacity;
    let required_clusters = directory_clusters + file_clusters + u64::from(root_needs_cluster);
    let free_clusters = (available as u64).saturating_sub(occupied.len() as u64);
    if required_clusters > free_clusters {
        return Err(Ps2PsuRestoreError::InsufficientSpace {
            required: required_clusters as u32,
            available: free_clusters as u32,
        });
    }
    let card_bytes =
        read_source_card(&card.path).map_err(|error| Ps2PsuRestoreError::Io(error.to_string()))?;
    Ok(Ps2PsuRestorePlan {
        source_card_path: card.path.clone(),
        source_psu_path: source_psu.to_path_buf(),
        source_psu_sha256,
        target_card_sha256: sha256_hex(&card_bytes),
        target_card_size_bytes: card_bytes.len() as u64,
        save_raw_name: save_name,
        save_display_name: root_entry.display_name,
        file_count: parsed.files.len(),
        required_clusters: required_clusters as u32,
        free_clusters: free_clusters as u32,
        backup_path: backup_path.to_path_buf(),
        existing_save: existing.is_some(),
        replace_existing,
        parsed,
        geometry,
        existing_entry_offset: existing.map(|save| save.entry.raw_entry_offset),
    })
}

pub fn apply_ps2_psu_restore(
    plan: &Ps2PsuRestorePlan,
) -> Result<Ps2PsuRestoreResult, Ps2PsuRestoreError> {
    validate_source_path(&plan.source_card_path)
        .map_err(|error| Ps2PsuRestoreError::InvalidPlan(error.to_string()))?;
    validate_backup_path(&plan.backup_path, &plan.source_card_path)?;
    let source_psu_bytes = fs::read(&plan.source_psu_path).map_err(restore_error)?;
    if sha256_hex(&source_psu_bytes) != plan.source_psu_sha256 {
        return Err(Ps2PsuRestoreError::SourceChanged);
    }
    let original = read_source_card(&plan.source_card_path)
        .map_err(|error| Ps2PsuRestoreError::Io(error.to_string()))?;
    if sha256_hex(&original) != plan.target_card_sha256
        || original.len() as u64 != plan.target_card_size_bytes
    {
        return Err(Ps2PsuRestoreError::CardChanged);
    }
    let backup_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&plan.backup_path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ps2PsuRestoreError::BackupExists(plan.backup_path.clone())
            } else {
                restore_error(error)
            }
        })?;
    let mut backup_file = backup_file;
    let backup_write = (|| {
        backup_file.write_all(&original).map_err(restore_error)?;
        backup_file.sync_all().map_err(restore_error)
    })();
    drop(backup_file);
    if let Err(error) = backup_write {
        let _ = fs::remove_file(&plan.backup_path);
        return Err(error);
    }
    let backup = fs::read(&plan.backup_path).map_err(restore_error)?;
    let backup_sha256 = sha256_hex(&backup);
    if backup.len() != original.len() || backup_sha256 != plan.target_card_sha256 {
        let _ = fs::remove_file(&plan.backup_path);
        return Err(Ps2PsuRestoreError::BackupVerificationFailed);
    }

    let result = (|| {
        let card = inspect_memory_card(&plan.source_card_path).map_err(Ps2PsuRestoreError::Io)?;
        let inventory = card.ps2_inventory.as_ref().ok_or_else(|| {
            Ps2PsuRestoreError::InvalidPlan("PS2 card inventory disappeared".into())
        })?;
        let mut bytes = original.clone();
        let replacing = plan.existing_entry_offset.and_then(|offset| {
            inventory
                .save_directories
                .iter()
                .find(|save| save.entry.raw_entry_offset == offset)
        });
        let occupied = restore_occupied_clusters(inventory, replacing);
        let available = plan
            .geometry
            .alloc_end
            .saturating_sub(plan.geometry.alloc_offset);
        let cluster_bytes = ps2_cluster_bytes(&plan.geometry) as u64;
        let records = plan.parsed.files.len() + 2;
        let directory_clusters =
            (records as u64 * PS2_DIRECTORY_ENTRY_BYTES as u64).div_ceil(cluster_bytes) as usize;
        let file_clusters = plan
            .parsed
            .files
            .iter()
            .map(|(_, data)| (data.len() as u64).div_ceil(cluster_bytes) as usize)
            .sum::<usize>();
        let root_capacity = inventory.root_chain_health.clusters.len()
            * (cluster_bytes as usize / PS2_DIRECTORY_ENTRY_BYTES);
        let root_needs_cluster =
            plan.existing_entry_offset.is_none() && inventory.root_entries.len() >= root_capacity;
        let needed = directory_clusters + file_clusters + usize::from(root_needs_cluster);
        let free = (0..available)
            .filter(|cluster| !occupied.contains(cluster))
            .collect::<Vec<_>>();
        if free.len() < needed {
            return Err(Ps2PsuRestoreError::InsufficientSpace {
                required: needed as u32,
                available: free.len() as u32,
            });
        }
        let mut allocation = free.into_iter().take(needed);
        let root_extra = root_needs_cluster.then(|| allocation.next().expect("checked allocation"));
        let directory_chain = (0..directory_clusters)
            .map(|_| allocation.next().expect("checked allocation"))
            .collect::<Vec<_>>();
        let mut file_chains = Vec::new();
        for (_, data) in &plan.parsed.files {
            let count = (data.len() as u64).div_ceil(cluster_bytes) as usize;
            file_chains.push(
                (0..count)
                    .map(|_| allocation.next().expect("checked allocation"))
                    .collect::<Vec<_>>(),
            );
        }
        if let Some(old) = replacing {
            for &cluster in &old.chain_health.clusters {
                restore_set_fat(&mut bytes, &plan.geometry, cluster, 0xffff_ffff)?;
            }
            for file in &old.files {
                for &cluster in &file.chain_health.clusters {
                    restore_set_fat(&mut bytes, &plan.geometry, cluster, 0xffff_ffff)?;
                }
            }
        }
        if let Some(extra) = root_extra {
            let last = *inventory.root_chain_health.clusters.last().ok_or_else(|| {
                Ps2PsuRestoreError::InvalidPlan("root directory chain is empty".into())
            })?;
            restore_set_fat(&mut bytes, &plan.geometry, last, 0x8000_0000 | extra)?;
            restore_set_fat(&mut bytes, &plan.geometry, extra, 0x7fff_ffff)?;
        }
        restore_link_chain(&mut bytes, &plan.geometry, &directory_chain)?;
        for chain in &file_chains {
            restore_link_chain(&mut bytes, &plan.geometry, chain)?;
        }

        let root_slot_offset = if let Some(offset) = plan.existing_entry_offset {
            offset
        } else if let Some(entry) = inventory
            .root_entries
            .iter()
            .find(|entry| entry.kind == Ps2DirectoryEntryKind::Unused)
        {
            entry.raw_entry_offset
        } else {
            let cluster = root_extra.ok_or_else(|| {
                Ps2PsuRestoreError::InvalidPlan("root directory has no free slot".into())
            })?;
            (plan.geometry.alloc_offset as u64 + cluster as u64) * cluster_bytes
                + (root_capacity % (cluster_bytes as usize / PS2_DIRECTORY_ENTRY_BYTES)) as u64
                    * PS2_DIRECTORY_ENTRY_BYTES as u64
        };
        let mut root_raw = plan.parsed.root.clone();
        restore_set_entry_u32(&mut root_raw, 0x10, directory_chain[0]);
        restore_set_entry_u32(&mut root_raw, 0x14, plan.geometry.rootdir_cluster);
        restore_set_entry_u32(&mut root_raw, 4, records as u32);
        restore_set_entry(&mut bytes, &plan.geometry, root_slot_offset, &root_raw)?;
        if plan.existing_entry_offset.is_none() {
            let mut first_raw =
                ps2_logical_cluster(&bytes, &plan.geometry, plan.geometry.rootdir_cluster)
                    .ok_or_else(|| {
                        Ps2PsuRestoreError::InvalidPlan("root directory is unavailable".into())
                    })?;
            let root_count = if root_extra.is_some()
                || !inventory
                    .root_entries
                    .iter()
                    .any(|entry| entry.kind == Ps2DirectoryEntryKind::Unused)
            {
                inventory.root_entries.len() as u32 + 1
            } else {
                inventory.root_entries.len() as u32
            };
            restore_set_entry_u32(&mut first_raw, 4, root_count);
            restore_write_logical_cluster(
                &mut bytes,
                &plan.geometry,
                plan.geometry.rootdir_cluster,
                &first_raw,
            )?;
        }
        let mut directory_bytes = vec![0u8; directory_clusters * cluster_bytes as usize];
        let mut records_raw = vec![plan.parsed.dot.clone(), plan.parsed.dotdot.clone()];
        for ((raw, _), chain) in plan.parsed.files.iter().zip(&file_chains) {
            let mut updated = raw.clone();
            restore_set_entry_u32(
                &mut updated,
                0x10,
                chain.first().copied().unwrap_or(u32::MAX),
            );
            restore_set_entry_u32(&mut updated, 0x14, directory_chain[0]);
            records_raw.push(updated);
        }
        for (index, raw) in records_raw.iter().enumerate() {
            directory_bytes
                [index * PS2_DIRECTORY_ENTRY_BYTES..(index + 1) * PS2_DIRECTORY_ENTRY_BYTES]
                .copy_from_slice(raw);
        }
        for (index, &cluster) in directory_chain.iter().enumerate() {
            restore_write_logical_cluster(
                &mut bytes,
                &plan.geometry,
                plan.geometry.alloc_offset + cluster,
                &directory_bytes
                    [index * cluster_bytes as usize..(index + 1) * cluster_bytes as usize],
            )?;
        }
        for ((_, data), chain) in plan.parsed.files.iter().zip(&file_chains) {
            for (index, &cluster) in chain.iter().enumerate() {
                let mut payload = vec![0u8; cluster_bytes as usize];
                let start = index * cluster_bytes as usize;
                let end = (start + payload.len()).min(data.len());
                if start < end {
                    payload[..end - start].copy_from_slice(&data[start..end]);
                }
                restore_write_logical_cluster(
                    &mut bytes,
                    &plan.geometry,
                    plan.geometry.alloc_offset + cluster,
                    &payload,
                )?;
            }
        }
        restore_write_card_atomically(&plan.source_card_path, &bytes)?;
        let after = inspect_memory_card(&plan.source_card_path).map_err(Ps2PsuRestoreError::Io)?;
        if after.card_size_bytes != plan.target_card_size_bytes
            || after.health != MemoryCardHealth::Healthy
            || !restore_contains_expected(&after, plan)
        {
            restore_write_card_atomically(&plan.source_card_path, &original)?;
            return Err(Ps2PsuRestoreError::VerificationFailed(
                "card structure or restored save did not match the reviewed package".into(),
            ));
        }
        Ok(Ps2PsuRestoreResult {
            source_card_path: plan.source_card_path.clone(),
            source_psu_path: plan.source_psu_path.clone(),
            backup_path: plan.backup_path.clone(),
            backup_sha256,
            original_card_sha256: plan.target_card_sha256.clone(),
            post_restore_card_sha256: sha256_hex(&bytes),
            card_size_bytes: bytes.len() as u64,
            save_display_name: plan.save_display_name.clone(),
            file_count: plan.file_count,
        })
    })();
    if result.is_err()
        && sha256_hex(&read_source_card(&plan.source_card_path).unwrap_or_default())
            != plan.target_card_sha256
    {
        let _ = restore_write_card_atomically(&plan.source_card_path, &original);
    }
    result
}

fn restore_contains_expected(card: &MemoryCardInventory, plan: &Ps2PsuRestorePlan) -> bool {
    let Some(inventory) = card.ps2_inventory.as_ref() else {
        return false;
    };
    let Some(save) = inventory.save_directories.iter().find(|save| {
        psu_name_bytes(&save.entry.raw_name).ok() == Some(plan.save_raw_name.as_slice())
    }) else {
        return false;
    };
    if save.files.len() != plan.file_count
        || !save.files.iter().all(|file| file.chain_health.complete)
    {
        return false;
    }
    let Ok(bytes) = read_source_card(&plan.source_card_path) else {
        return false;
    };
    save.files.iter().all(|file| {
        plan.parsed
            .files
            .iter()
            .find(|(raw, data)| {
                psu_name_bytes(&ps2_entry(raw, 0).raw_name).ok()
                    == psu_name_bytes(&file.entry.raw_name).ok()
                    && file.declared_size_bytes == data.len() as u64
                    && read_file_chain_bytes(&bytes, &plan.geometry, file)
                        .is_some_and(|actual| actual == *data)
            })
            .is_some()
    })
}

fn read_file_chain_bytes(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    file: &Ps2SaveFile,
) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(file.declared_size_bytes as usize);
    let mut remaining = file.declared_size_bytes as usize;
    for &cluster in &file.chain_health.clusters {
        let data = ps2_relative_cluster(bytes, geometry, cluster)?;
        let count = remaining.min(data.len());
        output.extend_from_slice(&data[..count]);
        remaining -= count;
        if remaining == 0 {
            break;
        }
    }
    (remaining == 0).then_some(output)
}

pub fn undo_ps2_psu_restore(result: &Ps2PsuRestoreResult) -> Result<(), Ps2PsuRestoreError> {
    validate_source_path(&result.source_card_path)
        .map_err(|error| Ps2PsuRestoreError::InvalidPlan(error.to_string()))?;
    validate_restore_parent(&result.backup_path)?;
    let current = read_source_card(&result.source_card_path)
        .map_err(|error| Ps2PsuRestoreError::Io(error.to_string()))?;
    if sha256_hex(&current) != result.post_restore_card_sha256 {
        return Err(Ps2PsuRestoreError::StaleUndo);
    }
    let backup_metadata = fs::symlink_metadata(&result.backup_path).map_err(restore_error)?;
    if !backup_metadata.file_type().is_file() || backup_metadata.file_type().is_symlink() {
        return Err(Ps2PsuRestoreError::BackupVerificationFailed);
    }
    let backup = fs::read(&result.backup_path).map_err(restore_error)?;
    if sha256_hex(&backup) != result.original_card_sha256
        || sha256_hex(&backup) != result.backup_sha256
    {
        return Err(Ps2PsuRestoreError::BackupVerificationFailed);
    }
    restore_write_card_atomically(&result.source_card_path, &backup)?;
    let restored = read_source_card(&result.source_card_path)
        .map_err(|error| Ps2PsuRestoreError::Io(error.to_string()))?;
    if sha256_hex(&restored) != result.original_card_sha256 {
        return Err(Ps2PsuRestoreError::VerificationFailed(
            "undo did not restore the original card bytes".into(),
        ));
    }
    Ok(())
}

fn psu_error(error: Ps2FileExportError) -> Ps2PsuExportError {
    match error {
        Ps2FileExportError::SourceChanged => Ps2PsuExportError::SourceChanged,
        Ps2FileExportError::DestinationExists(path) => Ps2PsuExportError::DestinationExists(path),
        Ps2FileExportError::UnsafeDestination(path) => Ps2PsuExportError::UnsafeDestination(path),
        Ps2FileExportError::InvalidPlan(detail) => Ps2PsuExportError::InvalidPlan(detail),
        Ps2FileExportError::Io(detail) => Ps2PsuExportError::Io(detail),
    }
}

fn validate_psu_name(name: &[u8]) -> Result<(), Ps2PsuExportError> {
    let name = psu_name_bytes(name)?;
    if name.is_empty() || name.len() > PS2_NAME_BYTES {
        return Err(Ps2PsuExportError::InvalidPlan(
            "PSU name is empty, too long, or contains NUL".into(),
        ));
    }
    if name
        .iter()
        .any(|byte| *byte < 0x20 || *byte == b'/' || *byte == b'?' || *byte == b'*')
    {
        return Err(Ps2PsuExportError::InvalidPlan(
            "PSU name contains an unsupported character".into(),
        ));
    }
    Ok(())
}

fn psu_name_bytes(name: &[u8]) -> Result<&[u8], Ps2PsuExportError> {
    let end = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    if name[end..].iter().any(|byte| *byte != 0) {
        return Err(Ps2PsuExportError::InvalidPlan(
            "PSU name has non-zero bytes after NUL".into(),
        ));
    }
    Ok(&name[..end])
}

fn validate_psu_file(file: &Ps2SaveFile) -> Result<(), Ps2PsuExportError> {
    if file.entry.kind != Ps2DirectoryEntryKind::RegularFile
        || !file.entry.warnings.is_empty()
        || !file.chain_health.complete
        || !file.chain_health.warnings.is_empty()
        || u64::from(file.entry.length) != file.declared_size_bytes
        || file.declared_size_bytes > u32::MAX as u64
        || file.entry.raw_mode > u16::MAX as u32
    {
        return Err(Ps2PsuExportError::InvalidPlan(
            "PSU member metadata or chain is unsafe".into(),
        ));
    }
    validate_psu_name(&file.entry.raw_name)
}

fn write_psu_entry(
    output: &mut ExportTemporary,
    name: &[u8],
    raw_mode: u32,
    length: u32,
    metadata: &Ps2DirectoryEntry,
    directory: bool,
) -> Result<(), Ps2PsuExportError> {
    let name = psu_name_bytes(name)?;
    validate_psu_name(name)?;
    if raw_mode > u16::MAX as u32 {
        return Err(Ps2PsuExportError::InvalidPlan(
            "PSU mode exceeds u16".into(),
        ));
    }
    let mut entry = [0u8; PS2_DIRECTORY_ENTRY_BYTES];
    entry[0..2].copy_from_slice(&(raw_mode as u16).to_le_bytes());
    entry[4..8].copy_from_slice(&length.to_le_bytes());
    entry[8..16].copy_from_slice(
        &metadata
            .created
            .as_ref()
            .ok_or_else(|| {
                Ps2PsuExportError::InvalidPlan("PSU creation timestamp is unavailable".into())
            })?
            .raw,
    );
    entry[0x18..0x20].copy_from_slice(
        &metadata
            .modified
            .as_ref()
            .ok_or_else(|| {
                Ps2PsuExportError::InvalidPlan("PSU modification timestamp is unavailable".into())
            })?
            .raw,
    );
    entry[0x20..0x24].copy_from_slice(&metadata.attributes.to_le_bytes());
    entry[0x40..0x40 + name.len()].copy_from_slice(name);
    if directory {
        entry[0x10..0x14].fill(0);
        entry[0x14..0x18].fill(0);
    }
    output
        .write_all(&entry)
        .map_err(|error| Ps2PsuExportError::Io(error.to_string()))
}

fn write_psu_dot_entry(
    output: &mut ExportTemporary,
    name: &[u8],
    metadata: &Ps2DirectoryEntry,
) -> Result<(), Ps2PsuExportError> {
    write_psu_entry(output, name, 0x8427, 0, metadata, true)
}

fn read_path(path: &Path) -> Result<Vec<u8>, Ps2PsuExportError> {
    fs::read(path).map_err(|error| Ps2PsuExportError::Io(error.to_string()))
}

/// Builds an export plan from an already-inspected PS2 regular file.
///
/// The file's validated chain is copied into the plan; the planner does not
/// rediscover the card filesystem or infer a destination filename.
pub fn plan_ps2_file_export(
    card: &MemoryCardInventory,
    file: &Ps2SaveFile,
    destination: &Path,
) -> Result<Ps2FileExportPlan, Ps2FileExportError> {
    let geometry = card
        .ps2_geometry
        .clone()
        .ok_or_else(|| Ps2FileExportError::InvalidPlan("PS2 geometry is unavailable".into()))?;
    if card.ps2_inventory.is_none() {
        return Err(Ps2FileExportError::InvalidPlan(
            "PS2 filesystem inventory is unavailable".into(),
        ));
    }
    if card.format != MemoryCardFormat::Ps2 || !card.shared_container {
        return Err(Ps2FileExportError::InvalidPlan(
            "source is not a shared PS2 memory-card container".into(),
        ));
    }
    if file.entry.kind != Ps2DirectoryEntryKind::RegularFile {
        return Err(Ps2FileExportError::InvalidPlan(
            "only regular files can be exported".into(),
        ));
    }
    if u64::from(file.entry.length) != file.declared_size_bytes {
        return Err(Ps2FileExportError::InvalidPlan(
            "directory length and inventory length disagree".into(),
        ));
    }
    if !file.entry.warnings.is_empty() {
        return Err(Ps2FileExportError::InvalidPlan(
            "regular-file metadata has warnings".into(),
        ));
    }
    let unsafe_chain_warning = file
        .chain_health
        .warnings
        .iter()
        .find(|warning| warning.kind != Ps2CorruptionKind::FileSizeExceedsChain);
    if !file.chain_health.complete || unsafe_chain_warning.is_some() {
        return Err(Ps2FileExportError::InvalidPlan(
            "the file FAT chain is not safe for exact reconstruction".into(),
        ));
    }
    let cluster_data_bytes = ps2_cluster_bytes(&geometry) as u64;
    let capacity = (file.chain_health.clusters.len() as u64)
        .checked_mul(cluster_data_bytes)
        .ok_or_else(|| Ps2FileExportError::InvalidPlan("chain capacity overflowed".into()))?;
    if file.declared_size_bytes > capacity {
        return Err(Ps2FileExportError::InvalidPlan(
            "declared file size exceeds the validated FAT chain".into(),
        ));
    }
    validate_source_path(&card.path)?;
    validate_destination_path(destination, &card.path)?;
    let source_bytes = read_source_card(&card.path)?;
    let source_card_sha256 = sha256_hex(&source_bytes);
    Ok(Ps2FileExportPlan {
        source_card_path: card.path.clone(),
        source_card_sha256,
        file_raw_entry_offset: file.entry.raw_entry_offset,
        raw_name: file.entry.raw_name.clone(),
        display_name: file.entry.display_name.clone(),
        declared_size_bytes: file.declared_size_bytes,
        chain: file.chain_health.clusters.clone(),
        cluster_data_bytes,
        destination: destination.to_path_buf(),
        overwrite: false,
        warnings: file
            .chain_health
            .warnings
            .iter()
            .map(|warning| warning.message.clone())
            .collect(),
        provenance: "PS2 memory-card directory entry and validated FAT chain; data pages only, spare/ECC excluded".into(),
        geometry,
    })
}

/// Applies one previously reviewed export plan using a read-only source and
/// an owned temporary destination. Existing destination files are refused.
pub fn apply_ps2_file_export(
    plan: &Ps2FileExportPlan,
) -> Result<Ps2FileExportResult, Ps2FileExportError> {
    if plan.overwrite {
        return Err(Ps2FileExportError::InvalidPlan(
            "overwrite is not supported for PS2 file export".into(),
        ));
    }
    validate_source_path(&plan.source_card_path)?;
    validate_destination_path(&plan.destination, &plan.source_card_path)?;
    let source_bytes = read_source_card(&plan.source_card_path)?;
    if sha256_hex(&source_bytes) != plan.source_card_sha256 {
        return Err(Ps2FileExportError::SourceChanged);
    }
    let expected_clusters = if plan.declared_size_bytes == 0 {
        0
    } else {
        plan.declared_size_bytes
            .div_ceil(plan.cluster_data_bytes.max(1))
    };
    if (plan.chain.len() as u64) < expected_clusters {
        return Err(Ps2FileExportError::InvalidPlan(
            "the reviewed FAT chain is too short".into(),
        ));
    }

    let mut temporary = create_export_temporary(&plan.destination)?;
    (|| {
        let mut remaining = plan.declared_size_bytes as usize;
        let mut hasher = Sha256::new();
        for &cluster in &plan.chain {
            if remaining == 0 {
                break;
            }
            let data =
                ps2_relative_cluster(&source_bytes, &plan.geometry, cluster).ok_or_else(|| {
                    Ps2FileExportError::InvalidPlan("FAT chain points outside card data".into())
                })?;
            let count = remaining.min(data.len());
            temporary
                .write_all(&data[..count])
                .map_err(|error| Ps2FileExportError::Io(error.to_string()))?;
            hasher.update(&data[..count]);
            remaining -= count;
        }
        if remaining != 0 {
            return Err(Ps2FileExportError::InvalidPlan(
                "validated chain did not provide the declared logical length".into(),
            ));
        }
        temporary
            .sync_all()
            .map_err(|error| Ps2FileExportError::Io(error.to_string()))?;
        let bytes_written = temporary
            .metadata()
            .map_err(|error| Ps2FileExportError::Io(error.to_string()))?
            .len();
        if bytes_written != plan.declared_size_bytes {
            return Err(Ps2FileExportError::Io(
                "staged output length did not match the declared logical length".into(),
            ));
        }
        let sha256 = hex_bytes(&hasher.finalize());
        if fs::symlink_metadata(&plan.destination).is_ok() {
            return Err(Ps2FileExportError::DestinationExists(
                plan.destination.clone(),
            ));
        }
        fs::hard_link(temporary.path(), &plan.destination)
            .map_err(|error| Ps2FileExportError::Io(error.to_string()))?;
        fs::remove_file(temporary.path())
            .map_err(|error| Ps2FileExportError::Io(error.to_string()))?;
        Ok(Ps2FileExportResult {
            destination: plan.destination.clone(),
            bytes_written,
            sha256,
            source_card_path: plan.source_card_path.clone(),
            source_card_sha256: plan.source_card_sha256.clone(),
            provenance: plan.provenance.clone(),
        })
    })()
}

static EXPORT_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct ExportTemporary {
    file: fs::File,
    path: PathBuf,
}

impl ExportTemporary {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Write for ExportTemporary {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.file.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl ExportTemporary {
    fn sync_all(&self) -> std::io::Result<()> {
        self.file.sync_all()
    }
    fn metadata(&self) -> std::io::Result<fs::Metadata> {
        self.file.metadata()
    }
}

impl Drop for ExportTemporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn create_export_temporary(destination: &Path) -> Result<ExportTemporary, Ps2FileExportError> {
    let parent = destination
        .parent()
        .ok_or_else(|| Ps2FileExportError::UnsafeDestination(destination.to_path_buf()))?;
    for _ in 0..32 {
        let number = EXPORT_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".emuwiz-ps2-export-{}-{number}.tmp",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok(ExportTemporary { file, path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(Ps2FileExportError::Io(error.to_string())),
        }
    }
    Err(Ps2FileExportError::Io(
        "could not allocate an owned temporary export path".into(),
    ))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_bytes(&Sha256::digest(bytes))
}

fn hex_bytes(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(DIGITS[(byte >> 4) as usize] as char);
        result.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    result
}

fn validate_source_path(path: &Path) -> Result<(), Ps2FileExportError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| Ps2FileExportError::Io(error.to_string()))?;
    if !path.is_absolute() || !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Ps2FileExportError::InvalidPlan(
            "source card is not an absolute regular file".into(),
        ));
    }
    Ok(())
}

fn validate_destination_path(destination: &Path, source: &Path) -> Result<(), Ps2FileExportError> {
    if !destination.is_absolute()
        || destination.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
        || destination == source
    {
        return Err(Ps2FileExportError::UnsafeDestination(
            destination.to_path_buf(),
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| Ps2FileExportError::UnsafeDestination(destination.to_path_buf()))?;
    if !parent.is_dir() {
        return Err(Ps2FileExportError::UnsafeDestination(
            destination.to_path_buf(),
        ));
    }
    let mut current = PathBuf::from(std::path::MAIN_SEPARATOR.to_string());
    for component in parent.components() {
        if matches!(component, std::path::Component::RootDir) {
            continue;
        }
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| Ps2FileExportError::Io(error.to_string()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Ps2FileExportError::UnsafeDestination(
                destination.to_path_buf(),
            ));
        }
    }
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if metadata.file_type().is_symlink() || metadata.is_dir() || metadata.is_file() {
            return Err(Ps2FileExportError::DestinationExists(
                destination.to_path_buf(),
            ));
        }
        return Err(Ps2FileExportError::DestinationExists(
            destination.to_path_buf(),
        ));
    }
    Ok(())
}

fn read_source_card(path: &Path) -> Result<Vec<u8>, Ps2FileExportError> {
    let safe = crate::safe_read::open_bounded_read(path, &crate::safe_read::TrustedRoots::none())
        .map_err(|error| Ps2FileExportError::Io(format!("{error:?}")))?;
    let file = safe.into_file();
    let mut bytes = Vec::new();
    file.take(PS2_MAX_CARD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Ps2FileExportError::Io(error.to_string()))?;
    if bytes.len() > PS2_MAX_CARD_BYTES {
        return Err(Ps2FileExportError::InvalidPlan(
            "source card exceeds inspection bound".into(),
        ));
    }
    Ok(bytes)
}

fn text(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let value = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
    (!value.is_empty()).then_some(value)
}
fn ps1_identity(header: &[u8], data: &[u8]) -> MemoryCardEntryIdentity {
    let product = text(header.get(12..22).unwrap_or_default());
    let title = text(data.get(4..4 + MAX_TITLE_BYTES).unwrap_or_default());
    let confidence = if product.as_deref().is_some_and(|v| v.contains('-')) {
        MemoryCardIdentityConfidence::ConfirmedIdentity
    } else if title.is_some() {
        MemoryCardIdentityConfidence::SupportingEvidence
    } else {
        MemoryCardIdentityConfidence::Unknown
    };
    let mut evidence = Vec::new();
    if product.is_some() {
        evidence.push("validated PS1 card product-code field".into());
    }
    if title.is_some() {
        evidence.push("PS1 save title field".into());
    }
    MemoryCardEntryIdentity {
        product_code: product,
        display_title: title,
        confidence,
        evidence,
    }
}

fn inspect_ps1(path: &Path, bytes: &[u8]) -> MemoryCardInventory {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut used = 0u32;
    let mut damaged = false;
    for slot in 0..PS1_SLOT_COUNT {
        let offset = slot * PS1_BLOCK_BYTES;
        let header = &bytes[offset..offset + PS1_HEADER_BYTES];
        let kind = header[0];
        let valid = matches!(kind, 0xA0 | 0x51 | 0x52 | 0x53 | 0xA1 | 0xA2 | 0xA3);
        if !valid {
            if kind != 0 {
                damaged = true;
                warnings.push(format!("slot {slot} has unknown block marker 0x{kind:02x}"));
            }
            continue;
        }
        if matches!(kind, 0xA0 | 0x51) {
            used += 1;
            let data = &bytes[offset + PS1_HEADER_BYTES..offset + PS1_BLOCK_BYTES];
            let checksum = header[..127].iter().fold(0u8, |sum, value| sum ^ value);
            let mut entry_warnings = Vec::new();
            if checksum != header[127] {
                entry_warnings
                    .push("header XOR checksum does not match; bytes were not repaired".into());
                damaged = true;
            }
            entries.push(MemoryCardEntry {
                slot: Some(slot as u32),
                name: text(&header[10..30]).unwrap_or_else(|| format!("slot-{slot}")),
                size_bytes: PS1_BLOCK_BYTES as u64,
                used_blocks: Some(1),
                identity: ps1_identity(header, data),
                deleted: false,
                warnings: entry_warnings,
            });
        } else if matches!(kind, 0xA1..=0xA3) {
            entries.push(MemoryCardEntry {
                slot: Some(slot as u32),
                name: format!("deleted-slot-{slot}"),
                size_bytes: 0,
                used_blocks: Some(1),
                identity: MemoryCardEntryIdentity {
                    product_code: None,
                    display_title: None,
                    confidence: MemoryCardIdentityConfidence::Unknown,
                    evidence: Vec::new(),
                },
                deleted: true,
                warnings: vec![
                    "Deleted slot state is reported only; deleted data is not recovered.".into(),
                ],
            });
        }
    }
    MemoryCardInventory {
        path: path.into(),
        format: MemoryCardFormat::Ps1Raw,
        format_confidence: MemoryCardFormatConfidence::ConfirmedFormat,
        health: if damaged {
            MemoryCardHealth::CorruptionSuspected
        } else {
            MemoryCardHealth::Healthy
        },
        card_size_bytes: bytes.len() as u64,
        entries,
        used_blocks: Some(used),
        free_blocks: Some((PS1_SLOT_COUNT as u32).saturating_sub(used)),
        warnings,
        shared_container: true,
        ps2_geometry: None,
        ps2_inventory: None,
    }
}

fn looks_like_ps2(bytes: &[u8]) -> bool {
    bytes.get(..27) == Some(b"Sony PS2 Memory Card Format")
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn ps2_geometry(bytes: &[u8]) -> Result<Ps2Geometry, (MemoryCardHealth, String)> {
    if bytes.len() < PS2_SUPERBLOCK_BYTES {
        return Err((
            MemoryCardHealth::Truncated,
            "PS2 superblock is truncated".into(),
        ));
    }
    let page_len = le_u16(bytes, 0x28).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 page length is unavailable".into(),
    ))? as usize;
    let pages_per_cluster = le_u16(bytes, 0x2a).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 cluster geometry is unavailable".into(),
    ))? as usize;
    let pages_per_block = le_u16(bytes, 0x2c).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 block geometry is unavailable".into(),
    ))? as usize;
    let clusters_per_card = le_u32(bytes, 0x30).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 card geometry is unavailable".into(),
    ))? as u64;
    let alloc_offset = le_u32(bytes, 0x34).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 allocation geometry is unavailable".into(),
    ))?;
    let alloc_end = le_u32(bytes, 0x38).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 allocation geometry is unavailable".into(),
    ))?;
    let rootdir_cluster = le_u32(bytes, 0x3c).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 root geometry is unavailable".into(),
    ))?;
    let backup_block1 = le_u32(bytes, 0x40).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 backup geometry is unavailable".into(),
    ))?;
    let backup_block2 = le_u32(bytes, 0x44).ok_or((
        MemoryCardHealth::Truncated,
        "PS2 backup geometry is unavailable".into(),
    ))?;
    if page_len != PS2_PAGE_DATA_BYTES || pages_per_cluster == 0 || pages_per_block == 0 {
        return Err((
            MemoryCardHealth::UnsupportedVariant,
            "PS2 page/cluster/block geometry is unsupported".into(),
        ));
    }
    let cluster_data = pages_per_cluster.checked_mul(page_len).ok_or((
        MemoryCardHealth::OutOfRangeMetadata,
        "PS2 cluster-size arithmetic overflowed".into(),
    ))?;
    if cluster_data == 0 || cluster_data > PS2_MAX_CARD_BYTES {
        return Err((
            MemoryCardHealth::OutOfRangeMetadata,
            "PS2 cluster size is outside the inspection bound".into(),
        ));
    }
    if clusters_per_card == 0 || clusters_per_card > (PS2_MAX_CARD_BYTES / page_len) as u64 {
        return Err((
            MemoryCardHealth::OutOfRangeMetadata,
            "PS2 cluster count is outside the inspection bound".into(),
        ));
    }
    if alloc_offset > alloc_end
        || u64::from(alloc_end) > clusters_per_card
        || (rootdir_cluster != 0
            && (rootdir_cluster < alloc_offset || rootdir_cluster >= alloc_end))
    {
        return Err((
            MemoryCardHealth::OutOfRangeMetadata,
            "PS2 allocation or root metadata is out of range".into(),
        ));
    }
    let pages = clusters_per_card
        .checked_mul(pages_per_cluster as u64)
        .ok_or((
            MemoryCardHealth::OutOfRangeMetadata,
            "PS2 page-count arithmetic overflowed".into(),
        ))?;
    let raw_size = pages.checked_mul(PS2_PAGE_DATA_BYTES as u64).ok_or((
        MemoryCardHealth::OutOfRangeMetadata,
        "PS2 raw geometry arithmetic overflowed".into(),
    ))?;
    let physical_size = pages.checked_mul(PS2_PHYSICAL_PAGE_BYTES as u64).ok_or((
        MemoryCardHealth::OutOfRangeMetadata,
        "PS2 spare geometry arithmetic overflowed".into(),
    ))?;
    let (representation, stride) = if bytes.len() as u64 == physical_size {
        (Ps2PageRepresentation::RawWithSpare, PS2_PHYSICAL_PAGE_BYTES)
    } else if bytes.len() as u64 == raw_size {
        (Ps2PageRepresentation::RawDataOnly, PS2_PAGE_DATA_BYTES)
    } else if (bytes.len() as u64) < raw_size {
        return Err((
            MemoryCardHealth::Truncated,
            "PS2 declared geometry exceeds file length".into(),
        ));
    } else {
        return Err((
            MemoryCardHealth::UnsupportedVariant,
            "PS2 file size does not match a proven page representation".into(),
        ));
    };
    let version = text(&bytes[0x1c..0x28]);
    Ok(Ps2Geometry {
        file_size_bytes: bytes.len() as u64,
        page_stride_bytes: stride as u32,
        page_data_bytes: PS2_PAGE_DATA_BYTES as u32,
        spare_bytes: (stride - PS2_PAGE_DATA_BYTES) as u32,
        pages_per_cluster: pages_per_cluster as u32,
        pages_per_block: pages_per_block as u32,
        clusters_per_card: clusters_per_card as u32,
        alloc_offset,
        alloc_end,
        rootdir_cluster,
        backup_block1,
        backup_block2,
        superblock_offset: 0,
        version,
        marker_bytes: [bytes[0x150], bytes[0x151]],
        representation,
    })
}

fn ps2_warning(kind: Ps2CorruptionKind, message: impl Into<String>) -> Ps2InventoryWarning {
    Ps2InventoryWarning {
        kind,
        message: message.into(),
    }
}

fn ps2_cluster_bytes(geometry: &Ps2Geometry) -> usize {
    geometry.pages_per_cluster as usize * geometry.page_data_bytes as usize
}

fn ps2_logical_cluster(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    absolute_cluster: u32,
) -> Option<Vec<u8>> {
    if absolute_cluster >= geometry.clusters_per_card {
        return None;
    }
    let mut cluster = Vec::with_capacity(ps2_cluster_bytes(geometry));
    let page_data = geometry.page_data_bytes as usize;
    let stride = geometry.page_stride_bytes as usize;
    let first_page = absolute_cluster as usize * geometry.pages_per_cluster as usize;
    for page in 0..geometry.pages_per_cluster as usize {
        let offset = (first_page + page).checked_mul(stride)?;
        cluster.extend_from_slice(bytes.get(offset..offset + page_data)?);
    }
    Some(cluster)
}

fn ps2_relative_cluster(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    relative_cluster: u32,
) -> Option<Vec<u8>> {
    let absolute = geometry.alloc_offset.checked_add(relative_cluster)?;
    if absolute >= geometry.alloc_end {
        return None;
    }
    ps2_logical_cluster(bytes, geometry, absolute)
}

fn ps2_fat_next(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    relative_cluster: u32,
) -> Result<Option<u32>, Ps2InventoryWarning> {
    let available = geometry.alloc_end.saturating_sub(geometry.alloc_offset);
    if relative_cluster >= available {
        return Err(ps2_warning(
            Ps2CorruptionKind::ClusterOutOfRange,
            format!("relative cluster {relative_cluster} is outside the allocation area"),
        ));
    }
    let entries_per_cluster = ps2_cluster_bytes(geometry) / 4;
    if entries_per_cluster == 0 {
        return Err(ps2_warning(
            Ps2CorruptionKind::InvalidFatReference,
            "PS2 cluster has no room for FAT entries",
        ));
    }
    let indirect_index = relative_cluster as usize / entries_per_cluster;
    let indirect_slot = indirect_index / entries_per_cluster;
    let ifc_offset = 0x50usize
        .checked_add(indirect_slot.checked_mul(4).ok_or_else(|| {
            ps2_warning(
                Ps2CorruptionKind::InvalidFatReference,
                "IFC slot arithmetic overflowed",
            )
        })?)
        .ok_or_else(|| {
            ps2_warning(
                Ps2CorruptionKind::InvalidFatReference,
                "IFC offset arithmetic overflowed",
            )
        })?;
    let ifc_cluster = le_u32(bytes, ifc_offset).ok_or_else(|| {
        ps2_warning(
            Ps2CorruptionKind::TruncatedCard,
            "IFC entry is outside the card",
        )
    })?;
    let fat_cluster_index = relative_cluster as usize % entries_per_cluster;
    let fat_cluster = ps2_logical_cluster(bytes, geometry, ifc_cluster).ok_or_else(|| {
        ps2_warning(
            Ps2CorruptionKind::ClusterOutOfRange,
            format!("IFC points to unavailable FAT cluster {ifc_cluster}"),
        )
    })?;
    let fat_offset = fat_cluster_index.checked_mul(4).ok_or_else(|| {
        ps2_warning(
            Ps2CorruptionKind::InvalidFatReference,
            "FAT entry arithmetic overflowed",
        )
    })?;
    let raw = le_u32(&fat_cluster, fat_offset).ok_or_else(|| {
        ps2_warning(
            Ps2CorruptionKind::TruncatedCard,
            "FAT entry is outside the card",
        )
    })?;
    if raw == 0x7fff_ffff || raw == 0xffff_ffff {
        return Ok(None);
    }
    if raw & 0x8000_0000 == 0 {
        return Err(ps2_warning(
            Ps2CorruptionKind::InvalidFatReference,
            format!("FAT entry {raw:#x} is free or reserved, not a chain link"),
        ));
    }
    let next = raw & 0x7fff_ffff;
    if next >= available {
        return Err(ps2_warning(
            Ps2CorruptionKind::ClusterOutOfRange,
            format!("FAT points to relative cluster {next} outside the allocation area"),
        ));
    }
    Ok(Some(next))
}

fn ps2_chain(bytes: &[u8], geometry: &Ps2Geometry, start: u32) -> Ps2ClusterChainHealth {
    let available = geometry.alloc_end.saturating_sub(geometry.alloc_offset) as usize;
    if start >= available as u32 {
        return Ps2ClusterChainHealth {
            clusters: Vec::new(),
            complete: false,
            warnings: vec![ps2_warning(
                Ps2CorruptionKind::ClusterOutOfRange,
                format!("start cluster {start} is outside the allocation area"),
            )],
        };
    }
    let mut clusters = Vec::new();
    let mut visited = HashSet::new();
    let mut current = start;
    let mut warnings = Vec::new();
    while clusters.len() < available {
        if !visited.insert(current) {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::FatLoop,
                format!("FAT chain loops at relative cluster {current}"),
            ));
            return Ps2ClusterChainHealth {
                clusters,
                complete: false,
                warnings,
            };
        }
        clusters.push(current);
        match ps2_fat_next(bytes, geometry, current) {
            Ok(Some(next)) => current = next,
            Ok(None) => {
                return Ps2ClusterChainHealth {
                    clusters,
                    complete: true,
                    warnings,
                };
            }
            Err(warning) => {
                warnings.push(warning);
                return Ps2ClusterChainHealth {
                    clusters,
                    complete: false,
                    warnings,
                };
            }
        }
    }
    warnings.push(ps2_warning(
        Ps2CorruptionKind::InvalidFatReference,
        "FAT chain exceeded the allocation-area cluster bound",
    ));
    Ps2ClusterChainHealth {
        clusters,
        complete: false,
        warnings,
    }
}

fn ps2_timestamp(raw: [u8; 8]) -> Result<Ps2Timestamp, Ps2InventoryWarning> {
    let timestamp = Ps2Timestamp {
        raw,
        year: u16::from_le_bytes([raw[6], raw[7]]),
        month: raw[5],
        day: raw[4],
        hour: raw[3],
        minute: raw[2],
        second: raw[1],
        timezone: "JST (UTC+09:00)",
    };
    if timestamp.year == 0
        || !(1..=12).contains(&timestamp.month)
        || timestamp.day == 0
        || timestamp.day > 31
        || timestamp.hour > 23
        || timestamp.minute > 59
        || timestamp.second > 59
    {
        return Err(ps2_warning(
            Ps2CorruptionKind::InvalidTimestamp,
            "timestamp fields are outside the validated PS2 range",
        ));
    }
    Ok(timestamp)
}

fn ps2_entry(raw: &[u8], raw_entry_offset: u64) -> Ps2DirectoryEntry {
    let raw_mode = le_u32(raw, 0).unwrap_or(0);
    let length = le_u32(raw, 4).unwrap_or(0);
    let start_cluster = le_u32(raw, 0x10).unwrap_or(u32::MAX);
    let parent_entry = le_u32(raw, 0x14).unwrap_or(0);
    let attributes = le_u32(raw, 0x20).unwrap_or(0);
    let raw_name = raw
        .get(0x40..0x40 + PS2_NAME_BYTES)
        .unwrap_or_default()
        .to_vec();
    let name_end = raw_name.iter().position(|byte| *byte == 0);
    let mut warnings = Vec::new();
    let name_bytes = match name_end {
        Some(end) => raw_name[..end].to_vec(),
        None => {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::InvalidFilename,
                "name field has no NUL terminator within 32 bytes",
            ));
            raw_name.clone()
        }
    };
    let display_name = match String::from_utf8(name_bytes) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::InvalidFilename,
                "name bytes are not valid UTF-8; display uses replacement characters",
            ));
            String::from_utf8_lossy(error.as_bytes()).into_owned()
        }
    };
    let created_raw = raw
        .get(0x08..0x10)
        .and_then(|value| value.try_into().ok())
        .unwrap_or([0; 8]);
    let modified_raw = raw
        .get(0x18..0x20)
        .and_then(|value| value.try_into().ok())
        .unwrap_or([0; 8]);
    let created = match ps2_timestamp(created_raw) {
        Ok(value) => Some(value),
        Err(warning) => {
            if created_raw.iter().any(|byte| *byte != 0) {
                warnings.push(warning);
            }
            None
        }
    };
    let modified = match ps2_timestamp(modified_raw) {
        Ok(value) => Some(value),
        Err(warning) => {
            if modified_raw.iter().any(|byte| *byte != 0) {
                warnings.push(warning);
            }
            None
        }
    };
    let low_mode = raw_mode as u16;
    let used = low_mode & 0x8000 != 0;
    let is_directory = low_mode & 0x0020 != 0;
    let is_file = low_mode & 0x0010 != 0;
    let kind = if raw_mode == 0 || raw_mode == u32::MAX || !used {
        Ps2DirectoryEntryKind::Unused
    } else if is_directory && !is_file {
        Ps2DirectoryEntryKind::Directory
    } else if is_file && !is_directory {
        Ps2DirectoryEntryKind::RegularFile
    } else if is_directory || is_file {
        warnings.push(ps2_warning(
            Ps2CorruptionKind::InvalidDirectoryEntry,
            "directory and regular-file mode bits are contradictory",
        ));
        Ps2DirectoryEntryKind::Invalid
    } else {
        Ps2DirectoryEntryKind::Special
    };
    Ps2DirectoryEntry {
        raw_entry_offset,
        raw_mode,
        kind,
        raw_name,
        display_name,
        length,
        start_cluster,
        parent_entry,
        attributes,
        created,
        modified,
        warnings,
    }
}

fn ps2_read_directory(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    start_cluster: u32,
    declared_entries: u32,
) -> (
    Ps2ClusterChainHealth,
    Vec<Ps2DirectoryEntry>,
    Vec<Ps2InventoryWarning>,
) {
    let mut warnings = Vec::new();
    let available = geometry.alloc_end.saturating_sub(geometry.alloc_offset) as usize;
    let max_entries = available
        .saturating_mul(ps2_cluster_bytes(geometry) / PS2_DIRECTORY_ENTRY_BYTES)
        .min(PS2_MAX_INVENTORY_ENTRIES);
    let requested = declared_entries as usize;
    if requested > max_entries {
        warnings.push(ps2_warning(
            Ps2CorruptionKind::TooManyEntries,
            format!("directory declares {declared_entries} entries; bounded at {max_entries}"),
        ));
    }
    let count = requested.min(max_entries);
    let chain = ps2_chain(bytes, geometry, start_cluster);
    let records_per_cluster = ps2_cluster_bytes(geometry) / PS2_DIRECTORY_ENTRY_BYTES;
    let needed_clusters = count.div_ceil(records_per_cluster.max(1));
    if chain.clusters.len() < needed_clusters {
        warnings.push(ps2_warning(
            Ps2CorruptionKind::TruncatedCard,
            format!(
                "directory needs {needed_clusters} clusters but its chain has {}",
                chain.clusters.len()
            ),
        ));
    }
    let mut entries = Vec::new();
    for index in 0..count {
        let cluster_index = index / records_per_cluster.max(1);
        let slot = index % records_per_cluster.max(1);
        let Some(&relative_cluster) = chain.clusters.get(cluster_index) else {
            break;
        };
        let Some(cluster) = ps2_relative_cluster(bytes, geometry, relative_cluster) else {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::TruncatedCard,
                format!("directory cluster {relative_cluster} is outside the card"),
            ));
            break;
        };
        let offset = slot * PS2_DIRECTORY_ENTRY_BYTES;
        let Some(raw) = cluster.get(offset..offset + PS2_DIRECTORY_ENTRY_BYTES) else {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::TruncatedCard,
                "directory entry is truncated",
            ));
            break;
        };
        let logical_offset = (geometry.alloc_offset as u64 + relative_cluster as u64)
            * ps2_cluster_bytes(geometry) as u64
            + (slot * PS2_DIRECTORY_ENTRY_BYTES) as u64;
        entries.push(ps2_entry(raw, logical_offset));
    }
    (chain, entries, warnings)
}

fn ps2_file_chain(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    entry: &Ps2DirectoryEntry,
) -> Ps2ClusterChainHealth {
    if entry.length == 0 && entry.start_cluster == u32::MAX {
        return Ps2ClusterChainHealth {
            clusters: Vec::new(),
            complete: true,
            warnings: Vec::new(),
        };
    }
    if entry.length > 0 && entry.start_cluster == u32::MAX {
        return Ps2ClusterChainHealth {
            clusters: Vec::new(),
            complete: false,
            warnings: vec![ps2_warning(
                Ps2CorruptionKind::InvalidFatReference,
                "non-empty file uses the PS2 empty-file cluster marker",
            )],
        };
    }
    let mut chain = ps2_chain(bytes, geometry, entry.start_cluster);
    let cluster_bytes = ps2_cluster_bytes(geometry) as u64;
    let needed = (u64::from(entry.length) + cluster_bytes.saturating_sub(1)) / cluster_bytes;
    if (chain.clusters.len() as u64) < needed {
        chain.complete = false;
        chain.warnings.push(ps2_warning(
            Ps2CorruptionKind::FileSizeExceedsChain,
            format!(
                "file declares {} bytes but its chain has {} clusters",
                entry.length,
                chain.clusters.len()
            ),
        ));
    } else if (chain.clusters.len() as u64) > needed {
        chain.warnings.push(ps2_warning(
            Ps2CorruptionKind::FileSizeExceedsChain,
            format!(
                "file chain has {} clusters for a declared {}-byte payload requiring {needed}",
                chain.clusters.len(),
                entry.length
            ),
        ));
    }
    chain
}

fn ps2_save_directory(
    bytes: &[u8],
    geometry: &Ps2Geometry,
    entry: Ps2DirectoryEntry,
) -> Ps2SaveDirectory {
    let (chain_health, entries, mut warnings) =
        ps2_read_directory(bytes, geometry, entry.start_cluster, entry.length);
    warnings.extend(chain_health.warnings.clone());
    let mut children = Vec::new();
    let mut files = Vec::new();
    for child in entries {
        if child.display_name == "." || child.display_name == ".." {
            continue;
        }
        if child.kind == Ps2DirectoryEntryKind::RegularFile {
            let declared_size_bytes = u64::from(child.length);
            files.push(Ps2SaveFile {
                chain_health: ps2_file_chain(bytes, geometry, &child),
                entry: child,
                declared_size_bytes,
            });
        } else {
            if child.kind == Ps2DirectoryEntryKind::Directory {
                warnings.push(ps2_warning(
                    Ps2CorruptionKind::DirectoryTooDeep,
                    format!(
                        "nested directory {} was not descended beyond the bounded save inventory depth {}",
                        child.display_name, PS2_MAX_DIRECTORY_DEPTH
                    ),
                ));
            }
            children.push(child);
        }
    }
    Ps2SaveDirectory {
        entry,
        chain_health,
        children,
        files,
        warnings,
    }
}

fn inspect_ps2_filesystem(bytes: &[u8], geometry: &Ps2Geometry) -> Ps2MemoryCardInventory {
    let mut warnings = Vec::new();
    let root_chain = ps2_chain(bytes, geometry, geometry.rootdir_cluster);
    let root_count = ps2_relative_cluster(bytes, geometry, geometry.rootdir_cluster)
        .map(|cluster| {
            cluster
                .get(..PS2_DIRECTORY_ENTRY_BYTES)
                .and_then(|raw| le_u32(raw, 4))
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let (root_chain_health, root_entries, root_warnings) =
        ps2_read_directory(bytes, geometry, geometry.rootdir_cluster, root_count);
    warnings.extend(root_warnings);
    warnings.extend(root_chain.warnings);
    let mut save_directories = Vec::new();
    for entry in &root_entries {
        if entry.kind != Ps2DirectoryEntryKind::Directory
            || entry.display_name == "."
            || entry.display_name == ".."
            || entry.display_name == "BEDATA-SYSTEM"
        {
            continue;
        }
        if save_directories.len() >= PS2_MAX_INVENTORY_ENTRIES {
            warnings.push(ps2_warning(
                Ps2CorruptionKind::TooManyEntries,
                "save-directory inventory bound reached",
            ));
            break;
        }
        save_directories.push(ps2_save_directory(bytes, geometry, entry.clone()));
    }
    Ps2MemoryCardInventory {
        root_chain_health,
        root_entries,
        save_directories,
        warnings,
    }
}

pub fn inspect_memory_card(path: &Path) -> Result<MemoryCardInventory, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("memory card must be a regular non-symlink file".into());
    }
    if metadata.len() > PS2_MAX_CARD_BYTES as u64 {
        return Err("memory card exceeds inspection bound".into());
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() == PS1_CARD_BYTES {
        return Ok(inspect_ps1(path, &bytes));
    }
    if looks_like_ps2(&bytes) {
        let geometry = ps2_geometry(&bytes);
        let (mut health, warning, geometry) = match geometry {
            Ok(geometry) => (
                MemoryCardHealth::Healthy,
                "PS2 geometry and superblock are structurally plausible; filesystem contents were inspected read-only."
                    .into(),
                Some(geometry),
            ),
            Err((health, warning)) => (health, warning, None),
        };
        let ps2_inventory = geometry
            .as_ref()
            .map(|geometry| inspect_ps2_filesystem(&bytes, geometry));
        if let Some(inventory) = &ps2_inventory {
            let entry_warning = inventory
                .root_entries
                .iter()
                .any(|entry| !entry.warnings.is_empty())
                || inventory.save_directories.iter().any(|directory| {
                    !directory.warnings.is_empty()
                        || directory
                            .children
                            .iter()
                            .any(|entry| !entry.warnings.is_empty())
                        || directory
                            .files
                            .iter()
                            .any(|file| !file.chain_health.warnings.is_empty())
                });
            if !inventory.warnings.is_empty() || entry_warning {
                health = MemoryCardHealth::StructuralWarning;
            }
        }
        return Ok(MemoryCardInventory {
            path: path.into(),
            format: MemoryCardFormat::Ps2,
            format_confidence: MemoryCardFormatConfidence::ConfirmedFormat,
            health,
            card_size_bytes: bytes.len() as u64,
            entries: Vec::new(),
            used_blocks: None,
            free_blocks: None,
            warnings: {
                let mut warnings = vec![warning];
                if let Some(inventory) = &ps2_inventory {
                    warnings.extend(
                        inventory
                            .warnings
                            .iter()
                            .map(|warning| warning.message.clone()),
                    );
                }
                warnings
            },
            shared_container: true,
            ps2_geometry: geometry,
            ps2_inventory,
        });
    }
    Ok(MemoryCardInventory {
        path: path.into(),
        format: MemoryCardFormat::Unknown,
        format_confidence: if bytes.len() < PS1_CARD_BYTES {
            MemoryCardFormatConfidence::Malformed
        } else {
            MemoryCardFormatConfidence::UnknownFormat
        },
        health: if bytes.len() < PS1_CARD_BYTES {
            MemoryCardHealth::Truncated
        } else {
            MemoryCardHealth::Unknown
        },
        card_size_bytes: bytes.len() as u64,
        entries: Vec::new(),
        used_blocks: None,
        free_blocks: None,
        warnings: vec!["Size/structure did not prove a supported PS1 or PS2 card format.".into()],
        shared_container: true,
        ps2_geometry: None,
        ps2_inventory: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    fn fixture() -> Vec<u8> {
        let mut card = vec![0u8; PS1_CARD_BYTES];
        card[0] = 0xA0;
        card[10..26].copy_from_slice(b"BASLUS-00067SAVE");
        card[12..22].copy_from_slice(b"BASLUS-000");
        card[128 + 4..128 + 4 + 10].copy_from_slice(b"Test Save\0");
        card[127] = card[..127].iter().fold(0u8, |sum, value| sum ^ value);
        card
    }
    #[test]
    fn ps1_card_is_shared_and_identity_is_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.mcr");
        let before = fixture();
        fs::write(&path, &before).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert!(inv.shared_container);
        assert_eq!(inv.entries.len(), 1);
        assert!(inv.entries[0].identity.product_code.is_some());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    #[test]
    fn truncated_and_unknown_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.bin");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"bad").unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.health, MemoryCardHealth::Truncated);
        assert!(inv.entries.is_empty());
    }
    #[test]
    fn ps2_header_is_not_falsely_enumerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.ps2");
        let mut bytes = vec![0u8; 8 * 1024 * 1024];
        bytes[..27].copy_from_slice(b"Sony PS2 Memory Card Format");
        fs::write(&path, &bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.format, MemoryCardFormat::Ps2);
        assert_eq!(inv.health, MemoryCardHealth::UnsupportedVariant);
        assert!(inv.entries.is_empty());
    }

    fn ps2_fixture(page_stride: usize) -> Vec<u8> {
        let clusters = 8192usize;
        let pages_per_cluster = 2usize;
        let mut card = vec![0u8; clusters * pages_per_cluster * page_stride];
        card[..28].copy_from_slice(b"Sony PS2 Memory Card Format ");
        card[0x1c..0x24].copy_from_slice(b"1.2.0.0\0");
        card[0x28..0x2a].copy_from_slice(&(PS2_PAGE_DATA_BYTES as u16).to_le_bytes());
        card[0x2a..0x2c].copy_from_slice(&(pages_per_cluster as u16).to_le_bytes());
        card[0x2c..0x2e].copy_from_slice(&(16u16).to_le_bytes());
        card[0x30..0x34].copy_from_slice(&(clusters as u32).to_le_bytes());
        card[0x34..0x38].copy_from_slice(&(41u32).to_le_bytes());
        card[0x38..0x3c].copy_from_slice(&(8135u32).to_le_bytes());
        card[0x3c..0x40].copy_from_slice(&(0u32).to_le_bytes());
        card[0x40..0x44].copy_from_slice(&(1023u32).to_le_bytes());
        card[0x44..0x48].copy_from_slice(&(1022u32).to_le_bytes());
        card[0x50..0x54].copy_from_slice(&8u32.to_le_bytes());
        let fat_cluster_offset = 8 * pages_per_cluster * page_stride;
        card[fat_cluster_offset..fat_cluster_offset + 4]
            .copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        card
    }

    fn ps2_inventory_fixture() -> Vec<u8> {
        let mut card = ps2_fixture(PS2_PAGE_DATA_BYTES);
        card[0x50..0x54].copy_from_slice(&8u32.to_le_bytes());
        card[8 * 1024..8 * 1024 + 4].copy_from_slice(&0x8000_0009u32.to_le_bytes());
        for relative in [3usize, 4, 9] {
            let offset = 8 * 1024 + relative * 4;
            card[offset..offset + 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        }
        card[8 * 1024 + 2 * 4..8 * 1024 + 2 * 4 + 4].copy_from_slice(&0x8000_0004u32.to_le_bytes());

        fn entry(
            card: &mut [u8],
            relative: usize,
            slot: usize,
            mode: u32,
            length: u32,
            cluster: u32,
            name: &[u8],
        ) {
            let offset = (41 + relative) * 1024 + slot * PS2_DIRECTORY_ENTRY_BYTES;
            card[offset..offset + 4].copy_from_slice(&mode.to_le_bytes());
            card[offset + 4..offset + 8].copy_from_slice(&length.to_le_bytes());
            card[offset + 0x08..offset + 0x10].copy_from_slice(&[0, 38, 44, 12, 24, 6, 0xea, 0x07]);
            card[offset + 0x10..offset + 0x14].copy_from_slice(&cluster.to_le_bytes());
            card[offset + 0x18..offset + 0x20].copy_from_slice(&[0, 39, 44, 12, 24, 6, 0xea, 0x07]);
            card[offset + 0x40..offset + 0x40 + name.len()].copy_from_slice(name);
        }

        entry(&mut card, 0, 0, 0x8427, 3, 0, b".");
        entry(&mut card, 0, 1, 0xa426, 0, 0, b"..");
        entry(&mut card, 9, 0, 0x8427, 3, 2, b"BASLUS-00001TEST");
        entry(&mut card, 2, 0, 0x8427, 3, 2, b".");
        entry(&mut card, 2, 1, 0xa426, 0, 0, b"..");
        entry(&mut card, 4, 0, 0x8497, 100, 3, b"icon.sys");
        card[(41 + 3) * 1024..(41 + 3) * 1024 + 100].fill(0x5a);
        card
    }

    #[test]
    fn ps2_528_byte_geometry_is_structurally_recognized_without_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Mcd001.ps2");
        let bytes = ps2_fixture(PS2_PHYSICAL_PAGE_BYTES);
        fs::write(&path, &bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.health, MemoryCardHealth::Healthy);
        assert!(inv.entries.is_empty());
        let geometry = inv.ps2_geometry.unwrap();
        assert_eq!(geometry.page_stride_bytes, 528);
        assert_eq!(geometry.spare_bytes, 16);
        assert_eq!(geometry.alloc_offset, 41);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn ps2_impossible_geometry_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.ps2");
        let mut bytes = ps2_fixture(PS2_PHYSICAL_PAGE_BYTES);
        bytes[0x28..0x2a].copy_from_slice(&(513u16).to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        assert_eq!(inv.health, MemoryCardHealth::UnsupportedVariant);
        assert!(inv.ps2_geometry.is_none());
    }

    #[test]
    fn ps2_inventory_walks_root_save_and_file_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Mcd001.ps2");
        let before = ps2_inventory_fixture();
        fs::write(&path, &before).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        let ps2 = inv.ps2_inventory.unwrap();
        assert!(ps2.root_chain_health.complete);
        assert_eq!(ps2.root_entries.len(), 3);
        assert_eq!(ps2.save_directories.len(), 1);
        let save = &ps2.save_directories[0];
        assert_eq!(save.entry.display_name, "BASLUS-00001TEST");
        assert_eq!(save.files.len(), 1);
        assert_eq!(save.files[0].entry.display_name, "icon.sys");
        assert_eq!(save.files[0].declared_size_bytes, 100);
        assert!(save.files[0].chain_health.complete);
        assert_eq!(save.files[0].chain_health.clusters, vec![3]);
        assert_eq!(save.files[0].entry.created.as_ref().unwrap().year, 2026);
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn ps2_inventory_reports_fat_loop_and_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loop.ps2");
        let mut bytes = ps2_inventory_fixture();
        bytes[8 * 1024 + 3 * 4..8 * 1024 + 4 * 4].copy_from_slice(&0x8000_0003u32.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        let ps2 = inv.ps2_inventory.unwrap();
        let file = &ps2.save_directories[0].files[0];
        assert!(
            file.chain_health
                .warnings
                .iter()
                .any(|warning| warning.kind == Ps2CorruptionKind::FatLoop)
        );

        let mut bytes = ps2_inventory_fixture();
        let file_entry_offset = (41 + 4) * 1024;
        bytes[file_entry_offset + 4..file_entry_offset + 8].copy_from_slice(&2000u32.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        let file = &inv.ps2_inventory.unwrap().save_directories[0].files[0];
        assert!(
            file.chain_health
                .warnings
                .iter()
                .any(|warning| warning.kind == Ps2CorruptionKind::FileSizeExceedsChain)
        );
    }

    #[test]
    fn ps2_inventory_keeps_raw_name_and_reports_invalid_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metadata.ps2");
        let mut bytes = ps2_inventory_fixture();
        let offset = (41 + 9) * 1024;
        bytes[offset + 0x08..offset + 0x10].copy_from_slice(&[0, 0, 0, 25, 0, 13, 0, 0]);
        bytes[offset + 0x40..offset + 0x40 + PS2_NAME_BYTES]
            .copy_from_slice(&[0xff; PS2_NAME_BYTES]);
        fs::write(&path, bytes).unwrap();
        let inv = inspect_memory_card(&path).unwrap();
        let root = &inv.ps2_inventory.unwrap().root_entries[2];
        assert_eq!(root.raw_name, vec![0xff; PS2_NAME_BYTES]);
        assert!(
            root.warnings
                .iter()
                .any(|warning| warning.kind == Ps2CorruptionKind::InvalidFilename)
        );
        assert!(
            root.warnings
                .iter()
                .any(|warning| warning.kind == Ps2CorruptionKind::InvalidTimestamp)
        );
    }

    #[test]
    fn ps2_file_export_reconstructs_exact_bytes_and_leaves_card_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("Mcd001.ps2");
        let before = ps2_inventory_fixture();
        fs::write(&card_path, &before).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let file = &card.ps2_inventory.as_ref().unwrap().save_directories[0].files[0];
        let destination = dir.path().join("icon.sys");
        let plan = plan_ps2_file_export(&card, file, &destination).unwrap();
        let result = apply_ps2_file_export(&plan).unwrap();
        assert_eq!(result.bytes_written, 100);
        assert_eq!(fs::read(&destination).unwrap(), vec![0x5a; 100]);
        assert_eq!(result.sha256, sha256_hex(&[0x5a; 100]));
        assert_eq!(fs::read(&card_path).unwrap(), before);
    }

    #[test]
    fn ps2_file_export_reads_fragmented_chain_and_truncates_final_cluster() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("fragmented.ps2");
        let mut bytes = ps2_inventory_fixture();
        bytes[(41 + 3) * 1024..(41 + 3) * 1024 + 1024].fill(0x5a);
        bytes[8 * 1024 + 3 * 4..8 * 1024 + 4 * 4].copy_from_slice(&0x8000_0005u32.to_le_bytes());
        bytes[8 * 1024 + 5 * 4..8 * 1024 + 6 * 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        bytes[(41 + 5) * 1024..(41 + 5) * 1024 + 1024].fill(0x6b);
        let file_entry_offset = (41 + 4) * 1024;
        bytes[file_entry_offset + 4..file_entry_offset + 8].copy_from_slice(&1500u32.to_le_bytes());
        fs::write(&card_path, &bytes).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let file = &card.ps2_inventory.as_ref().unwrap().save_directories[0].files[0];
        assert_eq!(file.chain_health.clusters, vec![3, 5]);
        let destination = dir.path().join("fragment.bin");
        let plan = plan_ps2_file_export(&card, file, &destination).unwrap();
        apply_ps2_file_export(&plan).unwrap();
        let output = fs::read(&destination).unwrap();
        assert_eq!(output.len(), 1500);
        assert_eq!(&output[..1024], &[0x5a; 1024]);
        assert_eq!(&output[1024..], &[0x6b; 476]);
    }

    #[test]
    fn ps2_file_export_refuses_invalid_entries_and_existing_destinations() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("Mcd001.ps2");
        fs::write(&card_path, ps2_inventory_fixture()).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let file = &card.ps2_inventory.as_ref().unwrap().save_directories[0].files[0];
        let destination = dir.path().join("existing.bin");
        fs::write(&destination, b"keep").unwrap();
        assert!(matches!(
            plan_ps2_file_export(&card, file, &destination),
            Err(Ps2FileExportError::DestinationExists(_))
        ));
        assert!(matches!(
            plan_ps2_file_export(&card, file, &card_path),
            Err(Ps2FileExportError::UnsafeDestination(_))
        ));
        let mut invalid = file.clone();
        invalid.chain_health.complete = false;
        assert!(matches!(
            plan_ps2_file_export(&card, &invalid, &dir.path().join("invalid.bin")),
            Err(Ps2FileExportError::InvalidPlan(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ps2_file_export_refuses_symlink_destination() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("Mcd001.ps2");
        fs::write(&card_path, ps2_inventory_fixture()).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let file = &card.ps2_inventory.as_ref().unwrap().save_directories[0].files[0];
        let real = dir.path().join("outside.bin");
        fs::write(&real, b"keep").unwrap();
        let link = dir.path().join("export.bin");
        symlink(&real, &link).unwrap();
        assert!(matches!(
            plan_ps2_file_export(&card, file, &link),
            Err(Ps2FileExportError::DestinationExists(_))
        ));
        assert_eq!(fs::read(&real).unwrap(), b"keep");
    }

    #[test]
    fn ps2_file_export_supports_a_valid_zero_length_file() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("empty.ps2");
        let mut bytes = ps2_inventory_fixture();
        let entry_offset = (41 + 4) * 1024;
        bytes[entry_offset + 4..entry_offset + 8].copy_from_slice(&0u32.to_le_bytes());
        bytes[entry_offset + 0x10..entry_offset + 0x14].copy_from_slice(&u32::MAX.to_le_bytes());
        fs::write(&card_path, &bytes).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let file = &card.ps2_inventory.as_ref().unwrap().save_directories[0].files[0];
        assert!(file.chain_health.clusters.is_empty());
        let destination = dir.path().join("empty.bin");
        let plan = plan_ps2_file_export(&card, file, &destination).unwrap();
        let result = apply_ps2_file_export(&plan).unwrap();
        assert_eq!(result.bytes_written, 0);
        assert!(fs::read(&destination).unwrap().is_empty());
    }

    #[test]
    fn ps2_file_export_excludes_spare_bytes_from_528_byte_pages() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("spare.ps2");
        let mut bytes = ps2_fixture(PS2_PHYSICAL_PAGE_BYTES);
        let fat_offset = 8 * 2 * PS2_PHYSICAL_PAGE_BYTES;
        bytes[fat_offset + 3 * 4..fat_offset + 4 * 4]
            .copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        let first_page = (41 + 3) * 2 * PS2_PHYSICAL_PAGE_BYTES;
        bytes[first_page..first_page + PS2_PAGE_DATA_BYTES].fill(0x11);
        bytes[first_page + PS2_PAGE_DATA_BYTES..first_page + PS2_PHYSICAL_PAGE_BYTES].fill(0xee);
        let second_page = first_page + PS2_PHYSICAL_PAGE_BYTES;
        bytes[second_page..second_page + PS2_PAGE_DATA_BYTES].fill(0x22);
        bytes[second_page + PS2_PAGE_DATA_BYTES..second_page + PS2_PHYSICAL_PAGE_BYTES].fill(0xdd);
        fs::write(&card_path, &bytes).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let file = Ps2SaveFile {
            entry: Ps2DirectoryEntry {
                raw_entry_offset: 0,
                raw_mode: 0x8497,
                kind: Ps2DirectoryEntryKind::RegularFile,
                raw_name: b"raw.bin".to_vec(),
                display_name: "raw.bin".into(),
                length: 600,
                start_cluster: 3,
                parent_entry: 0,
                attributes: 0,
                created: None,
                modified: None,
                warnings: Vec::new(),
            },
            declared_size_bytes: 600,
            chain_health: Ps2ClusterChainHealth {
                clusters: vec![3],
                complete: true,
                warnings: Vec::new(),
            },
        };
        let destination = dir.path().join("raw.bin");
        let plan = plan_ps2_file_export(&card, &file, &destination).unwrap();
        apply_ps2_file_export(&plan).unwrap();
        let output = fs::read(&destination).unwrap();
        assert_eq!(output.len(), 600);
        assert_eq!(&output[..512], &[0x11; 512]);
        assert_eq!(&output[512..], &[0x22; 88]);
    }

    #[test]
    fn ps2_psu_export_is_deterministic_and_preserves_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("Mcd001.ps2");
        let before = ps2_inventory_fixture();
        fs::write(&card_path, &before).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let save = &card.ps2_inventory.as_ref().unwrap().save_directories[0];
        let first = dir.path().join("first.psu");
        let second = dir.path().join("second.psu");
        let first_plan = plan_ps2_psu_export(&card, save, &first).unwrap();
        let second_plan = plan_ps2_psu_export(&card, save, &second).unwrap();
        let first_result = apply_ps2_psu_export(&first_plan).unwrap();
        let second_result = apply_ps2_psu_export(&second_plan).unwrap();
        assert_eq!(first_result.file_count, 1);
        assert_eq!(first_result.output_bytes, 4 * 512 + 1024);
        assert_eq!(first_result.sha256, second_result.sha256);
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        assert_eq!(fs::read(&card_path).unwrap(), before);

        let output = fs::read(first).unwrap();
        assert_eq!(&output[0x40..0x40 + 16], b"BASLUS-00001TEST");
        assert_eq!(u32::from_le_bytes(output[4..8].try_into().unwrap()), 3);
        assert_eq!(&output[1024 + 0x40..1024 + 0x40 + 1], b".");
        assert_eq!(&output[1536 + 0x40..1536 + 0x40 + 8], b"icon.sys");
        assert_eq!(&output[2048 + 100..3072], &[0; 924]);
    }

    #[test]
    fn ps2_psu_export_handles_zero_length_and_fragmented_files() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("fragmented.ps2");
        let mut bytes = ps2_inventory_fixture();
        bytes[(41 + 3) * 1024..(41 + 3) * 1024 + 1024].fill(0x5a);
        bytes[8 * 1024 + 3 * 4..8 * 1024 + 4 * 4].copy_from_slice(&0x8000_0005u32.to_le_bytes());
        bytes[8 * 1024 + 5 * 4..8 * 1024 + 6 * 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        bytes[(41 + 5) * 1024..(41 + 5) * 1024 + 1024].fill(0x6b);
        let file_entry_offset = (41 + 4) * 1024;
        bytes[file_entry_offset + 4..file_entry_offset + 8].copy_from_slice(&1500u32.to_le_bytes());
        fs::write(&card_path, &bytes).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let mut save = card.ps2_inventory.as_ref().unwrap().save_directories[0].clone();
        assert_eq!(save.files[0].chain_health.clusters, vec![3, 5]);
        let mut empty = save.files[0].clone();
        empty.entry.raw_name = b"empty.bin".to_vec();
        empty.entry.display_name = "empty.bin".into();
        empty.entry.length = 0;
        empty.declared_size_bytes = 0;
        empty.entry.start_cluster = u32::MAX;
        empty.chain_health.clusters.clear();
        save.files.push(empty);
        let destination = dir.path().join("save.psu");
        let plan = plan_ps2_psu_export(&card, &save, &destination).unwrap();
        let result = apply_ps2_psu_export(&plan).unwrap();
        assert_eq!(result.file_count, 2);
        assert_eq!(result.output_bytes, 5 * 512 + 2048);
        let output = fs::read(destination).unwrap();
        assert_eq!(output.len(), 5 * 512 + 2048);
        // Every directory entry precedes every payload: the entry table is
        // root + "." + ".." + one entry per file, and only then the data pages.
        assert_eq!(&output[0x40..0x40 + 16], b"BASLUS-00001TEST");
        assert_eq!(&output[512 + 0x40..512 + 0x40 + 1], b".");
        assert_eq!(&output[1024 + 0x40..1024 + 0x40 + 2], b"..");
        assert_eq!(&output[1536 + 0x40..1536 + 0x40 + 8], b"icon.sys");
        assert_eq!(
            u32::from_le_bytes(output[1536 + 4..1536 + 8].try_into().unwrap()),
            1500
        );
        assert_eq!(&output[2048 + 0x40..2048 + 0x40 + 9], b"empty.bin");
        assert_eq!(
            u32::from_le_bytes(output[2048 + 4..2048 + 8].try_into().unwrap()),
            0
        );
        // The fragmented file spans clusters 3 and 5 and is padded to a whole
        // 1024-byte logical page; the zero-length file contributes no payload.
        assert_eq!(&output[2560..2560 + 1024], &[0x5a; 1024]);
        assert_eq!(&output[2560 + 1024..2560 + 1500], &[0x6b; 476]);
        assert_eq!(&output[2560 + 1500..], &[0; 548]);
    }

    #[test]
    fn ps2_psu_export_refuses_changed_source_and_unsafe_save() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("Mcd001.ps2");
        fs::write(&card_path, ps2_inventory_fixture()).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let save = &card.ps2_inventory.as_ref().unwrap().save_directories[0];
        let destination = dir.path().join("save.psu");
        let plan = plan_ps2_psu_export(&card, save, &destination).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&card_path)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        assert!(matches!(
            apply_ps2_psu_export(&plan),
            Err(Ps2PsuExportError::SourceChanged)
        ));

        let mut unsafe_save = save.clone();
        unsafe_save.files[0].entry.raw_name = b"../escape".to_vec();
        assert!(matches!(
            plan_ps2_psu_export(&card, &unsafe_save, &dir.path().join("unsafe.psu")),
            Err(Ps2PsuExportError::InvalidPlan(_))
        ));
    }

    #[test]
    fn ps2_psu_restore_replaces_save_with_verified_backup_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("Mcd001.ps2");
        let before = ps2_inventory_fixture();
        fs::write(&card_path, &before).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let save = &card.ps2_inventory.as_ref().unwrap().save_directories[0];
        let source_psu = dir.path().join("save.psu");
        let export = plan_ps2_psu_export(&card, save, &source_psu).unwrap();
        apply_ps2_psu_export(&export).unwrap();
        let backup = dir.path().join("before-restore.card");
        let plan = plan_ps2_psu_restore(&card, &source_psu, &backup, true).unwrap();
        assert!(plan.existing_save);
        assert_eq!(plan.required_clusters, 3);
        let result = apply_ps2_psu_restore(&plan).unwrap();
        assert_eq!(result.original_card_sha256, sha256_hex(&before));
        assert_eq!(result.card_size_bytes, before.len() as u64);
        assert_eq!(fs::read(&card_path).unwrap().len(), before.len());
        assert_eq!(
            sha256_hex(&fs::read(&backup).unwrap()),
            result.backup_sha256
        );
        assert_ne!(result.post_restore_card_sha256, result.original_card_sha256);
        undo_ps2_psu_restore(&result).unwrap();
        assert_eq!(fs::read(&card_path).unwrap(), before);
    }

    #[test]
    fn ps2_psu_restore_preview_rejects_conflicts_and_malformed_input_without_writes() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("Mcd001.ps2");
        let before = ps2_inventory_fixture();
        fs::write(&card_path, &before).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let save = &card.ps2_inventory.as_ref().unwrap().save_directories[0];
        let source_psu = dir.path().join("save.psu");
        let export = plan_ps2_psu_export(&card, save, &source_psu).unwrap();
        apply_ps2_psu_export(&export).unwrap();
        let backup = dir.path().join("before-restore.card");
        assert!(matches!(
            plan_ps2_psu_restore(&card, &source_psu, &backup, false),
            Err(Ps2PsuRestoreError::ExistingSave(_))
        ));
        assert_eq!(fs::read(&card_path).unwrap(), before);
        let malformed = dir.path().join("malformed.psu");
        fs::write(&malformed, [0u8; 2048]).unwrap();
        assert!(matches!(
            plan_ps2_psu_restore(&card, &malformed, &dir.path().join("malformed.card"), true),
            Err(Ps2PsuRestoreError::InvalidPlan(_))
        ));
        assert_eq!(fs::read(&card_path).unwrap(), before);
    }

    #[test]
    fn ps2_psu_restore_fails_closed_for_unhealthy_cards_low_space_and_stale_undo() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("Mcd001.ps2");
        let before = ps2_inventory_fixture();
        fs::write(&card_path, &before).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let save = &card.ps2_inventory.as_ref().unwrap().save_directories[0];
        let source_psu = dir.path().join("save.psu");
        let export = plan_ps2_psu_export(&card, save, &source_psu).unwrap();
        apply_ps2_psu_export(&export).unwrap();

        let mut unhealthy = card.clone();
        unhealthy.health = MemoryCardHealth::StructuralWarning;
        assert!(matches!(
            plan_ps2_psu_restore(
                &unhealthy,
                &source_psu,
                &dir.path().join("unhealthy.card"),
                true
            ),
            Err(Ps2PsuRestoreError::InvalidPlan(_))
        ));

        let mut no_space = card.clone();
        no_space.ps2_geometry.as_mut().unwrap().alloc_end = 44;
        assert!(matches!(
            plan_ps2_psu_restore(
                &no_space,
                &source_psu,
                &dir.path().join("no-space.card"),
                true
            ),
            Err(Ps2PsuRestoreError::InsufficientSpace { .. })
        ));

        let backup = dir.path().join("before-restore.card");
        let plan = plan_ps2_psu_restore(&card, &source_psu, &backup, true).unwrap();
        let result = apply_ps2_psu_restore(&plan).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&card_path)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        assert!(matches!(
            undo_ps2_psu_restore(&result),
            Err(Ps2PsuRestoreError::StaleUndo)
        ));
    }

    #[test]
    fn ps2_psu_export_excludes_spare_bytes_and_refuses_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let card_path = dir.path().join("Mcd001.ps2");
        let mut bytes = ps2_fixture(PS2_PHYSICAL_PAGE_BYTES);
        let fat_offset = 8 * 2 * PS2_PHYSICAL_PAGE_BYTES;
        bytes[fat_offset + 3 * 4..fat_offset + 4 * 4]
            .copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        let first_page = (41 + 3) * 2 * PS2_PHYSICAL_PAGE_BYTES;
        bytes[first_page..first_page + PS2_PAGE_DATA_BYTES].fill(0x11);
        bytes[first_page + PS2_PAGE_DATA_BYTES..first_page + PS2_PHYSICAL_PAGE_BYTES].fill(0xee);
        let second_page = first_page + PS2_PHYSICAL_PAGE_BYTES;
        bytes[second_page..second_page + PS2_PAGE_DATA_BYTES].fill(0x11);
        bytes[second_page + PS2_PAGE_DATA_BYTES..second_page + PS2_PHYSICAL_PAGE_BYTES].fill(0xdd);
        fs::write(&card_path, &bytes).unwrap();
        let card = inspect_memory_card(&card_path).unwrap();
        let save = Ps2SaveDirectory {
            entry: Ps2DirectoryEntry {
                raw_entry_offset: 0,
                raw_mode: 0x8427,
                kind: Ps2DirectoryEntryKind::Directory,
                raw_name: b"SAVE".to_vec(),
                display_name: "SAVE".into(),
                length: 1,
                start_cluster: 0,
                parent_entry: 0,
                attributes: 0,
                created: Some(Ps2Timestamp {
                    raw: [0, 38, 44, 12, 24, 6, 0xea, 0x07],
                    year: 2026,
                    month: 6,
                    day: 24,
                    hour: 12,
                    minute: 44,
                    second: 38,
                    timezone: "JST (+09:00)",
                }),
                modified: Some(Ps2Timestamp {
                    raw: [0, 39, 44, 12, 24, 6, 0xea, 0x07],
                    year: 2026,
                    month: 6,
                    day: 24,
                    hour: 12,
                    minute: 44,
                    second: 39,
                    timezone: "JST (+09:00)",
                }),
                warnings: Vec::new(),
            },
            chain_health: Ps2ClusterChainHealth {
                clusters: Vec::new(),
                complete: true,
                warnings: Vec::new(),
            },
            children: Vec::new(),
            files: vec![Ps2SaveFile {
                entry: Ps2DirectoryEntry {
                    raw_entry_offset: 0,
                    raw_mode: 0x8497,
                    kind: Ps2DirectoryEntryKind::RegularFile,
                    raw_name: b"raw.bin".to_vec(),
                    display_name: "raw.bin".into(),
                    length: 600,
                    start_cluster: 3,
                    parent_entry: 0,
                    attributes: 0,
                    created: save_timestamp(38),
                    modified: save_timestamp(39),
                    warnings: Vec::new(),
                },
                declared_size_bytes: 600,
                chain_health: Ps2ClusterChainHealth {
                    clusters: vec![3],
                    complete: true,
                    warnings: Vec::new(),
                },
            }],
            warnings: Vec::new(),
        };
        let destination = dir.path().join("save.psu");
        let plan = plan_ps2_psu_export(&card, &save, &destination).unwrap();
        apply_ps2_psu_export(&plan).unwrap();
        let output = fs::read(&destination).unwrap();
        assert_eq!(&output[2048..2048 + 512], &[0x11; 512]);
        assert_eq!(&output[2560..2560 + 88], &[0x11; 88]);
        assert!(matches!(
            plan_ps2_psu_export(&card, &save, &destination),
            Err(Ps2PsuExportError::DestinationExists(_))
        ));
    }

    fn save_timestamp(second: u8) -> Option<Ps2Timestamp> {
        Some(Ps2Timestamp {
            raw: [0, second, 44, 12, 24, 6, 0xea, 0x07],
            year: 2026,
            month: 6,
            day: 24,
            hour: 12,
            minute: 44,
            second,
            timezone: "JST (+09:00)",
        })
    }
}
