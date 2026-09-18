//! Bounded, read-only inspection of standalone ROM patch files.
//!
//! This module deliberately stops at an immutable derived-output plan.  It
//! never invokes a patcher, writes the base game, or creates the derivative.
//! Format parsers validate framing and declared sizes without allocating from
//! untrusted output-size fields.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const MAX_PATCH_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_METADATA_BYTES: usize = 1024 * 1024;
pub const MAX_RECORDS: usize = 1_000_000;
pub const MAX_DECLARED_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Applying is deliberately more conservative than inspection: the first
/// implementation keeps the selected base and staged result bounded in RAM.
pub const MAX_APPLY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StandalonePatchFormat {
    Ips,
    Bps,
    Ups,
    XdeltaVcdiff,
    Ppf,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchInspectionState {
    Valid,
    Invalid,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchCompatibility {
    Compatible,
    Incompatible,
    ReviewRequired,
    Unknown,
}

/// A representation change that is safe only when the caller has established
/// the platform rule for it.  This is deliberately explicit in the plan and
/// provenance; headers are never silently removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderAdjustment {
    None,
    Strip512ByteCopierHeader,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandalonePatchInspection {
    pub path: PathBuf,
    pub format: StandalonePatchFormat,
    pub state: PatchInspectionState,
    pub patch_size: u64,
    pub patch_sha256: String,
    pub source_size: Option<u64>,
    pub target_size: Option<u64>,
    pub source_crc32: Option<u32>,
    pub target_crc32: Option<u32>,
    pub patch_crc32: Option<u32>,
    pub metadata: Option<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandalonePatchMatch {
    pub compatibility: PatchCompatibility,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DerivedPatchPlan {
    pub base_path: PathBuf,
    pub patch_path: PathBuf,
    pub output_path: PathBuf,
    pub patch_format: StandalonePatchFormat,
    pub base_sha256: Option<String>,
    pub patch_sha256: String,
    pub expected_source_size: Option<u64>,
    pub expected_output_size: Option<u64>,
    pub expected_output_crc32: Option<u32>,
    pub header_adjustment: HeaderAdjustment,
    pub overwrite_existing: bool,
    pub confirmation_required: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandalonePatchApplyPlan {
    pub reviewed: DerivedPatchPlan,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandalonePatchApplyResult {
    pub output_path: PathBuf,
    pub output_size: u64,
    pub output_sha256: String,
    pub provenance: DerivedPatchProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DerivedPatchProvenance {
    pub base_path: PathBuf,
    pub base_sha256: String,
    pub patch_path: PathBuf,
    pub patch_sha256: String,
    pub format: StandalonePatchFormat,
    pub output_path: PathBuf,
    pub output_sha256: String,
    pub expected_source_crc32: Option<u32>,
    pub expected_output_crc32: Option<u32>,
    pub header_adjustment: HeaderAdjustment,
    pub applied_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StandalonePatchError {
    Io(String),
    TooLarge,
    Malformed(String),
    Unsupported(String),
    UnsafeOutput(String),
    SourceEqualsOutput,
}

impl std::fmt::Display for StandalonePatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "patch I/O error: {s}"),
            Self::TooLarge => write!(f, "patch exceeds the bounded inspection limit"),
            Self::Malformed(s) => write!(f, "malformed patch: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported patch: {s}"),
            Self::UnsafeOutput(s) => write!(f, "unsafe derived output: {s}"),
            Self::SourceEqualsOutput => write!(f, "derived output must differ from the base"),
        }
    }
}
impl std::error::Error for StandalonePatchError {}

pub fn inspect_standalone_patch(
    path: impl AsRef<Path>,
) -> Result<StandalonePatchInspection, StandalonePatchError> {
    let path = path.as_ref().to_path_buf();
    let bytes = fs::read(&path).map_err(|e| StandalonePatchError::Io(e.to_string()))?;
    if bytes.len() > MAX_PATCH_BYTES {
        return Err(StandalonePatchError::TooLarge);
    }
    let sha = hex_digest(&bytes);
    let (format, state, fields) = parse(&bytes);
    Ok(StandalonePatchInspection {
        path,
        format,
        state,
        patch_size: bytes.len() as u64,
        patch_sha256: sha,
        source_size: fields.source_size,
        target_size: fields.target_size,
        source_crc32: fields.source_crc32,
        target_crc32: fields.target_crc32,
        patch_crc32: fields.patch_crc32,
        metadata: fields.metadata,
        warnings: fields.warnings,
        error: fields.error,
    })
}

/// Match only evidence that can identify the exact source.  `title_or_region`
/// is intentionally not enough for a confident update/apply decision.
pub fn match_patch_source(
    inspection: &StandalonePatchInspection,
    base_bytes: Option<&[u8]>,
    title_or_region: bool,
) -> StandalonePatchMatch {
    match_patch_source_with_header(inspection, base_bytes, title_or_region, false).0
}

/// Match a patch while optionally considering the one supported copier-header
/// rule.  The caller must prove that the platform permits this rule; this
/// function never infers that from a filename or title.
pub fn match_patch_source_with_header(
    inspection: &StandalonePatchInspection,
    base_bytes: Option<&[u8]>,
    title_or_region: bool,
    allow_512_byte_copier_header: bool,
) -> (StandalonePatchMatch, HeaderAdjustment) {
    if !matches!(inspection.state, PatchInspectionState::Valid) {
        return (
            StandalonePatchMatch {
                compatibility: PatchCompatibility::Unknown,
                reason: "patch is not structurally valid".into(),
            },
            HeaderAdjustment::None,
        );
    }
    if let Some(bytes) = base_bytes {
        if inspection
            .source_size
            .is_some_and(|size| size != bytes.len() as u64)
        {
            if allow_512_byte_copier_header
                && bytes.len() >= 512
                && inspection.source_size == Some((bytes.len() - 512) as u64)
            {
                let normalized = &bytes[512..];
                if inspection
                    .source_crc32
                    .is_some_and(|expected| crc32(normalized) == expected)
                {
                    return (StandalonePatchMatch {
                        compatibility: PatchCompatibility::Compatible,
                        reason: "patch expects a headerless source; the selected ROM has a 512-byte copier header".into(),
                    }, HeaderAdjustment::Strip512ByteCopierHeader);
                }
            }
            return (
                StandalonePatchMatch {
                    compatibility: PatchCompatibility::Incompatible,
                    reason: "base size differs from patch source size".into(),
                },
                HeaderAdjustment::None,
            );
        }
        if let Some(expected) = inspection.source_crc32 {
            if crc32(bytes) != expected {
                return (
                    StandalonePatchMatch {
                        compatibility: PatchCompatibility::Incompatible,
                        reason: "base CRC32 differs from patch source checksum".into(),
                    },
                    HeaderAdjustment::None,
                );
            }
            return (
                StandalonePatchMatch {
                    compatibility: PatchCompatibility::Compatible,
                    reason: "base size and embedded source checksum match".into(),
                },
                HeaderAdjustment::None,
            );
        }
    }
    if inspection.source_size.is_none() && inspection.source_crc32.is_none() {
        return (
            StandalonePatchMatch {
                compatibility: if title_or_region {
                    PatchCompatibility::ReviewRequired
                } else {
                    PatchCompatibility::Unknown
                },
                reason: "format does not contain source identity".into(),
            },
            HeaderAdjustment::None,
        );
    }
    (
        StandalonePatchMatch {
            compatibility: PatchCompatibility::ReviewRequired,
            reason: "source checksum must be compared with the selected base bytes".into(),
        },
        HeaderAdjustment::None,
    )
}

pub fn build_derived_patch_plan(
    inspection: &StandalonePatchInspection,
    base_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    approved_output_root: impl AsRef<Path>,
    base_sha256: Option<String>,
) -> Result<DerivedPatchPlan, StandalonePatchError> {
    if !matches!(inspection.state, PatchInspectionState::Valid) {
        return Err(StandalonePatchError::Malformed(
            "cannot plan an invalid patch".into(),
        ));
    }
    let base = base_path.as_ref().to_path_buf();
    let output = output_path.as_ref().to_path_buf();
    if base == output {
        return Err(StandalonePatchError::SourceEqualsOutput);
    }
    let root = approved_output_root.as_ref();
    if !is_safe_child(root, &output) {
        return Err(StandalonePatchError::UnsafeOutput(
            "output is outside the approved derivative root or uses a symlink component".into(),
        ));
    }
    if output.exists() {
        return Err(StandalonePatchError::UnsafeOutput(
            "destination already exists; overwrite is disabled".into(),
        ));
    }
    Ok(DerivedPatchPlan {
        base_path: base,
        patch_path: inspection.path.clone(),
        output_path: output,
        patch_format: inspection.format,
        base_sha256,
        patch_sha256: inspection.patch_sha256.clone(),
        expected_source_size: inspection.source_size,
        expected_output_size: inspection.target_size,
        expected_output_crc32: inspection.target_crc32,
        header_adjustment: HeaderAdjustment::None,
        overwrite_existing: false,
        confirmation_required: true,
        warnings: inspection.warnings.clone(),
    })
}

pub fn build_standalone_patch_apply_plan(
    inspection: &StandalonePatchInspection,
    base_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    approved_output_root: impl AsRef<Path>,
) -> Result<StandalonePatchApplyPlan, StandalonePatchError> {
    build_standalone_patch_apply_plan_with_header(
        inspection,
        base_path,
        output_path,
        approved_output_root,
        HeaderAdjustment::None,
    )
}

pub fn build_standalone_patch_apply_plan_with_header(
    inspection: &StandalonePatchInspection,
    base_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    approved_output_root: impl AsRef<Path>,
    header_adjustment: HeaderAdjustment,
) -> Result<StandalonePatchApplyPlan, StandalonePatchError> {
    if !matches!(
        inspection.format,
        StandalonePatchFormat::Ips | StandalonePatchFormat::Bps | StandalonePatchFormat::Ups
    ) {
        return Err(StandalonePatchError::Unsupported(
            "only IPS, BPS, and UPS application is enabled".into(),
        ));
    }
    let base = fs::read(base_path.as_ref()).map_err(|e| StandalonePatchError::Io(e.to_string()))?;
    if base.len() as u64 > MAX_APPLY_BYTES {
        return Err(StandalonePatchError::TooLarge);
    }
    let base_sha256 = hex_digest(&base);
    let patch_input = match header_adjustment {
        HeaderAdjustment::None => base.as_slice(),
        HeaderAdjustment::Strip512ByteCopierHeader => base.get(512..).ok_or_else(|| {
            StandalonePatchError::UnsafeOutput(
                "selected ROM has no complete 512-byte header".into(),
            )
        })?,
    };
    if inspection.source_size != Some(patch_input.len() as u64)
        || inspection
            .source_crc32
            .is_some_and(|expected| crc32(patch_input) != expected)
    {
        return Err(StandalonePatchError::Malformed(
            "selected base does not match the reviewed patch source".into(),
        ));
    }
    let mut plan = build_derived_patch_plan(
        inspection,
        base_path,
        output_path,
        approved_output_root,
        Some(base_sha256),
    )?;
    plan.header_adjustment = header_adjustment;
    Ok(StandalonePatchApplyPlan { reviewed: plan })
}

pub fn apply_standalone_patch(
    plan: &StandalonePatchApplyPlan,
) -> Result<StandalonePatchApplyResult, StandalonePatchError> {
    let base =
        fs::read(&plan.reviewed.base_path).map_err(|e| StandalonePatchError::Io(e.to_string()))?;
    if base.len() as u64 > MAX_APPLY_BYTES {
        return Err(StandalonePatchError::TooLarge);
    }
    let base_hash = hex_digest(&base);
    if plan.reviewed.base_sha256.as_deref() != Some(base_hash.as_str()) {
        return Err(StandalonePatchError::Malformed(
            "base changed since review".into(),
        ));
    }
    let inspection = inspect_standalone_patch(&plan.reviewed.patch_path)?;
    if inspection.patch_sha256 != plan.reviewed.patch_sha256 {
        return Err(StandalonePatchError::Malformed(
            "patch changed since review".into(),
        ));
    }
    let patch =
        fs::read(&plan.reviewed.patch_path).map_err(|e| StandalonePatchError::Io(e.to_string()))?;
    let patch_input = match plan.reviewed.header_adjustment {
        HeaderAdjustment::None => base.as_slice(),
        HeaderAdjustment::Strip512ByteCopierHeader => base.get(512..).ok_or_else(|| {
            StandalonePatchError::Malformed("selected base has no complete 512-byte header".into())
        })?,
    };
    if inspection
        .source_crc32
        .is_some_and(|expected| crc32(patch_input) != expected)
    {
        return Err(StandalonePatchError::Malformed(
            "base CRC differs from reviewed patch source checksum".into(),
        ));
    }
    let output = match inspection.format {
        StandalonePatchFormat::Ips => apply_ips(patch_input, &patch)?,
        StandalonePatchFormat::Bps => apply_bps(patch_input, &patch)?,
        StandalonePatchFormat::Ups => apply_ups(patch_input, &patch)?,
        _ => {
            return Err(StandalonePatchError::Unsupported(
                "format is inspection-only".into(),
            ));
        }
    };
    if output.len() as u64 > MAX_APPLY_BYTES {
        return Err(StandalonePatchError::TooLarge);
    }
    if inspection
        .target_size
        .is_some_and(|s| s != output.len() as u64)
    {
        return Err(StandalonePatchError::Malformed(
            "output size differs from patch declaration".into(),
        ));
    }
    if let Some(crc) = inspection.target_crc32
        && crc32(&output) != crc
    {
        return Err(StandalonePatchError::Malformed(
            "output CRC mismatch".into(),
        ));
    }
    let output_hash = hex_digest(&output);
    let parent = plan
        .reviewed
        .output_path
        .parent()
        .ok_or_else(|| StandalonePatchError::UnsafeOutput("output has no parent".into()))?;
    if !parent.is_dir()
        || plan.reviewed.output_path.exists()
        || !is_safe_child(parent, &plan.reviewed.output_path)
    {
        return Err(StandalonePatchError::UnsafeOutput(
            "destination is unavailable or unsafe".into(),
        ));
    }
    let provenance = DerivedPatchProvenance {
        base_path: plan.reviewed.base_path.clone(),
        base_sha256: base_hash,
        patch_path: plan.reviewed.patch_path.clone(),
        patch_sha256: plan.reviewed.patch_sha256.clone(),
        format: inspection.format,
        output_path: plan.reviewed.output_path.clone(),
        output_sha256: output_hash.clone(),
        expected_source_crc32: inspection.source_crc32,
        expected_output_crc32: inspection.target_crc32,
        header_adjustment: plan.reviewed.header_adjustment,
        applied_at_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
    };
    let provenance_path = plan.reviewed.output_path.with_file_name(format!(
        "{}.emuwiz-patch.json",
        plan.reviewed
            .output_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("patched-rom")
    ));
    if provenance_path.exists() {
        return Err(StandalonePatchError::UnsafeOutput(
            "provenance destination already exists; overwrite is disabled".into(),
        ));
    }
    let provenance_json = serde_json::to_vec_pretty(&provenance)
        .map_err(|error| StandalonePatchError::Io(error.to_string()))?;
    let stage = parent.join(format!(
        ".emuwiz-derived-{}-{}",
        std::process::id(),
        patch.len()
    ));
    let provenance_stage = parent.join(format!(
        ".emuwiz-derived-provenance-{}-{}",
        std::process::id(),
        patch.len()
    ));
    let result = (|| {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)
            .map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        file.write_all(&output)
            .map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        file.sync_all()
            .map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        let mut provenance_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&provenance_stage)
            .map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        provenance_file
            .write_all(&provenance_json)
            .map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        provenance_file
            .sync_all()
            .map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        fs::hard_link(&stage, &plan.reviewed.output_path)
            .map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        fs::hard_link(&provenance_stage, &provenance_path)
            .map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        fs::remove_file(&stage).map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        fs::remove_file(&provenance_stage).map_err(|e| StandalonePatchError::Io(e.to_string()))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&stage);
        let _ = fs::remove_file(&provenance_stage);
        let _ = fs::remove_file(&plan.reviewed.output_path);
        let _ = fs::remove_file(&provenance_path);
        return Err(error);
    }
    Ok(StandalonePatchApplyResult {
        output_path: plan.reviewed.output_path.clone(),
        output_size: output.len() as u64,
        output_sha256: output_hash.clone(),
        provenance,
    })
}

fn apply_ips(base: &[u8], patch: &[u8]) -> Result<Vec<u8>, StandalonePatchError> {
    let mut out = base.to_vec();
    let mut p = 5;
    while p < patch.len() {
        if patch[p..].starts_with(b"EOF") {
            p += 3;
            if patch.len() - p == 3 {
                let n = ((patch[p] as usize) << 16)
                    | ((patch[p + 1] as usize) << 8)
                    | patch[p + 2] as usize;
                if n as u64 > MAX_APPLY_BYTES {
                    return Err(StandalonePatchError::TooLarge);
                };
                out.resize(n, 0)
            } else if p != patch.len() {
                return Err(StandalonePatchError::Malformed("IPS trailing bytes".into()));
            }
            return Ok(out);
        }
        if patch.len() - p < 5 {
            return Err(StandalonePatchError::Malformed(
                "IPS truncated record".into(),
            ));
        }
        let at =
            ((patch[p] as usize) << 16) | ((patch[p + 1] as usize) << 8) | patch[p + 2] as usize;
        let size = u16::from_be_bytes([patch[p + 3], patch[p + 4]]) as usize;
        p += 5;
        let _n = if size == 0 {
            if patch.len() - p < 3 {
                return Err(StandalonePatchError::Malformed("IPS truncated RLE".into()));
            }
            let n = u16::from_be_bytes([patch[p], patch[p + 1]]) as usize;
            p += 2;
            if n == 0 {
                return Err(StandalonePatchError::Malformed("IPS zero RLE".into()));
            }
            let value = patch[p];
            p += 1;
            if at.checked_add(n).is_none() {
                return Err(StandalonePatchError::Malformed(
                    "IPS offset overflow".into(),
                ));
            }
            let end = at + n;
            if end as u64 > MAX_APPLY_BYTES {
                return Err(StandalonePatchError::TooLarge);
            };
            out.resize(end, 0);
            for x in &mut out[at..end] {
                *x = value
            }
            continue;
        } else {
            if patch.len() - p < size {
                return Err(StandalonePatchError::Malformed("IPS truncated data".into()));
            }
            if at.checked_add(size).is_none() {
                return Err(StandalonePatchError::Malformed(
                    "IPS offset overflow".into(),
                ));
            }
            let end = at + size;
            if end as u64 > MAX_APPLY_BYTES {
                return Err(StandalonePatchError::TooLarge);
            };
            out.resize(end, 0);
            out[at..end].copy_from_slice(&patch[p..p + size]);
            p += size;
            continue;
        };
    }
    Err(StandalonePatchError::Malformed("IPS missing EOF".into()))
}

fn apply_bps(base: &[u8], patch: &[u8]) -> Result<Vec<u8>, StandalonePatchError> {
    if patch.len() < 16 {
        return Err(StandalonePatchError::Malformed("BPS truncated".into()));
    }
    let end = patch.len() - 12;
    let mut p = 4;
    let source = read_var(patch, &mut p).map_err(|e| StandalonePatchError::Malformed(e.into()))?;
    let target = read_var(patch, &mut p).map_err(|e| StandalonePatchError::Malformed(e.into()))?;
    let meta = read_var(patch, &mut p).map_err(|e| StandalonePatchError::Malformed(e.into()))?;
    let Some(remaining) = end.checked_sub(p) else {
        return Err(StandalonePatchError::Malformed("BPS truncated".into()));
    };
    if source != base.len() as u64 || target > MAX_APPLY_BYTES || meta > remaining as u64 {
        return Err(StandalonePatchError::Malformed("BPS size mismatch".into()));
    }
    p += meta as usize;
    let mut out = Vec::with_capacity(target as usize);
    let mut sr = 0i64;
    let mut tr = 0i64;
    while p < end {
        let a = read_var(patch, &mut p).map_err(|e| StandalonePatchError::Malformed(e.into()))?;
        let n = (a >> 2)
            .checked_add(1)
            .ok_or_else(|| StandalonePatchError::Malformed("BPS length overflow".into()))?
            as usize;
        match a & 3 {
            0 => {
                if out.len().checked_add(n).is_none_or(|e| e > base.len()) {
                    return Err(StandalonePatchError::Malformed(
                        "BPS source read out of bounds".into(),
                    ));
                }
                out.extend_from_slice(&base[out.len()..out.len() + n]);
            }
            1 => {
                if p.checked_add(n).is_none_or(|next| next > end) {
                    return Err(StandalonePatchError::Malformed(
                        "BPS target read truncated".into(),
                    ));
                }
                out.extend_from_slice(&patch[p..p + n]);
                p += n;
            }
            2 => {
                let v = read_var(patch, &mut p)
                    .map_err(|e| StandalonePatchError::Malformed(e.into()))?;
                let d = (v >> 1) as i64;
                sr = if v & 1 == 0 {
                    sr.checked_add(d)
                } else {
                    sr.checked_sub(d)
                }
                .ok_or_else(|| {
                    StandalonePatchError::Malformed("BPS source offset overflow".into())
                })?;
                let at = usize::try_from(sr).map_err(|_| {
                    StandalonePatchError::Malformed("BPS source offset negative".into())
                })?;
                if at.checked_add(n).is_none_or(|e| e > base.len()) {
                    return Err(StandalonePatchError::Malformed(
                        "BPS source copy out of bounds".into(),
                    ));
                }
                out.extend_from_slice(&base[at..at + n]);
                // The spec reads `source[sourceRelativeOffset++]` per byte,
                // so the pointer ends `n` further on and the next record's
                // delta is relative to there.
                sr = sr.checked_add(n as i64).ok_or_else(|| {
                    StandalonePatchError::Malformed("BPS source offset overflow".into())
                })?;
            }
            3 => {
                let v = read_var(patch, &mut p)
                    .map_err(|e| StandalonePatchError::Malformed(e.into()))?;
                let d = (v >> 1) as i64;
                tr = if v & 1 == 0 {
                    tr.checked_add(d)
                } else {
                    tr.checked_sub(d)
                }
                .ok_or_else(|| {
                    StandalonePatchError::Malformed("BPS target offset overflow".into())
                })?;
                let at = usize::try_from(tr).map_err(|_| {
                    StandalonePatchError::Malformed("BPS target offset negative".into())
                })?;
                if at >= out.len() {
                    return Err(StandalonePatchError::Malformed(
                        "BPS target copy out of bounds".into(),
                    ));
                }
                for i in 0..n {
                    let x = *out.get(at + i).ok_or_else(|| {
                        StandalonePatchError::Malformed("BPS target copy out of bounds".into())
                    })?;
                    out.push(x)
                }
                // As for SourceCopy: `target[targetRelativeOffset++]`.
                tr = tr.checked_add(n as i64).ok_or_else(|| {
                    StandalonePatchError::Malformed("BPS target offset overflow".into())
                })?;
            }
            _ => unreachable!(),
        }
        if out.len() as u64 > target {
            return Err(StandalonePatchError::Malformed(
                "BPS output exceeds target".into(),
            ));
        }
    }
    if out.len() as u64 != target {
        return Err(StandalonePatchError::Malformed(
            "BPS output size mismatch".into(),
        ));
    }
    if crc32(&patch[..end + 8]) != u32::from_le_bytes(patch[end + 8..].try_into().unwrap()) {
        return Err(StandalonePatchError::Malformed(
            "BPS patch CRC mismatch".into(),
        ));
    }
    Ok(out)
}

fn apply_ups(base: &[u8], patch: &[u8]) -> Result<Vec<u8>, StandalonePatchError> {
    if patch.len() < 16 {
        return Err(StandalonePatchError::Malformed("UPS truncated".into()));
    }
    let end = patch.len() - 12;
    let mut p = 4;
    let s = read_var(patch, &mut p).map_err(|e| StandalonePatchError::Malformed(e.into()))?;
    let t = read_var(patch, &mut p).map_err(|e| StandalonePatchError::Malformed(e.into()))?;
    if s != base.len() as u64 || t > MAX_APPLY_BYTES {
        return Err(StandalonePatchError::Malformed("UPS size mismatch".into()));
    }
    let mut out = vec![0; t as usize];
    let initial = base.len().min(out.len());
    out[..initial].copy_from_slice(&base[..initial]);
    let mut at = 0usize;
    while p < end {
        let d = read_var(patch, &mut p).map_err(|e| StandalonePatchError::Malformed(e.into()))?
            as usize;
        at = at
            .checked_add(d)
            .ok_or_else(|| StandalonePatchError::Malformed("UPS offset overflow".into()))?;
        loop {
            let x = *patch
                .get(p)
                .ok_or_else(|| StandalonePatchError::Malformed("UPS truncated XOR".into()))?;
            p += 1;
            if x == 0 {
                at = at
                    .checked_add(1)
                    .ok_or_else(|| StandalonePatchError::Malformed("UPS offset overflow".into()))?;
                break;
            }
            if at >= out.len() {
                return Err(StandalonePatchError::Malformed(
                    "UPS offset out of bounds".into(),
                ));
            }
            out[at] ^= x;
            at += 1;
        }
    }
    if crc32(&patch[..end + 8]) != u32::from_le_bytes(patch[end + 8..].try_into().unwrap()) {
        return Err(StandalonePatchError::Malformed(
            "UPS patch CRC mismatch".into(),
        ));
    }
    Ok(out)
}

#[derive(Default)]
struct Fields {
    source_size: Option<u64>,
    target_size: Option<u64>,
    source_crc32: Option<u32>,
    target_crc32: Option<u32>,
    patch_crc32: Option<u32>,
    metadata: Option<String>,
    warnings: Vec<String>,
    error: Option<String>,
}

fn parse(b: &[u8]) -> (StandalonePatchFormat, PatchInspectionState, Fields) {
    if b.starts_with(b"PATCH") {
        let (s, f) = parse_ips(b);
        return (StandalonePatchFormat::Ips, s, f);
    }
    if b.starts_with(b"BPS1") {
        let (s, f) = parse_bps(b);
        return (StandalonePatchFormat::Bps, s, f);
    }
    if b.starts_with(b"UPS1") {
        let (s, f) = parse_ups(b);
        return (StandalonePatchFormat::Ups, s, f);
    }
    // RFC 3284 section 4.1: 'V', 'C' and 'D' with their most significant
    // bits set, in that order - 0xd6, 0xc3, 0xc4. Real xdelta3 output begins
    // with exactly these bytes; an earlier transposition (0xd6, 0xc4, 0xc3)
    // meant every genuine patch was reported as an unknown format.
    if b.starts_with(&[0xd6, 0xc3, 0xc4]) {
        return (
            StandalonePatchFormat::XdeltaVcdiff,
            PatchInspectionState::Valid,
            Fields {
                warnings: vec![
                    "VCDIFF structure recognized; source identity is not embedded".into(),
                ],
                ..Default::default()
            },
        );
    }
    if b.len() >= 4 && &b[..3] == b"PPF" && (b[3] == b'1' || b[3] == b'2' || b[3] == b'3') {
        if b.len() < 6 {
            let (state, fields) = invalid(StandalonePatchFormat::Ppf, "truncated PPF header");
            return (StandalonePatchFormat::Ppf, state, fields);
        }
        return (
            StandalonePatchFormat::Ppf,
            PatchInspectionState::Valid,
            Fields {
                warnings: vec![
                    "PPF is disc-image oriented; topology and source identity require review"
                        .into(),
                ],
                ..Default::default()
            },
        );
    }
    (
        StandalonePatchFormat::Unknown,
        PatchInspectionState::Unsupported,
        Fields {
            error: Some("no supported patch signature".into()),
            ..Default::default()
        },
    )
}

fn invalid(_format: StandalonePatchFormat, msg: &str) -> (PatchInspectionState, Fields) {
    (
        PatchInspectionState::Invalid,
        Fields {
            error: Some(msg.into()),
            ..Default::default()
        },
    )
}

fn parse_ips(b: &[u8]) -> (PatchInspectionState, Fields) {
    let mut f = Fields::default();
    let mut p = 5;
    let mut records = 0;
    let mut max = 0u64;
    while p < b.len() {
        if b[p..].starts_with(b"EOF") {
            p += 3;
            if p < b.len() && b.len() - p != 3 {
                return invalid(StandalonePatchFormat::Ips, "trailing bytes after IPS EOF");
            }
            return (PatchInspectionState::Valid, {
                f.target_size = Some(max);
                f
            });
        }
        if b.len() - p < 5 {
            return invalid(StandalonePatchFormat::Ips, "truncated IPS record");
        }
        records += 1;
        if records > MAX_RECORDS {
            return invalid(StandalonePatchFormat::Ips, "too many records");
        }
        let offset = ((b[p] as u64) << 16) | ((b[p + 1] as u64) << 8) | b[p + 2] as u64;
        let size = u16::from_be_bytes([b[p + 3], b[p + 4]]) as u64;
        p += 5;
        let length = if size == 0 {
            if b.len() - p < 3 {
                return invalid(StandalonePatchFormat::Ips, "truncated IPS RLE record");
            }
            let n = u16::from_be_bytes([b[p], b[p + 1]]) as u64;
            p += 3;
            if n == 0 {
                return invalid(StandalonePatchFormat::Ips, "zero-length IPS RLE record");
            }
            n
        } else {
            size
        };
        if size != 0 {
            if b.len() - p < size as usize {
                return invalid(StandalonePatchFormat::Ips, "truncated IPS data");
            }
            p += size as usize;
        }
        max = max.max(offset.saturating_add(length));
        if max > MAX_DECLARED_OUTPUT_BYTES {
            return invalid(StandalonePatchFormat::Ips, "output bound is excessive");
        }
    }
    invalid(StandalonePatchFormat::Ips, "missing IPS EOF")
}

/// The BPS/UPS variable-width integer.
///
/// Both formats use the same encoding, and it is not plain LEB128: the
/// *last* byte is the one with bit 7 **set**, and every continuation adds the
/// running multiplier back in, which is what makes the encoding bijective
/// (`encode` decrements before emitting the next septet). Decoding it as
/// LEB128 - stopping on a clear bit 7 and omitting the bias - silently reads
/// the wrong value and the wrong number of bytes, which is how a well-formed
/// patch could walk the cursor past the footer.
///
/// Patch files are untrusted input, so every step is checked and a malformed
/// integer is an error rather than a wrap or a panic. Ten septets cover the
/// whole u64 range.
fn read_var(b: &[u8], p: &mut usize) -> Result<u64, &'static str> {
    let mut value = 0u64;
    let mut multiplier = 1u64;
    for _ in 0..10 {
        let x = *b.get(*p).ok_or("truncated variable integer")?;
        *p += 1;
        value = value
            .checked_add(
                u64::from(x & 0x7f)
                    .checked_mul(multiplier)
                    .ok_or("variable integer overflow")?,
            )
            .ok_or("variable integer overflow")?;
        if x & 0x80 != 0 {
            return Ok(value);
        }
        multiplier = multiplier
            .checked_mul(0x80)
            .ok_or("variable integer overflow")?;
        value = value
            .checked_add(multiplier)
            .ok_or("variable integer overflow")?;
    }
    Err("variable integer too long")
}

fn parse_bps(b: &[u8]) -> (PatchInspectionState, Fields) {
    let mut f = Fields::default();
    if b.len() < 16 {
        return invalid(StandalonePatchFormat::Bps, "truncated BPS");
    }
    let end = b.len() - 12;
    let mut p = 4;
    let source = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Bps, e),
    };
    let target = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Bps, e),
    };
    let meta = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Bps, e),
    };
    // The three header integers can themselves run into or past the 12-byte
    // footer on a malformed file, so establish how much room is actually
    // left before any of it is used as a length.
    let Some(remaining) = end.checked_sub(p) else {
        return invalid(StandalonePatchFormat::Bps, "truncated BPS header");
    };
    if target > MAX_DECLARED_OUTPUT_BYTES || meta > MAX_METADATA_BYTES as u64 {
        return invalid(StandalonePatchFormat::Bps, "invalid size or metadata");
    }
    if meta > remaining as u64 {
        return invalid(StandalonePatchFormat::Bps, "truncated metadata");
    }
    let meta_len = meta as usize;
    if meta_len > 0 {
        f.metadata = Some(String::from_utf8_lossy(&b[p..p + meta_len]).into_owned());
    }
    p += meta_len;
    let mut output = 0u64;
    let mut records = 0;
    while p < end {
        let a = match read_var(b, &mut p) {
            Ok(v) => v,
            Err(e) => return invalid(StandalonePatchFormat::Bps, e),
        };
        let len = (a >> 2).saturating_add(1);
        if len > target.saturating_sub(output) {
            return invalid(StandalonePatchFormat::Bps, "operations exceed target size");
        }
        match a & 3 {
            0 => {}
            1 => {
                let Ok(len) = usize::try_from(len) else {
                    return invalid(StandalonePatchFormat::Bps, "truncated target data");
                };
                if p.checked_add(len).is_none_or(|next| next > end) {
                    return invalid(StandalonePatchFormat::Bps, "truncated target data");
                }
                p += len;
            }
            2 | 3 => {
                if read_var(b, &mut p).is_err() {
                    return invalid(StandalonePatchFormat::Bps, "truncated copy offset");
                }
            }
            _ => unreachable!(),
        }
        let Some(next_output) = output.checked_add(len) else {
            return invalid(StandalonePatchFormat::Bps, "operations exceed target size");
        };
        output = next_output;
        records += 1;
        if records > MAX_RECORDS {
            return invalid(StandalonePatchFormat::Bps, "too many records");
        }
    }
    f.source_size = Some(source);
    f.target_size = Some(target);
    f.source_crc32 = Some(u32::from_le_bytes(b[end..end + 4].try_into().unwrap()));
    f.target_crc32 = Some(u32::from_le_bytes(b[end + 4..end + 8].try_into().unwrap()));
    f.patch_crc32 = Some(u32::from_le_bytes(b[end + 8..].try_into().unwrap()));
    if crc32(&b[..end + 8]) != f.patch_crc32.unwrap() {
        return invalid(StandalonePatchFormat::Bps, "patch CRC mismatch");
    }
    if output != target {
        return invalid(
            StandalonePatchFormat::Bps,
            "operations do not produce target size",
        );
    }
    (PatchInspectionState::Valid, f)
}

