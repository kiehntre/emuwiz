//! Bounded preview and verified native WUX → WUD conversion.
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
    binding: Option<(
        PathBuf,
        PathBuf,
        crate::wiiu_disc::WiiUDiscEvidence,
        WiiUConversionIdentity,
    )>,
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
    let native = request.direction == WiiUConversionDirection::WuxToWud;
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
    if !native {
        refusals.push(WiiUConversionRefusal::DeferredDirection);
    }
    let source = report
        .structure
        .as_ref()
        .map_or(0, |s| s.physical_container_size_bytes);
    let exact = native
        .then(|| {
            report
                .structure
                .as_ref()
                .and_then(|s| s.logical_disc_size_bytes)
        })
        .flatten();
    let available = request.available_free_space.or_else(|| {
        request
            .destination
            .parent()
            .and_then(crate::diagnostics::environment::filesystem_stat)
            .map(|s| s.available_bytes)
    });
    if let (Some(required), Some(available)) = (exact, available)
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
        destination_description: exact
            .map(|n| format!("exact logical WUD size: {n} bytes"))
            .unwrap_or_else(|| "WUD → WUX generation is deferred".into()),
        // Same-filesystem staging becomes the destination by atomic rename.
        temporary_bytes: exact,
        atomic_duplicate_bytes: Some(0),
        available_bytes: available,
        sufficient: exact.zip(available).map(|(n, a)| a >= n),
        safety_margin_bytes: 0,
    };
    let readiness = if !native || report.format == WiiUDiscFormat::Wua {
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
    let estimated_blocks = report.structure.as_ref().and_then(|s| s.block_count);
    let warnings = report.issues.iter().map(|i| format!("{i:?}")).collect();
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
            exact_identity_provable: native,
            steps: vec![
                "revalidate bounded source binding before writes".into(),
                "stream sectors in logical WUD order and hash reconstructed bytes".into(),
                "inspect staged WUD and compare complete header evidence".into(),
                "verify exact output size and independent full-file SHA-256".into(),
                "revalidate full source hash; publish with journaled no-clobber move".into(),
            ],
            source_identity: request.source_identity.clone(),
        },
        source_immutable: true,
        keys_required: false,
        provenance: "native-wux-to-wud-v1".into(),
        estimated_blocks,
        no_clobber: true,
        post_write_verification_available: native,
        binding,
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
    if source != &plan.source
        || destination != &plan.destination
        || identity != &plan.source_identity
        || plan.direction != WiiUConversionDirection::WuxToWud
        || plan.source_format != WiiUDiscFormat::Wux
        || plan.target_format != WiiUDiscFormat::Wud
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
        || report
            .structure
            .as_ref()
            .and_then(|s| s.logical_disc_size_bytes)
            != plan.space.destination_exact_bytes
    {
        return Err(WiiUConversionError::StalePlan);
    }
    if !fresh.refusals.is_empty() {
        return Err(WiiUConversionError::Refused(fresh.refusals));
    }
    Ok(report)
}

pub const WIIU_CONVERSION_CHUNK_BYTES: usize = 64 * 1024;

/// Native streaming decode, explicitly invoked after preview approval.
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
    let layout = crate::wiiu_disc::load_wux(&mut source, evidence.size_bytes)
        .map_err(WiiUConversionError::InvalidSource)?;
    if !evidence.matches_metadata(&source.metadata()?) {
        return Err(WiiUConversionError::StalePlan);
    }
    let logical = layout.logical;
    let parent = plan
        .destination
        .parent()
        .ok_or(WiiUConversionError::StalePlan)?;
    let stage = tempfile::Builder::new()
        .prefix(".emuwiz-wiiu-")
        .tempdir_in(staging_parent.unwrap_or(parent))?;
    let staged = stage.path().join("output.wud");
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
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
        written = written
            .checked_add(length as u64)
            .ok_or(WiiUConversionError::InvalidSource(
                WiiUDiscIssue::LogicalSizeOverflow,
            ))?;
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
    check_cancel(cancel)?;
    // Release the bounded table before later source inspection allocates one.
    drop(layout);
    output.sync_all()?;
    if output.metadata()?.len() != logical {
        return Err(WiiUConversionError::VerificationFailed(
            "output size differs".into(),
        ));
    }
    drop(output);
    let reconstructed_sha = digest_hex(digest.finalize());
    let inspected_output = crate::wiiu_disc::inspect_wii_u_disc(&staged);
    let source_header = report
        .structure
        .as_ref()
        .and_then(|s| s.wud_header.as_ref())
        .unwrap();
    if !inspected_output.structural_complete
        || inspected_output.format != WiiUDiscFormat::Wud
        || inspected_output
            .structure
            .as_ref()
            .and_then(|s| s.wud_header.as_ref())
            != Some(source_header)
    {
        return Err(WiiUConversionError::VerificationFailed(
            "WUD structural evidence differs from WUX projection".into(),
        ));
    }
    let output_identity = capture_identity(&staged)?;
    let output_sha = digest_hex(output_identity.freshness.as_ref().unwrap().sha256);
    if output_identity.size_bytes != logical || output_sha != reconstructed_sha {
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
    let proposal = RepairProposal {
        id: RepairProposalId::new("wiiu-output").unwrap(),
        action: RepairAction::MovePath {
            destination: plan.destination.clone(),
        },
        source_path: staged.clone(),
        reason: "publish independently verified native WUX → WUD output".into(),
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
        verdict_label: Some("Verified WUD output".into()),
        match_confident: true,
        is_outer_archive: false,
        is_outer_archive_verified: false,
        survivor_path: None,
    };
    let publication = build_repair_plan(
        RepairPlanId::new("wiiu-output-v1").unwrap(),
        0,
        crate::dat::sources::now_unix(),
        Some("native-wux-to-wud-v1".into()),
        vec![proposal],
    );
    let mut transaction = build_repair_transaction(&publication)
        .map_err(|e| WiiUConversionError::VerificationFailed(e.to_string()))?;
    let record = WiiUConversionRecord {
        source: plan.source.clone(),
        destination: plan.destination.clone(),
        output_bytes: logical,
        source_container_sha256: source_sha,
        reconstructed_wud_sha256: reconstructed_sha,
        output_sha256: output_sha,
        header_sha256: digest_hex(source_header.header_sha256),
        source_header: source_header.clone(),
        policy: "native-wux-to-wud-v1".into(),
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
