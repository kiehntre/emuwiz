//! Read-only Dreamcast DCP patch inspection and readiness.
//!
//! DCP is a ZIP-shaped package used by Universal Dreamcast Patcher.  Its
//! standard package contract contains changed/new filesystem files and may
//! contain `bootsector/IP.BIN`; it does not carry a cryptographic source
//! identity.  This module therefore never treats a filename, title, or DCP
//! presence as a target match.  An exact target binding must be supplied by a
//! trusted caller/catalogue before this reaches `ReadyToPreview`.
//!
//! No patch application, filesystem extraction, image rebuild, or source
//! mutation is performed here.

use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::dreamcast_boot_evidence::{
    DreamcastIpBinInspection, IP_BIN_META_BYTES, parse_ip_bin_meta,
};
use crate::standalone_patch::{
    PatchInspectionState, StandalonePatchFormat, inspect_standalone_patch,
};
use serde::{Deserialize, Serialize};

pub const MAX_DCP_ENTRIES: usize = 512;
pub const MAX_DCP_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_DCP_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_DCP_PATH_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastPatchFormat {
    Dcp,
    Ips,
    Bps,
    Ups,
    XdeltaVcdiff,
    Ppf,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastPatchReadinessState {
    ReadyToPreview,
    PossiblyReady,
    NotReady,
    Unsupported,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastPatchReadinessReason {
    MissingTargetBinding,
    ExactTargetMismatch,
    WeakTargetEvidence,
    ConflictingTargetEvidence,
    MalformedPackage,
    UnsafePackagePath,
    PackageLimit,
    UnsupportedPatchFormat,
    OpaquePatchSemantics,
    IpBinChange,
    FilesystemRebuildRequired,
    LayoutImpactUnknown,
    AudioImpactUnknown,
    GdiTopologyUnknown,
    CdiUnsupported,
    ChdRepresentationUnknown,
    UnsupportedTrackMode,
    MissingSourceManifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastPatchEntryKind {
    FileReplacement,
    FileDelta,
    IpBin,
    Metadata,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastIpBinImpact {
    Unchanged,
    Replaced,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastTrackImpact {
    DataFilesystemOnly,
    DataAndIpBin,
    AudioNotAddressedByDcp,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamcastPatchEntry {
    pub relative_path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub kind: DreamcastPatchEntryKind,
    pub filesystem_member: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamcastPatchPackage {
    pub path: PathBuf,
    pub format: DreamcastPatchFormat,
    pub package_sha256: String,
    pub entries: Vec<DreamcastPatchEntry>,
    pub source_hashes_embedded: bool,
    pub ip_bin_impact: DreamcastIpBinImpact,
    pub track_impact: DreamcastTrackImpact,
    pub audio_modification_supported_by_dcp: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamcastPatchTargetClaim {
    /// DCP itself does not populate this.  A signed/catalogued provider claim
    /// may bind a package to one exact source image or data track.
    pub full_image_sha256: Option<String>,
    pub data_track_sha256: Option<String>,
    pub product_code: Option<String>,
    pub product_code_is_cryptographically_tied: bool,
    pub supported_input_formats: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamcastPatchTargetEvidence {
    pub full_image_sha256: Option<String>,
    pub data_track_sha256: Option<String>,
    pub product_code: Option<String>,
    pub product_code_is_cryptographically_tied: bool,
    pub gdi_topology_fingerprint: Option<String>,
    pub input_format: Option<String>,
    pub has_audio_tracks: bool,
    pub audio_hashes: Vec<String>,
    pub ip_bin: Option<DreamcastIpBinInspectionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamcastIpBinInspectionSummary {
    pub hardware_id: String,
    pub product_number: String,
    pub area_symbols: String,
    pub peripheral_flags: String,
    pub vga_compatibility: String,
}

impl From<&DreamcastIpBinInspection> for DreamcastIpBinInspectionSummary {
    fn from(value: &DreamcastIpBinInspection) -> Self {
        Self {
            hardware_id: value.hardware_id.value.clone(),
            product_number: value.product_number.value.clone(),
            area_symbols: value.area_symbols.value.clone(),
            peripheral_flags: value.peripheral_flags.value.clone(),
            vga_compatibility: format!("{:?}", value.vga_compatibility),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamcastPatchReadiness {
    pub package: DreamcastPatchPackage,
    pub target_claim: DreamcastPatchTargetClaim,
    pub target_evidence: DreamcastPatchTargetEvidence,
    pub target_match: DreamcastPatchTargetMatch,
    pub state: DreamcastPatchReadinessState,
    pub reasons: Vec<DreamcastPatchReadinessReason>,
    pub warnings: Vec<String>,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastPatchTargetMatch {
    ExactFullImage,
    ExactDataTrack,
    ExactCryptographicallyTiedProductCode,
    NoExactMatch,
    Mismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamcastPatchInspectionError {
    Io(String),
    Malformed(String),
    UnsafePath(String),
    PackageLimit(String),
    Unsupported(String),
}

impl std::fmt::Display for DreamcastPatchInspectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(value) => write!(f, "Dreamcast patch I/O error: {value}"),
            Self::Malformed(value) => write!(f, "malformed Dreamcast patch: {value}"),
            Self::UnsafePath(value) => write!(f, "unsafe Dreamcast patch path: {value}"),
            Self::PackageLimit(value) => write!(f, "Dreamcast patch package limit: {value}"),
            Self::Unsupported(value) => write!(f, "unsupported Dreamcast patch: {value}"),
        }
    }
}
impl std::error::Error for DreamcastPatchInspectionError {}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes.as_ref())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn safe_relative_path(name: &str) -> Result<(), DreamcastPatchInspectionError> {
    if name.is_empty()
        || name.len() > MAX_DCP_PATH_BYTES
        || name.contains('\0')
        || name.contains(':')
        || name.contains('\\')
        || name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(DreamcastPatchInspectionError::UnsafePath(name.into()));
    }
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(DreamcastPatchInspectionError::UnsafePath(name.into()));
    }
    Ok(())
}

fn entry_kind(name: &str, bytes: &[u8]) -> (DreamcastPatchEntryKind, bool, Vec<String>) {
    let normalized = name.replace('\\', "/").to_ascii_lowercase();
    if normalized == "bootsector/ip.bin" || normalized == "bootsector\\ip.bin" {
        let mut warnings = Vec::new();
        if bytes.len() < IP_BIN_META_BYTES {
            warnings.push("IP.BIN entry is shorter than the inspected metadata area".into());
        } else if parse_ip_bin_meta(bytes).is_none() {
            warnings.push("IP.BIN metadata could not be inspected".into());
        }
        return (DreamcastPatchEntryKind::IpBin, false, warnings);
    }
    if normalized == "readme.txt"
        || normalized == "readme.md"
        || normalized == "patch.ini"
        || normalized == "metadata.json"
    {
        return (DreamcastPatchEntryKind::Metadata, false, Vec::new());
    }
    if normalized.ends_with(".xdelta") || normalized.ends_with(".vcdiff") {
        return (
            DreamcastPatchEntryKind::FileDelta,
            true,
            vec!["file delta semantics are recorded but not decoded".into()],
        );
    }
    (DreamcastPatchEntryKind::FileReplacement, true, Vec::new())
}

/// Inspect the standard ZIP-shaped DCP package without extracting or applying
/// any member.
pub fn inspect_dreamcast_dcp(
    path: impl AsRef<Path>,
) -> Result<DreamcastPatchPackage, DreamcastPatchInspectionError> {
    let path = path.as_ref().to_path_buf();
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| DreamcastPatchInspectionError::Io(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DreamcastPatchInspectionError::UnsafePath(
            "package must be a regular non-symlink file".into(),
        ));
    }
    if metadata.len() > MAX_DCP_EXPANDED_BYTES {
        return Err(DreamcastPatchInspectionError::PackageLimit(
            "compressed package is too large".into(),
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .map_err(|e| DreamcastPatchInspectionError::Io(e.to_string()))?
        .take(MAX_DCP_EXPANDED_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| DreamcastPatchInspectionError::Io(e.to_string()))?;
    if bytes.len() as u64 > MAX_DCP_EXPANDED_BYTES {
        return Err(DreamcastPatchInspectionError::PackageLimit(
            "compressed package grew beyond bound".into(),
        ));
    }
    // The ZIP library collapses duplicate names in its index and allocates
    // from untrusted entry counts. Reuse the canonical bounded preflight first.
    let limits = crate::dat::archive::limits::ArchiveLimits {
        max_members: MAX_DCP_ENTRIES,
        max_member_logical_bytes: MAX_DCP_ENTRY_BYTES,
        max_archive_logical_bytes: MAX_DCP_EXPANDED_BYTES,
        ..Default::default()
    };
    let preflight = crate::dat::archive::zip_preflight::preflight_zip(
        &mut std::io::Cursor::new(&bytes),
        bytes.len() as u64,
        &limits,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .map_err(|e| DreamcastPatchInspectionError::Malformed(format!("ZIP preflight: {e:?}")))?;
    for entry in &preflight.entries {
        std::str::from_utf8(&entry.name_raw).map_err(|_| {
            DreamcastPatchInspectionError::UnsafePath("non-UTF8 member name".into())
        })?;
        if !matches!(entry.method, 0 | 8)
            || entry.flags & ((1 << 0) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 13)) != 0
        {
            return Err(DreamcastPatchInspectionError::Unsupported(
                "only unencrypted stored/deflated ZIP members are supported".into(),
            ));
        }
        if entry.is_directory && entry.logical_size != 0 {
            return Err(DreamcastPatchInspectionError::Malformed(
                "directory has payload".into(),
            ));
        }
    }
    let package_sha256 = hex_digest(&bytes);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|error| DreamcastPatchInspectionError::Malformed(error.to_string()))?;
    if archive.len() != preflight.entry_count {
        return Err(DreamcastPatchInspectionError::UnsafePath(
            "duplicate ZIP member names".into(),
        ));
    }
    if archive.len() > MAX_DCP_ENTRIES {
        return Err(DreamcastPatchInspectionError::PackageLimit(
            "too many entries".into(),
        ));
    }
    let mut entries = Vec::new();
    let mut expanded_bytes = 0_u64;
    let mut names = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut warnings = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| DreamcastPatchInspectionError::Malformed(error.to_string()))?;
        let name = entry.name().to_owned();
        let directory = name.ends_with('/');
        let relative = if directory {
            &name[..name.len() - 1]
        } else {
            &name
        };
        safe_relative_path(relative)?;
        let normalized = relative.to_ascii_lowercase();
        if !names.insert(normalized.clone()) {
            return Err(DreamcastPatchInspectionError::UnsafePath(
                "duplicate/conflicting member".into(),
            ));
        }
        let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
        if kind != 0 && kind != if directory { 0o040000 } else { 0o100000 } {
            return Err(DreamcastPatchInspectionError::UnsafePath(
                "special filesystem member".into(),
            ));
        }
        if directory {
            continue;
        }
        files.insert(normalized);
        let size = entry.size();
        if size > MAX_DCP_ENTRY_BYTES {
            return Err(DreamcastPatchInspectionError::PackageLimit(format!(
                "entry {name} is too large"
            )));
        }
        expanded_bytes = expanded_bytes.checked_add(size).ok_or_else(|| {
            DreamcastPatchInspectionError::PackageLimit("expanded size overflow".into())
        })?;
        if expanded_bytes > MAX_DCP_EXPANDED_BYTES {
            return Err(DreamcastPatchInspectionError::PackageLimit(
                "expanded package is too large".into(),
            ));
        }
        let mut member = Vec::with_capacity(size.min(1024 * 1024) as usize);
        entry
            .by_ref()
            .take(size + 1)
            .read_to_end(&mut member)
            .map_err(|error| DreamcastPatchInspectionError::Io(error.to_string()))?;
        if member.len() as u64 != size {
            return Err(DreamcastPatchInspectionError::Malformed(format!(
                "entry {name} size changed while reading"
            )));
        }
        let (kind, filesystem_member, entry_warnings) = entry_kind(&name, &member);
        warnings.extend(
            entry_warnings
                .iter()
                .map(|warning| format!("{name}: {warning}")),
        );
        entries.push(DreamcastPatchEntry {
            relative_path: name,
            size_bytes: size,
            sha256: hex_digest(&member),
            kind,
            filesystem_member,
            warnings: entry_warnings,
        });
    }
    for name in &names {
        let mut parent = Path::new(name).parent();
        while let Some(p) = parent {
            if files.contains(p.to_str().unwrap_or("")) {
                return Err(DreamcastPatchInspectionError::UnsafePath(
                    "file/directory target conflict".into(),
                ));
            }
            parent = p.parent();
        }
    }
    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let ip_bin_impact = if entries
        .iter()
        .any(|entry| entry.kind == DreamcastPatchEntryKind::IpBin)
    {
        DreamcastIpBinImpact::Replaced
    } else {
        DreamcastIpBinImpact::Unchanged
    };
    let filesystem_entries = entries
        .iter()
        .filter(|entry| entry.filesystem_member)
        .count();
    let track_impact = match (filesystem_entries > 0, ip_bin_impact) {
        (true, DreamcastIpBinImpact::Replaced) => DreamcastTrackImpact::DataAndIpBin,
        (true, DreamcastIpBinImpact::Unchanged) => DreamcastTrackImpact::DataFilesystemOnly,
        (false, DreamcastIpBinImpact::Replaced) => DreamcastTrackImpact::DataAndIpBin,
        (false, DreamcastIpBinImpact::Unchanged) => DreamcastTrackImpact::Unknown,
        (_, DreamcastIpBinImpact::Unknown) => DreamcastTrackImpact::Unknown,
    };
    Ok(DreamcastPatchPackage {
        path,
        format: DreamcastPatchFormat::Dcp,
        package_sha256,
        entries,
        source_hashes_embedded: false,
        ip_bin_impact,
        track_impact,
        audio_modification_supported_by_dcp: false,
        warnings,
    })
}

fn standalone_format(format: StandalonePatchFormat) -> DreamcastPatchFormat {
    match format {
        StandalonePatchFormat::Ips => DreamcastPatchFormat::Ips,
        StandalonePatchFormat::Bps => DreamcastPatchFormat::Bps,
        StandalonePatchFormat::Ups => DreamcastPatchFormat::Ups,
        StandalonePatchFormat::XdeltaVcdiff => DreamcastPatchFormat::XdeltaVcdiff,
        StandalonePatchFormat::Ppf => DreamcastPatchFormat::Ppf,
        StandalonePatchFormat::Unknown => DreamcastPatchFormat::Unknown,
    }
}

/// Classify a local patch without claiming that non-DCP formats have
/// Dreamcast filesystem or GD-ROM semantics.
pub fn classify_dreamcast_patch(
    path: impl AsRef<Path>,
) -> Result<DreamcastPatchFormat, DreamcastPatchInspectionError> {
    let path = path.as_ref();
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("dcp"))
    {
        return Ok(DreamcastPatchFormat::Dcp);
    }
    let inspection = inspect_standalone_patch(path)
        .map_err(|error| DreamcastPatchInspectionError::Malformed(error.to_string()))?;
    Ok(standalone_format(inspection.format))
}

fn target_match(
    claim: &DreamcastPatchTargetClaim,
    evidence: &DreamcastPatchTargetEvidence,
) -> DreamcastPatchTargetMatch {
    if let Some(expected) = claim.full_image_sha256.as_deref() {
        return if evidence.full_image_sha256.as_deref() == Some(expected) {
            DreamcastPatchTargetMatch::ExactFullImage
        } else {
            DreamcastPatchTargetMatch::Mismatch
        };
    }
    if let Some(expected) = claim.data_track_sha256.as_deref() {
        return if evidence.data_track_sha256.as_deref() == Some(expected) {
            DreamcastPatchTargetMatch::ExactDataTrack
        } else {
            DreamcastPatchTargetMatch::Mismatch
        };
    }
    if claim.product_code_is_cryptographically_tied {
        return if claim.product_code.is_some()
            && claim.product_code == evidence.product_code
            && evidence.product_code_is_cryptographically_tied
        {
            DreamcastPatchTargetMatch::ExactCryptographicallyTiedProductCode
        } else {
            DreamcastPatchTargetMatch::Mismatch
        };
    }
    DreamcastPatchTargetMatch::NoExactMatch
}

pub fn assess_dreamcast_patch_readiness(
    package: &DreamcastPatchPackage,
    claim: &DreamcastPatchTargetClaim,
    evidence: &DreamcastPatchTargetEvidence,
) -> DreamcastPatchReadiness {
    let target_match = target_match(claim, evidence);
    let mut reasons = Vec::new();
    let mut warnings = package.warnings.clone();
    if package.format != DreamcastPatchFormat::Dcp {
        reasons.push(DreamcastPatchReadinessReason::UnsupportedPatchFormat);
    }
    match target_match {
        DreamcastPatchTargetMatch::Mismatch => {
            reasons.push(DreamcastPatchReadinessReason::ExactTargetMismatch)
        }
        DreamcastPatchTargetMatch::NoExactMatch => {
            reasons.push(DreamcastPatchReadinessReason::MissingTargetBinding);
            reasons.push(DreamcastPatchReadinessReason::WeakTargetEvidence);
        }
        _ => {}
    }
    if package.ip_bin_impact == DreamcastIpBinImpact::Replaced {
        reasons.push(DreamcastPatchReadinessReason::IpBinChange);
        warnings
            .push("the patch replaces IP.BIN; region/VGA/boot metadata must be reviewed".into());
    }
    if package.entries.iter().any(|entry| entry.filesystem_member) {
        reasons.push(DreamcastPatchReadinessReason::FilesystemRebuildRequired);
        reasons.push(DreamcastPatchReadinessReason::LayoutImpactUnknown);
    }
    if evidence.has_audio_tracks {
        warnings.push(
            "DCP does not address CDDA tracks; audio must be independently verified unchanged"
                .into(),
        );
    }
    if package.track_impact != DreamcastTrackImpact::DataFilesystemOnly
        && package.track_impact != DreamcastTrackImpact::DataAndIpBin
    {
        reasons.push(DreamcastPatchReadinessReason::GdiTopologyUnknown);
    }
    if evidence
        .input_format
        .as_deref()
        .is_some_and(|format| format.eq_ignore_ascii_case("cdi"))
    {
        reasons.push(DreamcastPatchReadinessReason::CdiUnsupported);
    }
    if evidence
        .input_format
        .as_deref()
        .is_some_and(|format| format.eq_ignore_ascii_case("chd"))
    {
        reasons.push(DreamcastPatchReadinessReason::ChdRepresentationUnknown);
    }
    if evidence.has_audio_tracks {
        reasons.push(DreamcastPatchReadinessReason::AudioImpactUnknown);
    }
    let state = if reasons.contains(&DreamcastPatchReadinessReason::ConflictingTargetEvidence) {
        DreamcastPatchReadinessState::Ambiguous
    } else if reasons.contains(&DreamcastPatchReadinessReason::UnsupportedPatchFormat)
        || reasons.contains(&DreamcastPatchReadinessReason::CdiUnsupported)
    {
        DreamcastPatchReadinessState::Unsupported
    } else if matches!(
        target_match,
        DreamcastPatchTargetMatch::ExactFullImage
            | DreamcastPatchTargetMatch::ExactDataTrack
            | DreamcastPatchTargetMatch::ExactCryptographicallyTiedProductCode
    ) && !reasons.contains(&DreamcastPatchReadinessReason::ExactTargetMismatch)
    {
        DreamcastPatchReadinessState::PossiblyReady
    } else {
        DreamcastPatchReadinessState::NotReady
    };
    let explanation = if state == DreamcastPatchReadinessState::PossiblyReady {
        "The DCP target binding is exact, but filesystem rebuild, IP.BIN, topology, and audio invariants still require an explicit preview and post-build verification.".into()
    } else {
        "The Dreamcast patch is read-only inspected but cannot be considered safely ready for apply.".into()
    };
    DreamcastPatchReadiness {
        package: package.clone(),
        target_claim: claim.clone(),
        target_evidence: evidence.clone(),
        target_match,
        state,
        reasons,
        warnings,
        explanation,
    }
}

/// Inspect a non-DCP patch and return a refusal-shaped package projection.
pub fn unsupported_dreamcast_patch_package(
    path: impl AsRef<Path>,
) -> Result<DreamcastPatchPackage, DreamcastPatchInspectionError> {
    let path = path.as_ref().to_path_buf();
    let inspection = inspect_standalone_patch(&path)
        .map_err(|error| DreamcastPatchInspectionError::Malformed(error.to_string()))?;
    if inspection.state == PatchInspectionState::Invalid {
        return Err(DreamcastPatchInspectionError::Malformed(
            inspection.error.unwrap_or_else(|| "invalid patch".into()),
        ));
    }
    Ok(DreamcastPatchPackage {
        path,
        format: standalone_format(inspection.format),
        package_sha256: inspection.patch_sha256,
        entries: Vec::new(),
        source_hashes_embedded: inspection.source_crc32.is_some(),
        ip_bin_impact: DreamcastIpBinImpact::Unknown,
        track_impact: DreamcastTrackImpact::Unknown,
        audio_modification_supported_by_dcp: false,
        warnings: vec!["standalone patch semantics are not Dreamcast filesystem semantics".into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn dcp(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        file
    }

    fn evidence() -> DreamcastPatchTargetEvidence {
        DreamcastPatchTargetEvidence {
            full_image_sha256: Some("full".into()),
            data_track_sha256: Some("track".into()),
            product_code: Some("T-1234M".into()),
            ..Default::default()
        }
    }

    #[test]
    fn dcp_inspection_is_read_only_and_identifies_ip_bin_and_files() {
        let package = dcp(&[
            ("1ST_READ.BIN", b"changed"),
            ("bootsector/IP.BIN", &[0; 256]),
        ]);
        let before = fs::read(package.path()).unwrap();
        let inspection = inspect_dreamcast_dcp(package.path()).unwrap();
        assert_eq!(inspection.format, DreamcastPatchFormat::Dcp);
        assert_eq!(inspection.ip_bin_impact, DreamcastIpBinImpact::Replaced);
        assert!(inspection.source_hashes_embedded == false);
        assert_eq!(before, fs::read(package.path()).unwrap());
    }

    #[test]
    fn exact_target_binding_is_previewable_but_rebuild_impacts_remain_visible() {
        let package = dcp(&[("1ST_READ.BIN", b"changed")]);
        let package = inspect_dreamcast_dcp(package.path()).unwrap();
        let readiness = assess_dreamcast_patch_readiness(
            &package,
            &DreamcastPatchTargetClaim {
                full_image_sha256: Some("full".into()),
                ..Default::default()
            },
            &evidence(),
        );
        assert_eq!(
            readiness.target_match,
            DreamcastPatchTargetMatch::ExactFullImage
        );
        assert_eq!(readiness.state, DreamcastPatchReadinessState::PossiblyReady);
        assert!(
            readiness
                .reasons
                .contains(&DreamcastPatchReadinessReason::FilesystemRebuildRequired)
        );
    }

    #[test]
    fn no_hash_in_dcp_fails_closed_and_opaque_formats_are_unsupported() {
        let package = dcp(&[("README.md", b"title-only")]);
        let package = inspect_dreamcast_dcp(package.path()).unwrap();
        let readiness = assess_dreamcast_patch_readiness(
            &package,
            &DreamcastPatchTargetClaim::default(),
            &evidence(),
        );
        assert_eq!(readiness.state, DreamcastPatchReadinessState::NotReady);
        assert!(
            readiness
                .reasons
                .contains(&DreamcastPatchReadinessReason::MissingTargetBinding)
        );
    }

    #[test]
    fn ip_bin_and_audio_impacts_remain_explicit() {
        let package = dcp(&[("bootsector/IP.BIN", &[0; 256])]);
        let package = inspect_dreamcast_dcp(package.path()).unwrap();
        let mut target = evidence();
        target.has_audio_tracks = true;
        let readiness = assess_dreamcast_patch_readiness(
            &package,
            &DreamcastPatchTargetClaim {
                data_track_sha256: Some("track".into()),
                ..Default::default()
            },
            &target,
        );
        assert!(
            readiness
                .reasons
                .contains(&DreamcastPatchReadinessReason::IpBinChange)
        );
        assert!(
            readiness
                .reasons
                .contains(&DreamcastPatchReadinessReason::AudioImpactUnknown)
        );
    }

    #[test]
    fn malformed_package_is_refused() {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), b"not a zip").unwrap();
        assert!(matches!(
            inspect_dreamcast_dcp(file.path()),
            Err(DreamcastPatchInspectionError::Malformed(_))
        ));
    }

    #[test]
    fn traversal_is_refused() {
        let package = dcp(&[("../outside.bin", b"bad")]);
        assert!(matches!(
            inspect_dreamcast_dcp(package.path()),
            Err(DreamcastPatchInspectionError::UnsafePath(_))
        ));
    }
}