fn parse_ups(b: &[u8]) -> (PatchInspectionState, Fields) {
    let mut f = Fields::default();
    if b.len() < 16 {
        return invalid(StandalonePatchFormat::Ups, "truncated UPS");
    }
    let end = b.len() - 12;
    let mut p = 4;
    let s = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Ups, e),
    };
    let t = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Ups, e),
    };
    let mut records = 0;
    while p < end {
        if read_var(b, &mut p).is_err() {
            return invalid(StandalonePatchFormat::Ups, "truncated offset");
        }
        let mut terminated = false;
        while p < end {
            let x = b[p];
            p += 1;
            if x == 0 {
                terminated = true;
                break;
            }
        }
        if !terminated {
            return invalid(StandalonePatchFormat::Ups, "truncated XOR record");
        }
        records += 1;
        if records > MAX_RECORDS {
            return invalid(StandalonePatchFormat::Ups, "too many records");
        }
    }
    f.source_size = Some(s);
    f.target_size = Some(t);
    f.source_crc32 = Some(u32::from_le_bytes(b[end..end + 4].try_into().unwrap()));
    f.target_crc32 = Some(u32::from_le_bytes(b[end + 4..end + 8].try_into().unwrap()));
    f.patch_crc32 = Some(u32::from_le_bytes(b[end + 8..].try_into().unwrap()));
    if crc32(&b[..end + 8]) != f.patch_crc32.unwrap() {
        return invalid(StandalonePatchFormat::Ups, "patch CRC mismatch");
    }
    (PatchInspectionState::Valid, f)
}

