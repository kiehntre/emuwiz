//! Read-only Saturn patch readiness.
//!
//! This module deliberately stops at a typed preview.  It does not invoke a
//! patcher, rebuild a data track, rewrite a CUE sheet, or mutate any source.
//! A generic patch format is only useful here when its source semantics are
//! supplied explicitly by the caller; a title, product number, filename, or
//! emulator boot result is never a base identity.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::saturn_disc_manifest::{
    SaturnDiscManifest, SaturnManifestStatus, SaturnTrackType, verify_saturn_manifest,
};
use crate::standalone_patch::{
    PatchInspectionState, StandalonePatchFormat, StandalonePatchInspection,
    inspect_standalone_patch,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaturnPatchReadinessState {
    ReadyToPreview,
    PossiblyReady,
    NotReady,
    Unsupported,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaturnPatchTargetKind {
    FullRawImage,
    LogicalDataTrack,
    ComponentBin,
    FilesystemFile,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnPatchTarget {
    FullRawImage { component: PathBuf, sha256: String },
    LogicalDataTrack { track_number: u32, sha256: String },
    ComponentBin { component: PathBuf, sha256: String },
    FilesystemFile { path: PathBuf },
    Unknown,
}

impl SaturnPatchTarget {
    pub fn kind(&self) -> SaturnPatchTargetKind {
        match self {
            Self::FullRawImage { .. } => SaturnPatchTargetKind::FullRawImage,
            Self::LogicalDataTrack { .. } => SaturnPatchTargetKind::LogicalDataTrack,
            Self::ComponentBin { .. } => SaturnPatchTargetKind::ComponentBin,
            Self::FilesystemFile { .. } => SaturnPatchTargetKind::FilesystemFile,
            Self::Unknown => SaturnPatchTargetKind::Unknown,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnPatchProvenance {
    pub source: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub tool: Option<String>,
    pub provider_snapshot_sha256: Option<String>,
    pub claimed_game: Option<String>,
    pub claimed_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnExternalEvidence {
    pub provider: String,
    pub claim: String,
    pub snapshot_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnExpectedOutput {
    pub manifest_fingerprint: Option<String>,
    pub target_sha256: Option<String>,
    pub target_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnPatchReadinessRequest {
    pub manifest: SaturnDiscManifest,
    pub expected_manifest_fingerprint: Option<String>,
    pub patch_path: PathBuf,
    pub target: SaturnPatchTarget,
    pub provenance: SaturnPatchProvenance,
    pub expected_output: Option<SaturnExpectedOutput>,
    pub external_evidence: Vec<SaturnExternalEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaturnPatchBaseEvidence {
    ExactManifestFingerprint,
    ExactTargetHash,
    EmbeddedSourceChecksum,
    StrongExternalIdentity,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnPatchFormatCapabilities {
    pub source_size: Option<u64>,
    pub source_checksum: Option<u32>,
    pub expected_output_size: Option<u64>,
    pub expected_output_checksum: Option<u32>,
    pub changed_ranges: Option<String>,
    pub target_semantics: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnPatchInspectionSummary {
    pub path: PathBuf,
    pub format: StandalonePatchFormat,
    pub state: String,
    pub sha256: String,
    pub capabilities: SaturnPatchFormatCapabilities,
    pub metadata: Option<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnPatchReadinessReason {
    IncompleteManifest,
    ManifestUnsafe,
    SourceHashMismatch,
    PatchBaseUnknown,
    PatchFormatUnsupported,
    PatchTargetUnknown,
    MixedModeTargetAmbiguous,
    MultipleCandidateDataTracks,
    AudioImpactUnknown,
    TrackTopologyImpactUnknown,
    ExpectedOutputUnknown,
    OpaqueSspPackage,
    MalformedPatch,
    PatchPathUnsafe,
    SourceDestinationOverlap,
    NativeIdentityConflict,
    ProviderEvidenceConflict,
    UnsupportedRepresentation,
}

impl std::fmt::Display for SaturnPatchReadinessReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::IncompleteManifest => "the Saturn manifest is incomplete",
            Self::ManifestUnsafe => "the Saturn manifest is unsafe or invalid",
            Self::SourceHashMismatch => "the exact source hash does not match",
            Self::PatchBaseUnknown => "the patch does not identify its source strongly enough",
            Self::PatchFormatUnsupported => "the patch format has no supported Saturn semantics",
            Self::PatchTargetUnknown => "the patch target is unknown",
            Self::MixedModeTargetAmbiguous => "the mixed-mode target is ambiguous",
            Self::MultipleCandidateDataTracks => "multiple data tracks are possible targets",
            Self::AudioImpactUnknown => "audio impact cannot be proven absent",
            Self::TrackTopologyImpactUnknown => "track topology impact cannot be proven absent",
            Self::ExpectedOutputUnknown => "the expected output identity is unknown",
            Self::OpaqueSspPackage => {
                "SSP semantics are opaque and were not independently verified"
            }
            Self::MalformedPatch => "the patch is malformed",
            Self::PatchPathUnsafe => "the patch path is unsafe",
            Self::SourceDestinationOverlap => "the patch overlaps a Saturn source component",
            Self::NativeIdentityConflict => "native identity evidence conflicts",
            Self::ProviderEvidenceConflict => "external provider claims conflict",
            Self::UnsupportedRepresentation => "the Saturn representation is unsupported",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnPatchReadiness {
    pub source_manifest: SaturnDiscManifest,
    pub manifest_fingerprint: String,
    pub patch: Option<SaturnPatchInspectionSummary>,
    pub patch_provenance: SaturnPatchProvenance,
    pub target: SaturnPatchTarget,
    pub target_kind: SaturnPatchTargetKind,
    pub base_evidence: SaturnPatchBaseEvidence,
    pub expected_output: Option<SaturnExpectedOutput>,
    pub state: SaturnPatchReadinessState,
    pub reasons: Vec<SaturnPatchReadinessReason>,
    pub warnings: Vec<String>,
    pub external_evidence: Vec<SaturnExternalEvidence>,
    pub native_verified: bool,
    pub audio_expected_untouched: bool,
    pub topology_expected_untouched: bool,
    pub changed_ranges: Option<String>,
    pub explanation: String,
}

fn digest<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("readiness models are serializable");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn saturn_manifest_fingerprint(manifest: &SaturnDiscManifest) -> String {
    digest(manifest)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
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

fn safe_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
}

fn patch_summary(inspection: &StandalonePatchInspection) -> SaturnPatchInspectionSummary {
    let capabilities = match inspection.format {
        StandalonePatchFormat::Bps | StandalonePatchFormat::Ups => SaturnPatchFormatCapabilities {
            source_size: inspection.source_size,
            source_checksum: inspection.source_crc32,
            expected_output_size: inspection.target_size,
            expected_output_checksum: inspection.target_crc32,
            changed_ranges: None,
            target_semantics: "raw byte stream only; Saturn track semantics are caller-supplied"
                .into(),
        },
        StandalonePatchFormat::Ppf => SaturnPatchFormatCapabilities {
            source_size: inspection.source_size,
            source_checksum: None,
            expected_output_size: inspection.target_size,
            expected_output_checksum: None,
            changed_ranges: None,
            target_semantics: "PPF3 raw image stream; changed ranges are not exposed by the parser"
                .into(),
        },
        StandalonePatchFormat::Ips => SaturnPatchFormatCapabilities {
            source_size: None,
            source_checksum: None,
            expected_output_size: None,
            expected_output_checksum: None,
            changed_ranges: None,
            target_semantics: "raw byte stream; source and output identity are not embedded".into(),
        },
        StandalonePatchFormat::XdeltaVcdiff => SaturnPatchFormatCapabilities {
            source_size: inspection.source_size,
            source_checksum: inspection.source_crc32,
            expected_output_size: inspection.target_size,
            expected_output_checksum: inspection.target_crc32,
            changed_ranges: None,
            target_semantics: "opaque VCDIFF byte stream; Saturn target semantics are not embedded"
                .into(),
        },
        StandalonePatchFormat::Unknown => SaturnPatchFormatCapabilities {
            source_size: None,
            source_checksum: None,
            expected_output_size: None,
            expected_output_checksum: None,
            changed_ranges: None,
            target_semantics: "unknown".into(),
        },
    };
    SaturnPatchInspectionSummary {
        path: inspection.path.clone(),
        format: inspection.format,
        state: format!("{:?}", inspection.state),
        sha256: inspection.patch_sha256.clone(),
        capabilities,
        metadata: inspection.metadata.clone(),
        warnings: inspection.warnings.clone(),
        error: inspection.error.clone(),
    }
}

fn target_component(target: &SaturnPatchTarget) -> Option<&Path> {
    match target {
        SaturnPatchTarget::FullRawImage { component, .. }
        | SaturnPatchTarget::ComponentBin { component, .. } => Some(component),
        _ => None,
    }
}

fn component_is_audio_only(manifest: &SaturnDiscManifest, component: &Path) -> bool {
    let tracks: Vec<_> = manifest
        .tracks
        .iter()
        .filter(|track| track.source_file == component)
        .collect();
    !tracks.is_empty()
        && tracks
            .iter()
            .all(|track| track.track_type == SaturnTrackType::Audio)
}

fn component_has_audio(manifest: &SaturnDiscManifest, component: &Path) -> bool {
    manifest
        .tracks
        .iter()
        .any(|track| track.source_file == component && track.track_type == SaturnTrackType::Audio)
}

fn initial_result(request: &SaturnPatchReadinessRequest) -> SaturnPatchReadiness {
    SaturnPatchReadiness {
        source_manifest: request.manifest.clone(),
        manifest_fingerprint: saturn_manifest_fingerprint(&request.manifest),
        patch: None,
        patch_provenance: request.provenance.clone(),
        target_kind: request.target.kind(),
        target: request.target.clone(),
        base_evidence: SaturnPatchBaseEvidence::None,
        expected_output: request.expected_output.clone(),
        state: SaturnPatchReadinessState::NotReady,
        reasons: Vec::new(),
        warnings: Vec::new(),
        external_evidence: request.external_evidence.clone(),
        native_verified: false,
        audio_expected_untouched: false,
        topology_expected_untouched: false,
        changed_ranges: None,
        explanation: String::new(),
    }
}

pub fn assess_saturn_patch_readiness(
    request: &SaturnPatchReadinessRequest,
) -> SaturnPatchReadiness {
    let mut result = initial_result(request);
    let mut claims = BTreeSet::new();
    for evidence in &request.external_evidence {
        claims.insert(evidence.claim.clone());
    }
    if claims.len() > 1 {
        result
            .reasons
            .push(SaturnPatchReadinessReason::ProviderEvidenceConflict);
    }

    if !safe_regular_file(&request.patch_path) {
        result
            .reasons
            .push(SaturnPatchReadinessReason::PatchPathUnsafe);
        result.explanation = "The selected patch is not a safe regular file.".into();
        return result;
    }
    if request.patch_path == request.manifest.source_descriptor
        || request
            .manifest
            .components
            .iter()
            .any(|component| component.path == request.patch_path)
    {
        result
            .reasons
            .push(SaturnPatchReadinessReason::SourceDestinationOverlap);
        result.explanation =
            "The patch must be separate from every Saturn source component.".into();
        return result;
    }

    let is_ssp = request
        .patch_path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ssp"));
    if is_ssp {
        let bytes = fs::read(&request.patch_path);
        if let Ok(bytes) = bytes {
            result.patch = Some(SaturnPatchInspectionSummary {
                path: request.patch_path.clone(),
                format: StandalonePatchFormat::Unknown,
                state: "Unsupported".into(),
                sha256: digest(&bytes),
                capabilities: SaturnPatchFormatCapabilities {
                    source_size: None,
                    source_checksum: None,
                    expected_output_size: None,
                    expected_output_checksum: None,
                    changed_ranges: None,
                    target_semantics: "opaque external Sega Saturn Patcher package".into(),
                },
                metadata: None,
                warnings: vec!["semantic contents were not independently verified".into()],
                error: None,
            });
        }
        result
            .reasons
            .push(SaturnPatchReadinessReason::OpaqueSspPackage);
        result.state = SaturnPatchReadinessState::Unsupported;
        result.explanation = "This SSP package can be recorded and inspected, but EmuWiz cannot verify its internal patch semantics.".into();
        return result;
    }

    let inspection = match inspect_standalone_patch(&request.patch_path) {
        Ok(inspection) => inspection,
        Err(error) => {
            result
                .reasons
                .push(SaturnPatchReadinessReason::MalformedPatch);
            result.warnings.push(error.to_string());
            result.explanation =
                "The patch could not be inspected as a bounded supported patch.".into();
            return result;
        }
    };
    result.patch = Some(patch_summary(&inspection));
    if inspection.state == PatchInspectionState::Invalid {
        result
            .reasons
            .push(SaturnPatchReadinessReason::MalformedPatch);
        result.explanation = "The patch framing or checksums are invalid.".into();
        return result;
    }
    if inspection.state == PatchInspectionState::Unsupported
        || inspection.format == StandalonePatchFormat::Unknown
    {
        result
            .reasons
            .push(SaturnPatchReadinessReason::PatchFormatUnsupported);
        result.state = SaturnPatchReadinessState::Unsupported;
        result.explanation =
            "This patch format is recognized only for bounded inspection, not Saturn readiness."
                .into();
        return result;
    }

    match request.manifest.status {
        SaturnManifestStatus::Complete | SaturnManifestStatus::CompleteWithWarnings => {}
        SaturnManifestStatus::Incomplete => result
            .reasons
            .push(SaturnPatchReadinessReason::IncompleteManifest),
        SaturnManifestStatus::Unsafe | SaturnManifestStatus::Invalid => result
            .reasons
            .push(SaturnPatchReadinessReason::ManifestUnsafe),
    }
    if request
        .manifest
        .tracks
        .iter()
        .filter(|track| track.track_type == SaturnTrackType::Data)
        .count()
        > 1
    {
        result
            .reasons
            .push(SaturnPatchReadinessReason::MultipleCandidateDataTracks);
    }
    if result.reasons.iter().any(|reason| {
        matches!(
            reason,
            SaturnPatchReadinessReason::IncompleteManifest
                | SaturnPatchReadinessReason::ManifestUnsafe
        )
    }) {
        result.explanation = "A complete, safe Saturn source manifest is required before patch readiness can be considered.".into();
        return result;
    }

    let verification = verify_saturn_manifest(&request.manifest);
    if !verification.issues.is_empty() {
        result
            .reasons
            .push(SaturnPatchReadinessReason::SourceHashMismatch);
        result.explanation =
            "The selected Saturn source no longer matches its reviewed manifest.".into();
        return result;
    }

    if let Some(expected) = request.expected_manifest_fingerprint.as_deref() {
        if expected == result.manifest_fingerprint {
            result.base_evidence = SaturnPatchBaseEvidence::ExactManifestFingerprint;
        } else {
            result
                .reasons
                .push(SaturnPatchReadinessReason::SourceHashMismatch);
            result.explanation =
                "The selected Saturn source does not match the expected manifest fingerprint."
                    .into();
            return result;
        }
    }

    if let Some(expected) = request.expected_output.as_ref() {
        if expected.manifest_fingerprint.is_none()
            && expected.target_sha256.is_none()
            && expected.target_size.is_none()
        {
            result
                .reasons
                .push(SaturnPatchReadinessReason::ExpectedOutputUnknown);
        }
    } else {
        result
            .warnings
            .push("No expected output identity was supplied; this remains preview-only.".into());
    }

    let target_kind = request.target.kind();
    if target_kind == SaturnPatchTargetKind::Unknown
        || target_kind == SaturnPatchTargetKind::FilesystemFile
    {
        result
            .reasons
            .push(SaturnPatchReadinessReason::PatchTargetUnknown);
    }
    if target_kind == SaturnPatchTargetKind::LogicalDataTrack {
        let SaturnPatchTarget::LogicalDataTrack {
            track_number,
            sha256,
        } = &request.target
        else {
            unreachable!()
        };
        let matching: Vec<_> = request
            .manifest
            .tracks
            .iter()
            .filter(|track| {
                track.number == *track_number && track.track_type == SaturnTrackType::Data
            })
            .collect();
        if matching.len() != 1 {
            result
                .reasons
                .push(SaturnPatchReadinessReason::MixedModeTargetAmbiguous);
        } else if matching[0].data_logical_sha256.as_deref() != Some(sha256.as_str()) {
            result
                .reasons
                .push(SaturnPatchReadinessReason::SourceHashMismatch);
        } else {
            if result.base_evidence == SaturnPatchBaseEvidence::None {
                result.base_evidence = SaturnPatchBaseEvidence::ExactTargetHash;
            }
            result.audio_expected_untouched = true;
            result.topology_expected_untouched = inspection.target_size == inspection.source_size;
            if !result.topology_expected_untouched {
                result
                    .reasons
                    .push(SaturnPatchReadinessReason::TrackTopologyImpactUnknown);
            }
        }
    }
    if let Some(component) = target_component(&request.target) {
        let known = request
            .manifest
            .components
            .iter()
            .any(|item| item.path == component);
        if !known {
            result
                .reasons
                .push(SaturnPatchReadinessReason::SourceHashMismatch);
        }
        if let Some(expected_hash) = match &request.target {
            SaturnPatchTarget::FullRawImage { sha256, .. }
            | SaturnPatchTarget::ComponentBin { sha256, .. } => Some(sha256),
            _ => None,
        } {
            let actual = request
                .manifest
                .components
                .iter()
                .find(|item| item.path == component)
                .map(|item| item.sha256.as_str());
            if actual != Some(expected_hash.as_str()) {
                result
                    .reasons
                    .push(SaturnPatchReadinessReason::SourceHashMismatch);
            } else if result.base_evidence == SaturnPatchBaseEvidence::None {
                result.base_evidence = SaturnPatchBaseEvidence::ExactTargetHash;
            }
        }
        if inspection.source_size.is_some() || inspection.source_crc32.is_some() {
            match fs::read(component) {
                Ok(bytes)
                    if inspection
                        .source_size
                        .is_none_or(|size| size == bytes.len() as u64)
                        && inspection
                            .source_crc32
                            .is_none_or(|expected| crc32(&bytes) == expected) =>
                {
                    if result.base_evidence == SaturnPatchBaseEvidence::None {
                        result.base_evidence = SaturnPatchBaseEvidence::EmbeddedSourceChecksum;
                    }
                }
                Ok(_) => result
                    .reasons
                    .push(SaturnPatchReadinessReason::SourceHashMismatch),
                Err(_) => result
                    .reasons
                    .push(SaturnPatchReadinessReason::UnsupportedRepresentation),
            }
        }
        let has_audio = component_has_audio(&request.manifest, component);
        if has_audio && !component_is_audio_only(&request.manifest, component) {
            result
                .reasons
                .push(SaturnPatchReadinessReason::MixedModeTargetAmbiguous);
            result
                .reasons
                .push(SaturnPatchReadinessReason::AudioImpactUnknown);
        } else if !has_audio {
            result.audio_expected_untouched = true;
        } else {
            result
                .reasons
                .push(SaturnPatchReadinessReason::AudioImpactUnknown);
        }
        result.topology_expected_untouched = inspection.target_size == inspection.source_size;
        if !result.topology_expected_untouched {
            result
                .reasons
                .push(SaturnPatchReadinessReason::TrackTopologyImpactUnknown);
        }
    }

    if request.manifest.system_id.as_ref().is_none() {
        result
            .reasons
            .push(SaturnPatchReadinessReason::NativeIdentityConflict);
    }
    if result.base_evidence == SaturnPatchBaseEvidence::None {
        result
            .reasons
            .push(SaturnPatchReadinessReason::PatchBaseUnknown);
    }
    result.changed_ranges = result
        .patch
        .as_ref()
        .and_then(|patch| patch.capabilities.changed_ranges.clone());
    if result
        .reasons
        .contains(&SaturnPatchReadinessReason::ProviderEvidenceConflict)
    {
        result.state = SaturnPatchReadinessState::Ambiguous;
        result.explanation = "Conflicting external claims were preserved; they do not become native Verified identity.".into();
    } else if result.reasons.is_empty()
        && result.base_evidence == SaturnPatchBaseEvidence::ExactManifestFingerprint
    {
        result.state = SaturnPatchReadinessState::ReadyToPreview;
        result.explanation = "The patch matches the exact Saturn manifest and its declared target is bounded for preview.".into();
    } else if result.reasons.is_empty()
        && result.base_evidence == SaturnPatchBaseEvidence::ExactTargetHash
    {
        result.state = SaturnPatchReadinessState::ReadyToPreview;
        result.explanation = "The patch matches the exact target hash. Audio tracks and CUE layout are outside the patch target.".into();
    } else if result.reasons.is_empty() {
        result.state = SaturnPatchReadinessState::PossiblyReady;
        result.explanation = "The patch has useful source evidence, but exact Saturn base semantics remain incomplete.".into();
    } else {
        result.state = SaturnPatchReadinessState::NotReady;
        result.explanation =
            "The patch cannot be considered safely applicable to this Saturn source.".into();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "emuwiz-saturn-readiness-{}-{name}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn varint(mut value: u64) -> Vec<u8> {
        let mut output = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                byte |= 0x80;
                output.push(byte);
                break;
            }
            output.push(byte);
            value -= 1;
        }
        output
    }

    fn bps(size: usize) -> Vec<u8> {
        let mut patch = b"BPS1".to_vec();
        patch.extend(varint(size as u64));
        patch.extend(varint(size as u64));
        patch.push(0x80);
        patch.extend(varint(((size as u64 - 1) << 2) | 0));
        patch.extend([0_u8; 8]);
        let checksum = crc32(&patch);
        patch.extend(checksum.to_le_bytes());
        patch
    }

    fn raw_sha256(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn manifest(root: &Path, with_audio: bool) -> SaturnDiscManifest {
        let data = root.join("data.bin");
        let mut data_bytes = vec![1_u8; 2048];
        data_bytes[..16].copy_from_slice(b"SEGA SEGASATURN ");
        fs::write(&data, &data_bytes).unwrap();
        let mut cue =
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n".to_string();
        if with_audio {
            let audio = root.join("audio.bin");
            fs::write(&audio, vec![2_u8; 2352]).unwrap();
            cue.push_str("FILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n");
        }
        let cue_path = root.join("disc.cue");
        fs::write(&cue_path, cue).unwrap();
        crate::saturn_disc_manifest::inspect_saturn_disc(&cue_path).unwrap()
    }

    fn request(
        root: &Path,
        target: SaturnPatchTarget,
        patch: &Path,
    ) -> SaturnPatchReadinessRequest {
        let manifest = manifest(root, false);
        SaturnPatchReadinessRequest {
            manifest,
            expected_manifest_fingerprint: None,
            patch_path: patch.to_path_buf(),
            target,
            provenance: SaturnPatchProvenance::default(),
            expected_output: Some(SaturnExpectedOutput {
                manifest_fingerprint: None,
                target_sha256: Some("known-output".into()),
                target_size: Some(2048),
            }),
            external_evidence: Vec::new(),
        }
    }

    #[test]
    fn exact_data_track_match_is_previewable_and_preserves_audio_boundary() {
        let root = temp_path("exact");
        fs::create_dir_all(&root).unwrap();
        let patch = root.join("patch.bps");
        let mut source = vec![1_u8; 2048];
        source[..16].copy_from_slice(b"SEGA SEGASATURN ");
        fs::write(&patch, bps(source.len())).unwrap();
        let target = SaturnPatchTarget::LogicalDataTrack {
            track_number: 1,
            sha256: raw_sha256(&source),
        };
        let mut request = request(&root, target, &patch);
        request.expected_manifest_fingerprint =
            Some(saturn_manifest_fingerprint(&request.manifest));
        let before_source = fs::read(root.join("data.bin")).unwrap();
        let before_patch = fs::read(&patch).unwrap();
        let result = assess_saturn_patch_readiness(&request);
        assert_eq!(result.state, SaturnPatchReadinessState::ReadyToPreview);
        assert_eq!(
            result.base_evidence,
            SaturnPatchBaseEvidence::ExactManifestFingerprint
        );
        assert!(result.audio_expected_untouched && result.topology_expected_untouched);
        assert!(!result.native_verified);
        assert_eq!(fs::read(root.join("data.bin")).unwrap(), before_source);
        assert_eq!(fs::read(&patch).unwrap(), before_patch);
        assert_eq!(result, assess_saturn_patch_readiness(&request));
    }

    #[test]
    fn wrong_source_and_unknown_target_fail_closed() {
        let root = temp_path("wrong");
        fs::create_dir_all(&root).unwrap();
        let patch = root.join("patch.bps");
        fs::write(&patch, bps(2048)).unwrap();
        let result =
            assess_saturn_patch_readiness(&request(&root, SaturnPatchTarget::Unknown, &patch));
        assert_eq!(result.state, SaturnPatchReadinessState::NotReady);
        assert!(
            result
                .reasons
                .contains(&SaturnPatchReadinessReason::PatchTargetUnknown)
        );
    }

    #[test]
    fn opaque_ssp_is_hashable_but_unsupported() {
        let root = temp_path("ssp");
        fs::create_dir_all(&root).unwrap();
        let patch = root.join("translation.ssp");
        fs::write(&patch, b"opaque").unwrap();
        let result =
            assess_saturn_patch_readiness(&request(&root, SaturnPatchTarget::Unknown, &patch));
        assert_eq!(result.state, SaturnPatchReadinessState::Unsupported);
        assert!(result.patch.is_some());
        assert!(
            result
                .reasons
                .contains(&SaturnPatchReadinessReason::OpaqueSspPackage)
        );
    }

    #[test]
    fn provider_conflict_is_preserved_without_native_verified() {
        let root = temp_path("conflict");
        fs::create_dir_all(&root).unwrap();
        let patch = root.join("patch.bps");
        let mut source = vec![1_u8; 2048];
        source[..16].copy_from_slice(b"SEGA SEGASATURN ");
        fs::write(&patch, bps(source.len())).unwrap();
        let mut request = request(
            &root,
            SaturnPatchTarget::LogicalDataTrack {
                track_number: 1,
                sha256: raw_sha256(&source),
            },
            &patch,
        );
        request.external_evidence = vec![
            SaturnExternalEvidence {
                provider: "HackHash".into(),
                claim: "A".into(),
                snapshot_sha256: Some("one".into()),
            },
            SaturnExternalEvidence {
                provider: "Other".into(),
                claim: "B".into(),
                snapshot_sha256: Some("two".into()),
            },
        ];
        let result = assess_saturn_patch_readiness(&request);
        assert_eq!(result.state, SaturnPatchReadinessState::Ambiguous);
        assert!(!result.native_verified);
    }

    #[test]
    fn mixed_mode_unknown_target_refuses_without_inventing_semantics() {
        let root = temp_path("mixed");
        fs::create_dir_all(&root).unwrap();
        let manifest = manifest(&root, true);
        let patch = root.join("patch.ips");
        fs::write(&patch, b"PATCHEOF").unwrap();
        let request = SaturnPatchReadinessRequest {
            manifest,
            expected_manifest_fingerprint: None,
            patch_path: patch,
            target: SaturnPatchTarget::Unknown,
            provenance: SaturnPatchProvenance::default(),
            expected_output: None,
            external_evidence: Vec::new(),
        };
        let result = assess_saturn_patch_readiness(&request);
        assert!(
            result
                .reasons
                .contains(&SaturnPatchReadinessReason::PatchTargetUnknown)
        );
    }

    #[test]
    fn wrong_target_hash_and_missing_output_are_explicit() {
        let root = temp_path("wrong-hash");
        fs::create_dir_all(&root).unwrap();
        let patch = root.join("patch.ips");
        fs::write(&patch, b"PATCHEOF").unwrap();
        let result = assess_saturn_patch_readiness(&request(
            &root,
            SaturnPatchTarget::LogicalDataTrack {
                track_number: 1,
                sha256: "wrong".into(),
            },
            &patch,
        ));
        assert_eq!(result.state, SaturnPatchReadinessState::NotReady);
        assert!(
            result
                .reasons
                .contains(&SaturnPatchReadinessReason::SourceHashMismatch)
        );
    }

    #[test]
    fn malformed_and_unknown_formats_are_not_ready_or_supported() {
        let root = temp_path("formats");
        fs::create_dir_all(&root).unwrap();
        let malformed = root.join("bad.bps");
        fs::write(&malformed, b"BPS1").unwrap();
        let malformed_result =
            assess_saturn_patch_readiness(&request(&root, SaturnPatchTarget::Unknown, &malformed));
        assert!(
            malformed_result
                .reasons
                .contains(&SaturnPatchReadinessReason::MalformedPatch)
        );
        let unknown = root.join("patch.bin");
        fs::write(&unknown, b"not-a-patch").unwrap();
        let unknown_result =
            assess_saturn_patch_readiness(&request(&root, SaturnPatchTarget::Unknown, &unknown));
        assert_eq!(unknown_result.state, SaturnPatchReadinessState::Unsupported);
        assert!(
            unknown_result
                .reasons
                .contains(&SaturnPatchReadinessReason::PatchFormatUnsupported)
        );
    }

    #[test]
    fn audio_component_target_refuses_audio_impact() {
        let root = temp_path("audio-impact");
        fs::create_dir_all(&root).unwrap();
        let manifest = manifest(&root, true);
        let component = manifest.components[1].clone();
        let patch = root.join("patch.ips");
        fs::write(&patch, b"PATCHEOF").unwrap();
        let result = assess_saturn_patch_readiness(&SaturnPatchReadinessRequest {
            manifest,
            expected_manifest_fingerprint: None,
            patch_path: patch,
            target: SaturnPatchTarget::ComponentBin {
                component: component.path,
                sha256: component.sha256,
            },
            provenance: SaturnPatchProvenance::default(),
            expected_output: None,
            external_evidence: Vec::new(),
        });
        assert_eq!(result.state, SaturnPatchReadinessState::NotReady);
        assert!(
            result
                .reasons
                .contains(&SaturnPatchReadinessReason::AudioImpactUnknown)
        );
    }

    #[test]
    fn incomplete_manifest_is_refused_before_target_claims() {
        let root = temp_path("incomplete");
        fs::create_dir_all(&root).unwrap();
        let patch = root.join("patch.ips");
        fs::write(&patch, b"PATCHEOF").unwrap();
        let mut request = request(&root, SaturnPatchTarget::Unknown, &patch);
        request.manifest.status = crate::saturn_disc_manifest::SaturnManifestStatus::Incomplete;
        let result = assess_saturn_patch_readiness(&request);
        assert!(
            result
                .reasons
                .contains(&SaturnPatchReadinessReason::IncompleteManifest)
        );
    }
}
