//! Bounded preview and verified native WUD ↔ WUX conversion.
//! Publication reuses the journaled Repair transaction engine.

use std::fs;
use std::path::{Path, PathBuf};

use crate::wiiu_disc::{WiiUDiscFormat, WiiUDiscInspection, WiiUDiscIssue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUConversionDirection {
    WudToWux,
    WuxToWud,
}

impl WiiUConversionDirection {
    pub fn source_format(self) -> WiiUDiscFormat {
        match self {
            Self::WudToWux => WiiUDiscFormat::Wud,
            Self::WuxToWud => WiiUDiscFormat::Wux,
        }
    }

    pub fn target_format(self) -> WiiUDiscFormat {
        match self {
            Self::WudToWux => WiiUDiscFormat::Wux,
            Self::WuxToWud => WiiUDiscFormat::Wud,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUConversionReadiness {
    ReadyToPreview,
    ReadyIfToolAvailable,
    VerificationRequired,
    NotReady,
    Unsupported,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUConversionToolStatus {
    Missing,
    VersionUnknown,
    CapabilityUnknown,
    Supported,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUConversionToolCapability {
    pub name: String,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub status: WiiUConversionToolStatus,
    pub directions: Vec<WiiUConversionDirection>,
    pub source: String,
    pub modifies_source_in_place: bool,
    pub output_naming: String,
    pub license_provenance: String,
}

impl WiiUConversionToolCapability {
    pub fn supports(&self, direction: WiiUConversionDirection) -> bool {
        self.status == WiiUConversionToolStatus::Supported && self.directions.contains(&direction)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WiiUConversionToolInventory {
    pub tools: Vec<WiiUConversionToolCapability>,
}

/// Compatibility entry point for existing preview callers. Native conversion
/// does not discover or execute external tools.
pub fn probe_wiiu_conversion_tools() -> WiiUConversionToolInventory {
    WiiUConversionToolInventory::default()
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUConversionIdentity {
    HashAvailable { algorithm: String, value: String },
    HashMissing,
    HashStale,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUConversionSpaceEstimate {
    pub source_bytes: u64,
    pub destination_exact_bytes: Option<u64>,
    pub destination_maximum_bytes: Option<u64>,
    pub destination_description: String,
    pub temporary_bytes: Option<u64>,
    pub atomic_duplicate_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub sufficient: Option<bool>,
    pub safety_margin_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUConversionVerificationPlan {
    pub required: bool,
    pub exact_identity_provable: bool,
    pub steps: Vec<String>,
    pub source_identity: WiiUConversionIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WiiUConversionRefusal {
    WrongSourceFormat {
        expected: WiiUDiscFormat,
        actual: WiiUDiscFormat,
    },
    IncompleteSource(Vec<WiiUDiscIssue>),
    SourcePathUnsafe,
    DestinationPathUnsafe,
    DestinationIsSource,
    DestinationExists,
    HashStale,
    HashMissingForVerification,
    ToolUnavailable,
    ToolCapabilityUnproven,
    ToolDoesNotSupportDirection,
    InsufficientDestinationSpace {
        required: u64,
        available: u64,
    },
    OutputSizeUnknown,
    UnsupportedFormat(WiiUDiscFormat),
    AmbiguousSplit,
    DeferredDirection,
    InvalidSourceIdentity,
    InvalidWriterLayout(WiiUDiscIssue),
}

/// One interoperable, deterministic encoding, not a tunable block-size range.
pub const WIIU_WUX_CANONICAL_BLOCK_BYTES: u32 = crate::wiiu_disc::WUD_SECTOR_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUWuxCreationLayout {
    pub block_size_bytes: u32,
    pub logical_size_bytes: u64,
    pub logical_block_count: u64,
    pub lookup_table_bytes: u64,
    pub payload_offset: u64,
    pub maximum_output_bytes: u64,
}
impl WiiUWuxCreationLayout {
    pub(crate) fn for_size(logical: u64) -> Result<Self, WiiUDiscIssue> {
        crate::wiiu_disc::validate_logical_size(logical)?;
        let overflow = || WiiUDiscIssue::LogicalSizeOverflow;
        let block = u64::from(WIIU_WUX_CANONICAL_BLOCK_BYTES);
        let count = logical.checked_div(block).ok_or_else(overflow)?;
        let table = count.checked_mul(4).ok_or_else(overflow)?;
        if table > crate::wiiu_disc::MAX_WUX_TABLE_BYTES
            || usize::try_from(count).is_err()
            || u32::try_from(count).is_err()
        {
            return Err(WiiUDiscIssue::AbsurdBlockCount(count));
        }
        let payload_offset = crate::wiiu_disc::checked_align(
            crate::wiiu_disc::WUX_HEADER
                .checked_add(table)
                .ok_or_else(overflow)?,
            block,
        )
        .ok_or_else(overflow)?;
        Ok(Self {
            block_size_bytes: WIIU_WUX_CANONICAL_BLOCK_BYTES,
            logical_size_bytes: logical,
            logical_block_count: count,
            lookup_table_bytes: table,
            payload_offset,
            maximum_output_bytes: payload_offset.checked_add(logical).ok_or_else(overflow)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUWuxCreationStatistics {
    /// All logical zero blocks, including the first physically stored one.
    pub zero_blocks: u64,
    pub reused_zero_blocks: u64,
    /// Includes the shared zero block if present. Non-zero duplicates stay stored.
    pub stored_blocks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUConversionPlan {
    pub source: PathBuf,
    pub source_parts: Vec<PathBuf>,
    pub destination: PathBuf,
    pub direction: WiiUConversionDirection,
    pub source_format: WiiUDiscFormat,
    pub target_format: WiiUDiscFormat,
    pub source_inspection: WiiUDiscInspection,
    pub source_identity: WiiUConversionIdentity,
    pub tool: Option<WiiUConversionToolCapability>,
    pub readiness: WiiUConversionReadiness,
    pub refusals: Vec<WiiUConversionRefusal>,
    pub warnings: Vec<String>,
    pub space: WiiUConversionSpaceEstimate,
    pub verification: WiiUConversionVerificationPlan,
    pub source_immutable: bool,
    pub keys_required: bool,
    pub provenance: String,
    pub estimated_blocks: Option<u64>,
    pub no_clobber: bool,
    pub post_write_verification_available: bool,
    pub wux_creation: Option<WiiUWuxCreationLayout>,
    binding: Option<(
        PathBuf,
        PathBuf,
        crate::wiiu_disc::WiiUDiscEvidence,
        WiiUConversionIdentity,
    )>,
    // Queues saved before encoding support have the four-part binding above.
    // Their only executable direction was WUX -> WUD; retain that binding.
    binding_direction: Option<WiiUConversionDirection>,
}

#[derive(Debug, Clone)]
pub struct WiiUConversionRequest {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub direction: WiiUConversionDirection,
    pub source_identity: WiiUConversionIdentity,
    pub available_free_space: Option<u64>,
    pub tools: WiiUConversionToolInventory,
}

fn safe_parent(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let mut current = PathBuf::new();
    for c in path.components() {
        if !matches!(
            c,
            std::path::Component::RootDir | std::path::Component::Normal(_)
        ) {
            return false;
        }
        current.push(c.as_os_str());
        if !fs::symlink_metadata(&current).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            return false;
        }
    }
    true
}
fn safe_destination(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(crate::dat::rename_apply::preflight::is_safe_basename)
        && path.parent().is_some_and(safe_parent)
}
fn source_parts(report: &WiiUDiscInspection) -> Vec<PathBuf> {
    report
        .structure
        .as_ref()
        .map(|s| s.parts.iter().map(|p| p.path.clone()).collect())
        .unwrap_or_default()
}
pub(crate) fn digest_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
fn valid_sha256(algorithm: &str, value: &str) -> bool {
    algorithm.eq_ignore_ascii_case("sha256")
        && value.len() == 64
        && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Read-only: hashes only bounded structural evidence, never creates a stage or
/// journal. HashAvailable is an optional SHA-256 of the physical SOURCE file,
/// not a purported hash of the uncompressed original disc.
pub fn plan_wiiu_conversion(request: &WiiUConversionRequest) -> WiiUConversionPlan {
    let report = crate::wiiu_disc::inspect_wii_u_disc(&request.source);
    let creating_wux = request.direction == WiiUConversionDirection::WudToWux;
    let mut refusals = Vec::new();
    if report.format != request.direction.source_format() {
        refusals.push(WiiUConversionRefusal::WrongSourceFormat {
            expected: request.direction.source_format(),
            actual: report.format,
        });
    }
    if !report.structural_complete {
        refusals.push(WiiUConversionRefusal::IncompleteSource(
            report.issues.clone(),
        ));
    }
    if report.source_evidence.is_none() {
        refusals.push(WiiUConversionRefusal::SourcePathUnsafe);
    }
    if !safe_destination(&request.destination) {
        refusals.push(WiiUConversionRefusal::DestinationPathUnsafe);
    }
    if request.destination == request.source {
        refusals.push(WiiUConversionRefusal::DestinationIsSource);
    }
    // Includes dangling symlinks, unlike Path::exists().
    match fs::symlink_metadata(&request.destination) {
        Ok(_) => refusals.push(WiiUConversionRefusal::DestinationExists),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => refusals.push(WiiUConversionRefusal::DestinationPathUnsafe),
    }
    match &request.source_identity {
        WiiUConversionIdentity::HashStale => refusals.push(WiiUConversionRefusal::HashStale),
        WiiUConversionIdentity::HashAvailable { algorithm, value }
            if !valid_sha256(algorithm, value) =>
        {
            refusals.push(WiiUConversionRefusal::InvalidSourceIdentity)
        }
        _ => {}
    }
    let source = report
        .structure
        .as_ref()
        .map_or(0, |s| s.physical_container_size_bytes);
    let exact = (!creating_wux)
        .then(|| {
            report
                .structure
                .as_ref()
                .and_then(|s| s.logical_disc_size_bytes)
        })
        .flatten();
    let wux_creation = if creating_wux && report.structural_complete {
        match WiiUWuxCreationLayout::for_size(source) {
            Ok(layout) => Some(layout),
            Err(issue) => {
                refusals.push(WiiUConversionRefusal::InvalidWriterLayout(issue));
                None
            }
        }
    } else {
        None
    };
    let maximum = wux_creation.map(|l| l.maximum_output_bytes).or(exact);
    let available = request.available_free_space.or_else(|| {
        request
            .destination
            .parent()
            .and_then(crate::diagnostics::environment::filesystem_stat)
            .map(|s| s.available_bytes)
    });
    if let (Some(required), Some(available)) = (maximum, available)
        && available < required
    {
        refusals.push(WiiUConversionRefusal::InsufficientDestinationSpace {
            required,
            available,
        });
    }
    let space = WiiUConversionSpaceEstimate {
        source_bytes: source,
        destination_exact_bytes: exact,
        destination_maximum_bytes: maximum,
        destination_description: exact
            .map(|n| format!("exact logical WUD size: {n} bytes"))
            .or_else(|| {
                maximum.map(|n| {
                    format!(
                        "canonical WUX upper bound: {n} bytes; zero sharing determines actual size"
                    )
                })
            })
            .unwrap_or_else(|| "valid output geometry unavailable".into()),
        // Same-filesystem staging becomes the destination by atomic rename.
        temporary_bytes: maximum,
        atomic_duplicate_bytes: Some(0),
        available_bytes: available,
        sufficient: maximum.zip(available).map(|(n, a)| a >= n),
        safety_margin_bytes: 0,
    };
    let readiness = if report.format == WiiUDiscFormat::Wua {
        WiiUConversionReadiness::Unsupported
    } else if refusals.is_empty() {
        WiiUConversionReadiness::ReadyToPreview
    } else {
        WiiUConversionReadiness::NotReady
    };
    let binding = report.source_evidence.clone().map(|e| {
        (
            request.source.clone(),
            request.destination.clone(),
            e,
            request.source_identity.clone(),
        )
    });
    let estimated_blocks = wux_creation
        .map(|l| l.logical_block_count)
        .or_else(|| report.structure.as_ref().and_then(|s| s.block_count));
    let mut warnings: Vec<String> = report.issues.iter().map(|i| format!("{i:?}")).collect();
    if let Some(unreferenced) = report
        .structure
        .as_ref()
        .and_then(|s| s.unreferenced_payload_block_count)
        .filter(|n| *n > 0)
    {
        warnings.push(format!(
            "Source WUX contains {unreferenced} physical payload block(s) not referenced by its sector map; the WUD output will contain the mapped logical disc stream only. The source WUX remains unchanged."
        ));
    }
    WiiUConversionPlan {
        source: request.source.clone(),
        source_parts: source_parts(&report),
        destination: request.destination.clone(),
        direction: request.direction,
        source_format: report.format,
        target_format: request.direction.target_format(),
        source_inspection: report,
        source_identity: request.source_identity.clone(),
        tool: None,
        readiness,
        refusals,
        warnings,
        space,
        verification: WiiUConversionVerificationPlan {
            required: true,
            exact_identity_provable: true,
            steps: vec![
                "revalidate bounded source binding before writes".into(),
                if creating_wux {
                    "encode sequential 32 KiB blocks; share only exactly zero blocks".into()
                } else {
                    "stream sectors in logical WUD order and hash reconstructed bytes".into()
                },
                "inspect staged output; compare logical size and complete WUD header evidence"
                    .into(),
                if creating_wux {
                    "stream staged WUX through the existing logical reader; compare full SHA-256 with source WUD".into()
                } else {
                    "verify exact output size and independent full-file SHA-256".into()
                },
                "revalidate full source hash; publish with journaled no-clobber move".into(),
            ],
            source_identity: request.source_identity.clone(),
        },
        source_immutable: true,
        keys_required: false,
        provenance: if creating_wux {
            "native-wud-to-wux-zero-sharing-v1"
        } else {
            "native-wux-to-wud-v1"
        }
        .into(),
        estimated_blocks,
        no_clobber: true,
        post_write_verification_available: true,
        wux_creation,
        binding,
        binding_direction: Some(request.direction),
    }
}

#[derive(Debug)]
pub enum WiiUConversionError {
    Refused(Vec<WiiUConversionRefusal>),
    StalePlan,
    Cancelled,
    InvalidSource(WiiUDiscIssue),
    VerificationFailed(String),
    Io(std::io::Error),
    /// Recovery must retain this directory because the transaction may have
    /// reached publication before an I/O/journal failure was reported.
    Transaction {
        detail: String,
        recovery_directory: PathBuf,
    },
}
impl std::fmt::Display for WiiUConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for WiiUConversionError {}
impl From<std::io::Error> for WiiUConversionError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUConversionProgress {
    /// Logical bytes processed, also for WUX creation (not physical WUX bytes).
    pub written_bytes: u64,
    pub expected_bytes: u64,
    pub completed_blocks: u64,
    pub total_blocks: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WiiUConversionRecord {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub output_bytes: u64,
    pub source_container_sha256: String,
    pub reconstructed_wud_sha256: String,
    pub output_sha256: String,
    pub header_sha256: String,
    pub source_header: crate::wiiu_disc::WiiUHeaderEvidence,
    pub wux_creation: Option<WiiUWuxCreationStatistics>,
    pub policy: String,
    pub transaction_id: String,
}
fn check_cancel(cancel: &std::sync::atomic::AtomicBool) -> Result<(), WiiUConversionError> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        Err(WiiUConversionError::Cancelled)
    } else {
        Ok(())
    }
}
pub(crate) fn revalidate(
    plan: &WiiUConversionPlan,
    staging_needed: bool,
) -> Result<WiiUDiscInspection, WiiUConversionError> {
    let Some((source, destination, evidence, identity)) = &plan.binding else {
        return Err(WiiUConversionError::StalePlan);
    };
    let direction = plan
        .binding_direction
        .unwrap_or(WiiUConversionDirection::WuxToWud);
    if source != &plan.source
        || destination != &plan.destination
        || identity != &plan.source_identity
        || direction != plan.direction
        || plan.source_format != direction.source_format()
        || plan.target_format != direction.target_format()
    {
        return Err(WiiUConversionError::StalePlan);
    }
    let fresh = plan_wiiu_conversion(&WiiUConversionRequest {
        source: source.clone(),
        destination: destination.clone(),
        direction: plan.direction,
        source_identity: identity.clone(),
        // Once the stage exists, that output space is already occupied; do not
        // demand another full image's free space during publication checks.
        available_free_space: if staging_needed { None } else { Some(u64::MAX) },
        tools: WiiUConversionToolInventory::default(),
    });
    let report = fresh.source_inspection;
    if !report.structural_complete
        || report.source_evidence.as_ref() != Some(evidence)
        || report != plan.source_inspection
        || fresh.space.destination_exact_bytes != plan.space.destination_exact_bytes
        // Pre-encoder queue plans bind the exact decode size, without a maximum.
        || fresh.space.destination_maximum_bytes
            != plan.space.destination_maximum_bytes.or(plan.space.destination_exact_bytes)
        || fresh.space.temporary_bytes != plan.space.temporary_bytes
        || fresh.wux_creation != plan.wux_creation
    {
        return Err(WiiUConversionError::StalePlan);
    }
    if !fresh.refusals.is_empty() {
        return Err(WiiUConversionError::Refused(fresh.refusals));
    }
    Ok(report)
}

pub const WIIU_CONVERSION_CHUNK_BYTES: usize = 64 * 1024;

/// The format has stored-sector references, not sparse sentinels. Store the
/// first zero block and share it; store every non-zero block in encounter order.
fn encode_wud(
    source: &mut fs::File,
    output: &mut fs::File,
    layout: WiiUWuxCreationLayout,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut dyn FnMut(WiiUConversionProgress),
) -> Result<(String, WiiUWuxCreationStatistics), WiiUConversionError> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek, SeekFrom, Write};

    if WiiUWuxCreationLayout::for_size(source.metadata()?.len())
        .map_err(WiiUConversionError::InvalidSource)?
        != layout
    {
        return Err(WiiUConversionError::StalePlan);
    }
    let overflow = || WiiUConversionError::InvalidSource(WiiUDiscIssue::LogicalSizeOverflow);
    let mut table = Vec::<u32>::new();
    // Geometry and the allocation cap are proven before allocating metadata.
    table
        .try_reserve_exact(usize::try_from(layout.logical_block_count).map_err(|_| overflow())?)
        .map_err(|e| std::io::Error::other(format!("WUX table allocation failed: {e}")))?;
    let mut header = [0_u8; crate::wiiu_disc::WUX_HEADER as usize];
    header[..4].copy_from_slice(b"WUX0");
    header[4..8].copy_from_slice(&crate::wiiu_disc::MAGIC1.to_le_bytes());
    header[8..12].copy_from_slice(&layout.block_size_bytes.to_le_bytes());
    header[16..24].copy_from_slice(&layout.logical_size_bytes.to_le_bytes());
    output.write_all(&header)?;
    let zeros = [0_u8; WIIU_WUX_CANONICAL_BLOCK_BYTES as usize];
    let mut padding = layout
        .payload_offset
        .checked_sub(crate::wiiu_disc::WUX_HEADER)
        .ok_or_else(overflow)?;
    while padding > 0 {
        check_cancel(cancel)?;
        let length = padding.min(zeros.len() as u64) as usize;
        output.write_all(&zeros[..length])?;
        padding = padding.checked_sub(length as u64).ok_or_else(overflow)?;
    }
    source.seek(SeekFrom::Start(0))?;
    let mut buffer = [0_u8; WIIU_WUX_CANONICAL_BLOCK_BYTES as usize];
    let mut digest = Sha256::new();
    let mut zero_index = None;
    let mut stored = 0_u32;
    let mut zero_blocks = 0_u64;
    let mut processed = 0_u64;
    for completed in 0..layout.logical_block_count {
        check_cancel(cancel)?;
        source.read_exact(&mut buffer)?;
        digest.update(buffer);
        let is_zero = buffer == zeros;
        if is_zero {
            zero_blocks = zero_blocks.checked_add(1).ok_or_else(overflow)?;
        }
        let index = if is_zero && let Some(index) = zero_index {
            index
        } else {
            let index = stored;
            output.write_all(&buffer)?;
            stored = stored.checked_add(1).ok_or_else(overflow)?;
            if is_zero {
                zero_index = Some(index);
            }
            index
        };
        table.push(index);
        processed = processed
            .checked_add(u64::from(layout.block_size_bytes))
            .ok_or_else(overflow)?;
        if processed < layout.logical_size_bytes {
            progress(WiiUConversionProgress {
                written_bytes: processed,
                expected_bytes: layout.logical_size_bytes,
                completed_blocks: completed.checked_add(1).ok_or_else(overflow)?,
                total_blocks: layout.logical_block_count,
            });
        }
    }
    check_cancel(cancel)?;
    output.seek(SeekFrom::Start(crate::wiiu_disc::WUX_HEADER))?;
    let mut entries = std::io::BufWriter::new(output);
    for (i, index) in table.iter().enumerate() {
        if i.is_multiple_of(2048) {
            check_cancel(cancel)?;
        }
        entries.write_all(&index.to_le_bytes())?;
    }
    entries.flush()?;
    progress(WiiUConversionProgress {
        written_bytes: processed,
        expected_bytes: layout.logical_size_bytes,
        completed_blocks: layout.logical_block_count,
        total_blocks: layout.logical_block_count,
    });
    Ok((
        digest_hex(digest.finalize()),
        WiiUWuxCreationStatistics {
            zero_blocks,
            reused_zero_blocks: zero_blocks.checked_sub(1).unwrap_or(0),
            stored_blocks: u64::from(stored),
        },
    ))
}

/// Reconstruct through the existing reader into a hash, without a second WUD.
fn hash_staged_wux(
    path: &Path,
    inspection: &WiiUDiscInspection,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, WiiUConversionError> {
    use sha2::{Digest, Sha256};
    let evidence = inspection
        .source_evidence
        .as_ref()
        .ok_or_else(|| WiiUConversionError::VerificationFailed("missing staged evidence".into()))?;
    let mut file =
        crate::safe_read::open_bounded_read(path, &crate::safe_read::TrustedRoots::none())
            .map_err(|e| WiiUConversionError::VerificationFailed(format!("{e:?}")))?
            .into_file();
    if !evidence.matches_metadata(&file.metadata()?) {
        return Err(WiiUConversionError::VerificationFailed(
            "staged WUX changed".into(),
        ));
    }
    let layout = crate::wiiu_disc::load_wux(&mut file, evidence.size_bytes)
        .map_err(WiiUConversionError::InvalidSource)?;
    let mut buffer = [0_u8; WIIU_CONVERSION_CHUNK_BYTES];
    let mut digest = Sha256::new();
    let mut offset = 0_u64;
    while offset < layout.logical {
        check_cancel(cancel)?;
        let length = (layout.logical - offset).min(buffer.len() as u64) as usize;
        crate::wiiu_disc::read_wux_at(&mut file, &layout, offset, &mut buffer[..length])
            .map_err(WiiUConversionError::InvalidSource)?;
        digest.update(&buffer[..length]);
        offset = offset
            .checked_add(length as u64)
            .ok_or(WiiUConversionError::InvalidSource(
                WiiUDiscIssue::LogicalSizeOverflow,
            ))?;
    }
    if !evidence.matches_metadata(&file.metadata()?)
        || !evidence.matches_metadata(&fs::symlink_metadata(path)?)
    {
        return Err(WiiUConversionError::VerificationFailed(
            "staged WUX changed".into(),
        ));
    }
    Ok(digest_hex(digest.finalize()))
}

/// Native streaming conversion, explicitly invoked after preview approval.
/// Full identity capture and Repair's hash checks use existing fixed buffers;
/// cancellation is checked per decode chunk and at phase boundaries.
pub fn execute_wiiu_conversion(
    plan: &WiiUConversionPlan,
    options: &crate::repair::execute::RepairExecutionOptions,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut dyn FnMut(WiiUConversionProgress),
) -> Result<
    (
        WiiUConversionRecord,
        crate::repair::execute::RepairTransactionResult,
    ),
    WiiUConversionError,
> {
    execute_wiiu_conversion_in_stage(plan, options, cancel, progress, None)
}

/// Queue attempts provide a persisted, same-filesystem staging parent. The
/// ordinary executor retains its existing destination-adjacent staging policy.
pub(crate) fn execute_wiiu_conversion_in_stage(
    plan: &WiiUConversionPlan,
    options: &crate::repair::execute::RepairExecutionOptions,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut dyn FnMut(WiiUConversionProgress),
    staging_parent: Option<&Path>,
) -> Result<
    (
        WiiUConversionRecord,
        crate::repair::execute::RepairTransactionResult,
    ),
    WiiUConversionError,
> {
    use crate::dat::rename_apply::identity::{capture_identity, identity_matches};
    use crate::repair::execute::{
        RepairApplyExecution, RepairReverifyOutcome, apply_repair_transaction,
        build_repair_transaction,
    };
    use crate::repair::plan::{RepairPlanId, build_repair_plan};
    use crate::repair::proposal::{
        RepairAction, RepairEvidence, RepairEvidenceKind, RepairProposal, RepairProposalId,
        SafetyState,
    };
    use sha2::{Digest, Sha256};
    use std::io::Write;

    check_cancel(cancel)?;
    if !plan.refusals.is_empty() {
        return Err(WiiUConversionError::Refused(plan.refusals.clone()));
    }
    let report = revalidate(plan, true)?;
    let source_identity = capture_identity(&plan.source)?;
    let source_sha = digest_hex(
        source_identity
            .freshness
            .as_ref()
            .ok_or(WiiUConversionError::StalePlan)?
            .sha256,
    );
    if let WiiUConversionIdentity::HashAvailable { value, .. } = &plan.source_identity
        && !value.eq_ignore_ascii_case(&source_sha)
    {
        return Err(WiiUConversionError::StalePlan);
    }
    revalidate(plan, true)?;
    let mut source =
        crate::safe_read::open_bounded_read(&plan.source, &crate::safe_read::TrustedRoots::none())
            .map_err(|e| WiiUConversionError::VerificationFailed(format!("{e:?}")))?
            .into_file();
    let evidence = report.source_evidence.as_ref().unwrap();
    if !evidence.matches_metadata(&source.metadata()?) {
        return Err(WiiUConversionError::StalePlan);
    }
    let logical = report
        .structure
        .as_ref()
        .unwrap()
        .logical_disc_size_bytes
        .unwrap();
    let creating_wux = plan.direction == WiiUConversionDirection::WudToWux;
    let parent = plan
        .destination
        .parent()
        .ok_or(WiiUConversionError::StalePlan)?;
    let stage = tempfile::Builder::new()
        .prefix(".emuwiz-wiiu-")
        .tempdir_in(staging_parent.unwrap_or(parent))?;
    let staged = stage.path().join(if creating_wux {
        "output.wux"
    } else {
        "output.wud"
    });
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    let (reconstructed_sha, creation_statistics) =
        if let Some(layout) = plan.wux_creation {
            let (streamed_source_sha, statistics) =
                encode_wud(&mut source, &mut output, layout, cancel, progress)?;
            if streamed_source_sha != source_sha {
                return Err(WiiUConversionError::StalePlan);
            }
            (streamed_source_sha, Some(statistics))
        } else {
            let layout = crate::wiiu_disc::load_wux(&mut source, evidence.size_bytes)
                .map_err(WiiUConversionError::InvalidSource)?;
            if !evidence.matches_metadata(&source.metadata()?) {
                return Err(WiiUConversionError::StalePlan);
            }
            let mut buffer = [0_u8; WIIU_CONVERSION_CHUNK_BYTES];
            let mut digest = Sha256::new();
            let mut written = 0_u64;
            while written < layout.logical {
                check_cancel(cancel)?;
                let length = (layout.logical - written).min(buffer.len() as u64) as usize;
                crate::wiiu_disc::read_wux_at(&mut source, &layout, written, &mut buffer[..length])
                    .map_err(WiiUConversionError::InvalidSource)?;
                output.write_all(&buffer[..length])?;
                digest.update(&buffer[..length]);
                written = written.checked_add(length as u64).ok_or(
                    WiiUConversionError::InvalidSource(WiiUDiscIssue::LogicalSizeOverflow),
                )?;
                progress(WiiUConversionProgress {
                    written_bytes: written,
                    expected_bytes: layout.logical,
                    completed_blocks: if written == layout.logical {
                        layout.table.len() as u64
                    } else {
                        written / u64::from(layout.sector)
                    },
                    total_blocks: layout.table.len() as u64,
                });
            }
            // Release the bounded table before later source inspection allocates one.
            (digest_hex(digest.finalize()), None)
        };
    check_cancel(cancel)?;
    output.sync_all()?;
    let output_bytes = output.metadata()?.len();
    let expected_output_bytes =
        if let (Some(layout), Some(stats)) = (plan.wux_creation, creation_statistics) {
            stats
                .stored_blocks
                .checked_mul(u64::from(layout.block_size_bytes))
                .and_then(|n| layout.payload_offset.checked_add(n))
                .ok_or(WiiUConversionError::InvalidSource(
                    WiiUDiscIssue::LogicalSizeOverflow,
                ))?
        } else {
            logical
        };
    if output_bytes != expected_output_bytes {
        return Err(WiiUConversionError::VerificationFailed(
            "output size differs".into(),
        ));
    }
    drop(output);
    let inspected_output = crate::wiiu_disc::inspect_wii_u_disc(&staged);
    let source_header = report
        .structure
        .as_ref()
        .and_then(|s| s.wud_header.as_ref())
        .unwrap();
    if !inspected_output.structural_complete
        || inspected_output.format != plan.target_format
        || inspected_output
            .structure
            .as_ref()
            .and_then(|s| s.logical_disc_size_bytes)
            != Some(logical)
        || inspected_output
            .structure
            .as_ref()
            .and_then(|s| s.wud_header.as_ref())
            != Some(source_header)
    {
        return Err(WiiUConversionError::VerificationFailed(
            "staged logical size/header evidence differs from source".into(),
        ));
    }
    if let Some(stats) = creation_statistics {
        let structure = inspected_output.structure.as_ref().unwrap();
        if structure.sector_size_bytes != Some(WIIU_WUX_CANONICAL_BLOCK_BYTES)
            || structure.referenced_block_count != Some(stats.stored_blocks)
            || structure.repeated_block_count != Some(stats.reused_zero_blocks)
            || hash_staged_wux(&staged, &inspected_output, cancel)? != reconstructed_sha
        {
            return Err(WiiUConversionError::VerificationFailed(
                "WUX round-trip SHA-256 or canonical mapping differs".into(),
            ));
        }
    }
    let output_identity = capture_identity(&staged)?;
    let output_sha = digest_hex(output_identity.freshness.as_ref().unwrap().sha256);
    if output_identity.size_bytes != expected_output_bytes
        || (!creating_wux && output_sha != reconstructed_sha)
        || !inspected_output
            .source_evidence
            .as_ref()
            .unwrap()
            .matches_metadata(&fs::symlink_metadata(&staged)?)
    {
        return Err(WiiUConversionError::VerificationFailed(
            "independent output hash/size differs".into(),
        ));
    }
    if !identity_matches(&source_identity, &capture_identity(&plan.source)?)
        || !evidence.matches_metadata(&source.metadata()?)
    {
        return Err(WiiUConversionError::StalePlan);
    }
    revalidate(plan, false)?;
    check_cancel(cancel)?;
    let policy = if creating_wux {
        "native-wud-to-wux-zero-sharing-v1"
    } else {
        "native-wux-to-wud-v1"
    };
    let proposal = RepairProposal {
        id: RepairProposalId::new("wiiu-output").unwrap(),
        action: RepairAction::MovePath {
            destination: plan.destination.clone(),
        },
        source_path: staged.clone(),
        reason: "publish independently verified native Wii U conversion output".into(),
        evidence: vec![RepairEvidence::new(
            RepairEvidenceKind::UserRequestedOrganisation,
            "exact size, SHA-256 and WUD header evidence verified",
        )],
        expected_source_identity: Some(output_identity),
        originating_audit: None,
        safety: SafetyState::Safe,
        blockers: vec![],
        warnings: vec![],
        dat_source_id: None,
        dat_source_display: None,
        game_name: None,
        rom_name: None,
        verdict_label: Some(format!("Verified {:?} output", plan.target_format)),
        match_confident: true,
        is_outer_archive: false,
        is_outer_archive_verified: false,
        survivor_path: None,
    };
    let publication = build_repair_plan(
        RepairPlanId::new("wiiu-output-v1").unwrap(),
        0,
        crate::dat::sources::now_unix(),
        Some(policy.into()),
        vec![proposal],
    );
    let mut transaction = build_repair_transaction(&publication)
        .map_err(|e| WiiUConversionError::VerificationFailed(e.to_string()))?;
    let record = WiiUConversionRecord {
        source: plan.source.clone(),
        destination: plan.destination.clone(),
        output_bytes,
        source_container_sha256: source_sha,
        reconstructed_wud_sha256: reconstructed_sha,
        output_sha256: output_sha,
        header_sha256: digest_hex(source_header.header_sha256),
        source_header: source_header.clone(),
        wux_creation: creation_statistics,
        policy: policy.into(),
        transaction_id: transaction.transaction_id.clone(),
    };
    transaction.unknown.insert(
        "wiiu_conversion".into(),
        serde_json::json!({
            "policy": record.policy, "source": record.source, "destination": record.destination,
            "source_container_sha256": record.source_container_sha256,
            "reconstructed_wud_sha256": record.reconstructed_wud_sha256,
            "output_sha256": record.output_sha256, "header_sha256": record.header_sha256,
            "output_bytes": record.output_bytes, "keys_required": false, "source_retained": true,
            "logical_wud_bytes": logical,
            "wux_creation": record.wux_creation.map(|s| serde_json::json!({
                "block_size_bytes": WIIU_WUX_CANONICAL_BLOCK_BYTES,
                "stored_blocks": s.stored_blocks, "zero_blocks": s.zero_blocks,
                "reused_zero_blocks": s.reused_zero_blocks,
            })),
        }),
    );
    revalidate(plan, false)?;
    check_cancel(cancel)?;
    fs::create_dir_all(&options.journal_dir)?;
    // From this point the journal owns the stage path, including an ambiguous
    // publication failure. Retain it for existing rollback/recovery rules.
    let recovery_directory = stage.keep();
    let publication_result = apply_repair_transaction(&mut RepairApplyExecution {
        transaction: &mut transaction,
        current_generation: 0,
        options,
        cancel,
    });
    let applied = match publication_result {
        Ok(applied)
            if applied.summary.applied == 1
                && applied.summary.failed == 0
                && applied.transaction.state
                    == crate::dat::rename_apply::model::TransactionState::Applied
                && applied.reverify.len() == 1
                && applied.reverify[0].outcome == RepairReverifyOutcome::Verified =>
        {
            applied
        }
        outcome => {
            let detail = match outcome {
                Err(e) => e.to_string(),
                Ok(_) => "publication was not confirmed".into(),
            };
            // Failure cleanup must not be cancelled by the forward operation's
            // flag. The reused rollback still requires identity/no-clobber proof.
            let rollback = crate::repair::execute::rollback_repair_transaction(
                &mut transaction,
                &options.journal_dir,
                &std::sync::atomic::AtomicBool::new(false),
            );
            return Err(WiiUConversionError::Transaction {
                detail: format!("{detail}; rollback: {rollback:?}"),
                recovery_directory,
            });
        }
    };
    Ok((record, applied))
}