fn is_safe_child(root: &Path, child: &Path) -> bool {
    if !child.is_absolute() || !root.is_absolute() {
        return false;
    }
    let Ok(rel) = child.strip_prefix(root) else {
        return false;
    };
    if rel.as_os_str().is_empty() {
        return false;
    }
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        if !matches!(c, Component::Normal(_)) {
            return false;
        }
        cur.push(c);
        if cur
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return false;
        }
    }
    true
}
fn hex_digest(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}
fn crc32(b: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for &x in b {
        crc ^= x as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    /// The reference BPS/UPS encoder from the format specification: septets
    /// little-endian, bit 7 set on the *last* byte, and one subtracted before
    /// each continuation. `read_var` is its exact inverse.
    fn var(mut n: u64) -> Vec<u8> {
        let mut o = Vec::new();
        loop {
            let x = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                o.push(x | 0x80);
                break;
            } else {
                o.push(x);
                n -= 1;
            }
        }
        o
    }
    fn bps(source: u64, target: u64, body: &[u8]) -> Vec<u8> {
        let mut b = b"BPS1".to_vec();
        b.extend(var(source));
        b.extend(var(target));
        b.push(0x80);
        b.extend(body);
        b.extend([0, 0, 0, 0]);
        b.extend([0, 0, 0, 0]);
        let c = crc32(&b);
        b.extend(c.to_le_bytes());
        b
    }
    fn temp_file(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(name);
        fs::File::create(&p).unwrap().write_all(bytes).unwrap();
        (d, p)
    }
    #[test]
    fn variable_width_integers_round_trip_the_reference_encoding() {
        // The regression this pins: `read_var` used to decode these as
        // LEB128 - terminating on a *clear* bit 7 and without the
        // continuation bias - so it returned wrong values and consumed the
        // wrong number of bytes for everything above 0.
        for value in [
            0u64,
            1,
            2,
            126,
            127,
            128,
            129,
            255,
            256,
            16_383,
            16_384,
            1 << 20,
            1 << 32,
            u32::MAX as u64,
        ] {
            let encoded = var(value);
            let mut cursor = 0usize;
            assert_eq!(
                read_var(&encoded, &mut cursor),
                Ok(value),
                "decoding {value} from {encoded:02x?}"
            );
            assert_eq!(cursor, encoded.len(), "cursor after decoding {value}");
        }
    }

    #[test]
    fn a_target_read_action_consumes_its_payload_byte() {
        // The other minimal encoding: action word 1 is a TargetRead of one
        // byte, which takes its output from the patch stream itself.
        let mut body = var(1);
        body.push(0x5a);
        let (_d, p) = temp_file("x.bps", &bps(1, 1, &body));
        let i = inspect_standalone_patch(p).unwrap();
        assert_eq!(i.state, PatchInspectionState::Valid);
        assert_eq!(i.target_size, Some(1));
    }

    #[test]
    fn malformed_bps_never_panics_and_is_reported_invalid() {
        // Each of these used to reach unchecked arithmetic once the header
        // integers pushed the cursor past the 12-byte footer.
        let cases: Vec<Vec<u8>> = vec![
            // A TargetRead whose payload byte is missing.
            bps(1, 1, &var(1)),
            // Header integers that run into the footer: three continuation
            // bytes with no terminator before the trailer.
            {
                let mut b = b"BPS1".to_vec();
                b.extend([0x00, 0x00, 0x00, 0x00]);
                b.extend([0u8; 12]);
                b
            },
            // Metadata longer than the bytes that remain.
            {
                let mut b = b"BPS1".to_vec();
                b.extend(var(1));
                b.extend(var(1));
                b.extend(var(64));
                b.extend([0u8; 12]);
                b
            },
            // Exactly the 16-byte minimum, all zeroes.
            vec![0x42, 0x50, 0x53, 0x31, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ];
        for (index, bytes) in cases.into_iter().enumerate() {
            let (_d, path) = temp_file("x.bps", &bytes);
            let inspected = inspect_standalone_patch(path).unwrap();
            assert_eq!(
                inspected.state,
                PatchInspectionState::Invalid,
                "case {index} should be rejected, not accepted"
            );
        }
    }

    /// Builds a BPS action word: mode in the low two bits, `length - 1`
    /// above them, exactly as the format specifies.
    fn action(mode: u64, length: u64) -> Vec<u8> {
        var(((length - 1) << 2) | mode)
    }

    #[test]
    fn source_copy_advances_its_relative_offset_between_records() {
        // Two consecutive SourceCopy records, each with a relative offset
        // delta of zero. The spec advances the source pointer by the copied
        // length, so the second record must continue where the first
        // stopped; a pointer that never advances would copy the first two
        // bytes twice.
        let base = [1u8, 2, 3, 4];
        let mut body = action(2, 2);
        body.extend(var(0));
        body.extend(action(2, 2));
        body.extend(var(0));
        let patch = bps(base.len() as u64, base.len() as u64, &body);
        assert_eq!(apply_bps(&base, &patch).unwrap(), base);
    }

    #[test]
    fn target_copy_advances_its_relative_offset_between_records() {
        // Same rule for TargetCopy, reading from the output built so far.
        // Two records with a zero delta each: the second must continue from
        // where the first stopped, so the tail repeats the whole base rather
        // than its first two bytes twice.
        let base = [1u8, 2, 3, 4];
        let mut body = action(0, 4);
        body.extend(action(3, 2));
        body.extend(var(0));
        body.extend(action(3, 2));
        body.extend(var(0));
        let patch = bps(base.len() as u64, 8, &body);
        assert_eq!(apply_bps(&base, &patch).unwrap(), [1, 2, 3, 4, 1, 2, 3, 4]);
    }

    #[test]
    fn valid_bps_and_hash() {
        // Action word 0: SourceRead, length (0 >> 2) + 1 = 1, no payload -
        // exactly the one output byte the declared target size calls for.
        let body = var(0);
        let (_d, p) = temp_file("x.bps", &bps(1, 1, &body));
        let i = inspect_standalone_patch(p).unwrap();
        assert_eq!(i.format, StandalonePatchFormat::Bps);
        assert_eq!(i.state, PatchInspectionState::Valid);
        assert_eq!(i.source_size, Some(1));
    }
    #[test]
    fn malformed_and_extension_spoofing() {
        let (_d, p) = temp_file("x.bps", b"not-bps");
        let i = inspect_standalone_patch(p).unwrap();
        assert_eq!(i.format, StandalonePatchFormat::Unknown);
        let (_d, p) = temp_file("x.bin", b"BPS1");
        assert_eq!(
            inspect_standalone_patch(p).unwrap().state,
            PatchInspectionState::Invalid
        );
    }
    #[test]
    fn ips_and_ups_and_unknown() {
        let (_d, p) = temp_file("x.ips", b"PATCH\0\0\0\0\x01xEOF");
        assert_eq!(
            inspect_standalone_patch(p).unwrap().state,
            PatchInspectionState::Valid
        );
        let (_d, p) = temp_file("x.ups", b"UPS1\x80\x80\0\0\0\0\0\0\0\0\0\0");
        assert_eq!(
            inspect_standalone_patch(p).unwrap().format,
            StandalonePatchFormat::Ups
        );
    }
    #[test]
    fn plan_is_safe_and_deterministic() {
        let body = var(0);
        let (_d, p) = temp_file("x.bps", &bps(1, 1, &body));
        let i = inspect_standalone_patch(&p).unwrap();
        let root = p.parent().unwrap();
        let base = root.join("base.rom");
        fs::write(&base, [7u8]).unwrap();
        let a = build_derived_patch_plan(&i, &base, root.join("out.rom"), root, None).unwrap();
        let b = build_derived_patch_plan(&i, &base, root.join("out.rom"), root, None).unwrap();
        assert_eq!(a, b);
        assert_eq!(fs::read(&base).unwrap(), [7]);
        assert!(matches!(
            build_derived_patch_plan(&i, &base, &base, root, None),
            Err(StandalonePatchError::SourceEqualsOutput)
        ));
    }
    #[test]
    fn unsafe_output_and_vcdiff_ppf() {
        let (_d, p) = temp_file("x", &[0xd6, 0xc3, 0xc4]);
        let i = inspect_standalone_patch(p).unwrap();
        assert_eq!(i.format, StandalonePatchFormat::XdeltaVcdiff);
        let (_d, p) = temp_file("x.ppf", b"PPF3.0");
        assert_eq!(
            inspect_standalone_patch(p).unwrap().format,
            StandalonePatchFormat::Ppf
        );
    }

    #[test]
    fn vcdiff_magic_matches_rfc3284_and_rejects_the_transposed_order() {
        // RFC 3284 section 4.1: 0xd6 0xc3 0xc4, i.e. "VCD" with each most
        // significant bit set. Derived from the ASCII letters rather than
        // written as a literal, so a future transposition cannot satisfy
        // both this test and the parser at once.
        let rfc_magic = [b'V' | 0x80, b'C' | 0x80, b'D' | 0x80];
        assert_eq!(rfc_magic, [0xd6, 0xc3, 0xc4]);
        let (_dir, path) = temp_file("real.xdelta", &[rfc_magic[0], rfc_magic[1], rfc_magic[2], 0]);
        let inspection = inspect_standalone_patch(&path).unwrap();
        assert_eq!(inspection.format, StandalonePatchFormat::XdeltaVcdiff);
        assert_eq!(inspection.state, PatchInspectionState::Valid);

        // The historical bug: bytes 2 and 3 swapped. That is not VCDIFF and
        // must never be reported as one again.
        let (_dir, path) = temp_file("transposed.xdelta", &[0xd6, 0xc4, 0xc3, 0]);
        assert_ne!(
            inspect_standalone_patch(&path).unwrap().format,
            StandalonePatchFormat::XdeltaVcdiff,
            "the transposed magic must not be detected as VCDIFF"
        );
    }

    #[test]
    fn real_xdelta3_header_bytes_are_detected() {
        // The first 16 bytes emitted by xdelta3 3.0.11 for a synthetic
        // base/target pair: VCDIFF magic, version 0, an indicator byte, then
        // the application header carrying the two local file names. No game
        // data is involved.
        let header: [u8; 16] = [
            0xd6, 0xc3, 0xc4, 0x00, 0x05, 0x02, 0x15, b't', b'a', b'r', b'g', b'e', b't', b'.',
            b'b', b'i',
        ];
        let (_dir, path) = temp_file("from-xdelta3.xdelta", &header);
        let inspection = inspect_standalone_patch(&path).unwrap();
        assert_eq!(inspection.format, StandalonePatchFormat::XdeltaVcdiff);
        assert_eq!(inspection.state, PatchInspectionState::Valid);
    }

    #[test]
    fn copier_header_match_requires_platform_rule_and_exact_normalized_crc() {
        let base = [0u8; 512]
            .into_iter()
            .chain([1u8, 2, 3])
            .collect::<Vec<_>>();
        let inspection = StandalonePatchInspection {
            path: PathBuf::from("patch.bps"),
            format: StandalonePatchFormat::Bps,
            state: PatchInspectionState::Valid,
            patch_size: 1,
            patch_sha256: "patch".into(),
            source_size: Some(3),
            target_size: Some(3),
            source_crc32: Some(crc32(&[1, 2, 3])),
            target_crc32: None,
            patch_crc32: None,
            metadata: None,
            warnings: Vec::new(),
            error: None,
        };
        let (matching, adjustment) =
            match_patch_source_with_header(&inspection, Some(&base), false, true);
        assert_eq!(matching.compatibility, PatchCompatibility::Compatible);
        assert_eq!(adjustment, HeaderAdjustment::Strip512ByteCopierHeader);
        let (matching, adjustment) =
            match_patch_source_with_header(&inspection, Some(&base), false, false);
        assert_eq!(matching.compatibility, PatchCompatibility::Incompatible);
        assert_eq!(adjustment, HeaderAdjustment::None);
    }

    #[test]
    fn header_transform_applies_to_derived_output_and_keeps_source_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let base_path = temp.path().join("base with header.sfc");
        let output_path = temp.path().join("patched.sfc");
        let base = [0xabu8; 512]
            .into_iter()
            .chain([1u8, 2, 3])
            .collect::<Vec<_>>();
        fs::write(&base_path, &base).unwrap();
        let mut patch = b"BPS1".to_vec();
        patch.extend(var(3));
        patch.extend(var(3));
        patch.extend(var(0));
        let mut body = action(1, 1);
        body.push(b'z');
        body.extend(action(0, 2));
        patch.extend(body);
        patch.extend(crc32(&[1, 2, 3]).to_le_bytes());
        patch.extend(crc32(b"z\x02\x03").to_le_bytes());
        let patch_crc = crc32(&patch);
        patch.extend(patch_crc.to_le_bytes());
        let patch_path = temp.path().join("patch with spaces.bps");
        fs::write(&patch_path, patch).unwrap();
        let inspection = inspect_standalone_patch(&patch_path).unwrap();
        let plan = build_standalone_patch_apply_plan_with_header(
            &inspection,
            &base_path,
            &output_path,
            temp.path(),
            HeaderAdjustment::Strip512ByteCopierHeader,
        )
        .unwrap();
        let result = apply_standalone_patch(&plan).unwrap();
        assert_eq!(fs::read(&base_path).unwrap(), base);
        assert_eq!(fs::read(&output_path).unwrap(), [b'z', 2, 3]);
        assert_eq!(
            result.provenance.header_adjustment,
            HeaderAdjustment::Strip512ByteCopierHeader
        );
        assert_eq!(result.provenance.base_path, base_path);
        assert_eq!(result.provenance.patch_path, patch_path);
        assert_eq!(result.provenance.output_path, output_path);
        assert!(temp.path().join("patched.sfc.emuwiz-patch.json").is_file());
        assert!(result.provenance.applied_at_unix_seconds > 0);
    }
}
