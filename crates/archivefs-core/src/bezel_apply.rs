//! Typed, fail-closed bezel apply planning.
//!
//! The current RetroArch environment abstraction is intentionally read-only.
//! This module therefore seals all facts needed for a future apply adapter,
//! but refuses mutation until an emulator-specific config writer proves the
//! overlay binding, ownership, and rollback contract.

use crate::bezel_decorations::{
    DecorationAsset, DecorationProvenance, DecorationSource, LocalBezelImageInfo, ViewportMetadata,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const BEZEL_APPLY_SCHEMA_VERSION: u32 = 1;
pub const MAX_BEZEL_APPLY_SOURCE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BezelApplyRequest {
    pub source_asset: DecorationAsset,
    pub source_image: LocalBezelImageInfo,
    pub resolved_identity: String,
    pub platform: String,
    pub emulator: String,
    pub core: Option<String>,
    /// The exact profile/config paths observed during read-only discovery.
    pub config_path: Option<PathBuf>,
    pub overlay_root: Option<PathBuf>,
    /// Roots approved by the caller for any future destination/config write.
    pub approved_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BezelPlanSource {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub provenance: DecorationProvenance,
}

/// A future adapter copies the source into its owned destination. It never
/// moves or links a catalogue asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BezelAssetTransferStrategy {
    CopyReadOnlySource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BezelPlanTarget {
    pub emulator: String,
    pub core: Option<String>,
    pub resolved_identity: String,
    pub platform: String,
    pub config_path: Option<PathBuf>,
    pub destination_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BezelPlannedFile {
    pub path: PathBuf,
    pub purpose: BezelFilePurpose,
    pub exists: bool,
    pub existing_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BezelFilePurpose {
    OverlayAsset,
    EmulatorConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BezelConfigEntry {
    pub path: PathBuf,
    pub key: String,
    pub old_value: Option<String>,
    pub new_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BezelRollbackInfo {
    pub supported: bool,
    pub backup_paths: Vec<PathBuf>,
    pub exact_restore: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BezelApplyStatus {
    Ready,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BezelApplyPlan {
    pub schema_version: u32,
    pub plan_id: String,
    pub status: BezelApplyStatus,
    pub source: BezelPlanSource,
    pub transfer_strategy: BezelAssetTransferStrategy,
    pub target: BezelPlanTarget,
    pub viewport: Option<ViewportMetadata>,
    pub files: Vec<BezelPlannedFile>,
    pub config_entries: Vec<BezelConfigEntry>,
    pub conflicts: Vec<String>,
    pub warnings: Vec<String>,
    pub refusals: Vec<BezelPlanRefusal>,
    pub rollback: BezelRollbackInfo,
}

impl BezelApplyPlan {
    /// Check the source observation again before any future apply adapter
    /// starts. Adapters must perform the equivalent check for every planned
    /// destination and config observation too.
    pub fn source_is_stale(&self) -> bool {
        match fs::symlink_metadata(&self.source.path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                metadata.len() != self.source.size_bytes
                    || digest_file(&self.source.path)
                        .map(|digest| digest != self.source.sha256)
                        .unwrap_or(true)
            }
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BezelPlanRefusal {
    MissingSourceAsset,
    SourceSymlink,
    SourceOutsideApprovedRoot,
    SourceTooLarge,
    UnsupportedSource,
    InvalidIdentity,
    InvalidViewport,
    UnsupportedEmulator,
    MissingRetroArchConfigPath,
    MissingRetroArchOverlayRoot,
    DestinationOutsideApprovedRoot,
    RetroArchConfigWriterMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BezelPlanError {
    pub refusal: BezelPlanRefusal,
    pub path: Option<PathBuf>,
    pub detail: String,
}

impl std::fmt::Display for BezelPlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.detail)
    }
}

impl std::error::Error for BezelPlanError {}

/// Build a complete, sealed plan without writing any source, config, or
/// destination file. The plan is deliberately refused until RetroArch has a
/// reviewed config writer and per-game binding contract.
pub fn build_bezel_apply_plan(
    request: &BezelApplyRequest,
) -> Result<BezelApplyPlan, BezelPlanError> {
    let source_path = match &request.source_asset.source {
        DecorationSource::LocalPack { path } | DecorationSource::UserOverride { path } => {
            PathBuf::from(path)
        }
        DecorationSource::Provider { .. } => {
            return Err(error(
                BezelPlanRefusal::UnsupportedSource,
                None,
                "provider bezel assets are not local apply sources",
            ));
        }
    };
    let metadata = fs::symlink_metadata(&source_path).map_err(|_| {
        error(
            BezelPlanRefusal::MissingSourceAsset,
            Some(source_path.clone()),
            "source bezel asset is missing",
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            BezelPlanRefusal::SourceSymlink,
            Some(source_path.clone()),
            "source bezel asset must not be a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(error(
            BezelPlanRefusal::MissingSourceAsset,
            Some(source_path.clone()),
            "source bezel asset is not a regular file",
        ));
    }
    if metadata.len() > MAX_BEZEL_APPLY_SOURCE_BYTES {
        return Err(error(
            BezelPlanRefusal::SourceTooLarge,
            Some(source_path.clone()),
            "source bezel asset exceeds the bounded apply limit",
        ));
    }
    if !request
        .approved_roots
        .iter()
        .any(|root| path_is_within(root, &source_path))
    {
        return Err(error(
            BezelPlanRefusal::SourceOutsideApprovedRoot,
            Some(source_path.clone()),
            "source bezel asset is outside the approved local bezel roots",
        ));
    }
    if request.resolved_identity.trim().is_empty() || request.platform.trim().is_empty() {
        return Err(error(
            BezelPlanRefusal::InvalidIdentity,
            None,
            "resolved game identity and platform are required",
        ));
    }
    validate_viewport(
        &request.source_image,
        request.source_asset.viewport.as_ref(),
    )?;
    if !request.emulator.eq_ignore_ascii_case("retroarch") {
        return Err(error(
            BezelPlanRefusal::UnsupportedEmulator,
            None,
            "no bezel apply adapter is proven for this emulator",
        ));
    }
    let config_path = request.config_path.clone().ok_or_else(|| {
        error(
            BezelPlanRefusal::MissingRetroArchConfigPath,
            None,
            "RetroArch profile did not provide an exact config path",
        )
    })?;
    let overlay_root = request.overlay_root.clone().ok_or_else(|| {
        error(
            BezelPlanRefusal::MissingRetroArchOverlayRoot,
            None,
            "RetroArch profile did not provide an exact overlay directory",
        )
    })?;
    for path in [&config_path, &overlay_root] {
        if !path.is_absolute()
            || !request
                .approved_roots
                .iter()
                .any(|root| path_is_within(root, path))
        {
            return Err(error(
                BezelPlanRefusal::DestinationOutsideApprovedRoot,
                Some(path.clone()),
                "RetroArch destination is outside the approved config roots",
            ));
        }
    }
    let source_digest = digest_file(&source_path).map_err(|detail| {
        error(
            BezelPlanRefusal::MissingSourceAsset,
            Some(source_path.clone()),
            &detail,
        )
    })?;
    let source = BezelPlanSource {
        path: source_path,
        sha256: source_digest,
        size_bytes: metadata.len(),
        provenance: request.source_asset.provenance.clone(),
    };
    let mut plan = BezelApplyPlan {
        schema_version: BEZEL_APPLY_SCHEMA_VERSION,
        plan_id: String::new(),
        status: BezelApplyStatus::Refused,
        source,
        transfer_strategy: BezelAssetTransferStrategy::CopyReadOnlySource,
        target: BezelPlanTarget {
            emulator: request.emulator.clone(),
            core: request.core.clone(),
            resolved_identity: request.resolved_identity.clone(),
            platform: request.platform.clone(),
            config_path: Some(config_path),
            destination_root: Some(overlay_root),
        },
        viewport: request.source_asset.viewport.clone(),
        files: Vec::new(),
        config_entries: Vec::new(),
        conflicts: Vec::new(),
        warnings: vec!["No files were written. RetroArch bezel apply remains preview-only.".into()],
        refusals: vec![BezelPlanRefusal::RetroArchConfigWriterMissing],
        rollback: BezelRollbackInfo {
            supported: false,
            backup_paths: Vec::new(),
            exact_restore: false,
            reason: "RetroArch overlay config ownership and exact restore are not yet proven"
                .into(),
        },
    };
    plan.plan_id = plan_digest(&plan)?;
    Ok(plan)
}

fn validate_viewport(
    image: &LocalBezelImageInfo,
    viewport: Option<&ViewportMetadata>,
) -> Result<(), BezelPlanError> {
    let Some(viewport) = viewport else {
        return Err(error(
            BezelPlanRefusal::InvalidViewport,
            None,
            "a bounded viewport is required before apply planning",
        ));
    };
    if viewport.width == 0
        || viewport.height == 0
        || viewport
            .left
            .checked_add(viewport.width)
            .is_none_or(|right| right > image.width)
        || viewport
            .top
            .checked_add(viewport.height)
            .is_none_or(|bottom| bottom > image.height)
    {
        return Err(error(
            BezelPlanRefusal::InvalidViewport,
            None,
            "viewport must be non-empty and contained within the source image",
        ));
    }
    Ok(())
}

fn path_is_within(root: &Path, path: &Path) -> bool {
    root.is_absolute()
        && path.is_absolute()
        && !path
            .components()
            .any(|component| component == Component::ParentDir)
        && path.strip_prefix(root).is_ok()
}

fn digest_file(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("source asset could not be read: {error}"))?;
    if bytes.len() as u64 > MAX_BEZEL_APPLY_SOURCE_BYTES {
        return Err("source asset exceeds the bounded apply limit".into());
    }
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn plan_digest(plan: &BezelApplyPlan) -> Result<String, BezelPlanError> {
    let mut unsigned = plan.clone();
    unsigned.plan_id.clear();
    let bytes = serde_json::to_vec(&unsigned).map_err(|serialization_error| {
        error(
            BezelPlanRefusal::InvalidIdentity,
            None,
            &format!("bezel plan could not be sealed: {serialization_error}"),
        )
    })?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn error(refusal: BezelPlanRefusal, path: Option<PathBuf>, detail: &str) -> BezelPlanError {
    BezelPlanError {
        refusal,
        path,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bezel_decorations::{
        DecorationEvidence, DecorationProvenance, DecorationReadiness, DecorationScope,
    };
    use std::io::Write;

    fn request(root: &Path, source: &Path) -> BezelApplyRequest {
        BezelApplyRequest {
            source_asset: DecorationAsset {
                id: source.display().to_string(),
                scope: DecorationScope::Game,
                source: DecorationSource::LocalPack {
                    path: source.display().to_string(),
                },
                evidence: DecorationEvidence::VerifiedIdentity {
                    identity: "game-id".into(),
                },
                targets: Vec::new(),
                readiness: DecorationReadiness::Ready,
                provenance: DecorationProvenance {
                    provider: "local-test".into(),
                    reference: "test".into(),
                    retrieved_at_unix_seconds: None,
                },
                viewport: Some(ViewportMetadata {
                    left: 10,
                    top: 10,
                    width: 100,
                    height: 80,
                }),
            },
            source_image: LocalBezelImageInfo {
                width: 320,
                height: 240,
                format: "rgba".into(),
            },
            resolved_identity: "game-id".into(),
            platform: "SNES".into(),
            emulator: "retroarch".into(),
            core: Some("snes9x".into()),
            config_path: Some(root.join("retroarch.cfg")),
            overlay_root: Some(root.join("overlays")),
            approved_roots: vec![root.to_path_buf()],
        }
    }

    #[test]
    fn planner_refuses_retroarch_until_config_writer_exists_without_writing_source() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"safe-source").unwrap();
        let before = fs::read(&source).unwrap();
        let plan = build_bezel_apply_plan(&request(root.path(), &source)).unwrap();
        assert_eq!(plan.status, BezelApplyStatus::Refused);
        assert_eq!(
            plan.refusals,
            vec![BezelPlanRefusal::RetroArchConfigWriterMissing]
        );
        assert!(!plan.rollback.supported);
        assert!(!plan.plan_id.is_empty());
        assert_eq!(
            plan.transfer_strategy,
            BezelAssetTransferStrategy::CopyReadOnlySource
        );
        assert!(!plan.source_is_stale());
        assert_eq!(fs::read(source).unwrap(), before);
    }

    #[test]
    fn plan_detects_a_stale_source_before_apply() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"safe-source").unwrap();
        let plan = build_bezel_apply_plan(&request(root.path(), &source)).unwrap();
        fs::write(&source, b"changed-source").unwrap();
        assert!(plan.source_is_stale());
    }

    #[test]
    fn missing_source_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("missing.png");
        let error = build_bezel_apply_plan(&request(root.path(), &source)).unwrap_err();
        assert_eq!(error.refusal, BezelPlanRefusal::MissingSourceAsset);
    }

    #[test]
    fn bad_viewport_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"safe-source").unwrap();
        let mut request = request(root.path(), &source);
        request.source_asset.viewport = Some(ViewportMetadata {
            left: 300,
            top: 0,
            width: 100,
            height: 10,
        });
        let error = build_bezel_apply_plan(&request).unwrap_err();
        assert_eq!(error.refusal, BezelPlanRefusal::InvalidViewport);
    }

    #[test]
    fn traversal_destination_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        fs::write(&source, b"safe-source").unwrap();
        let mut request = request(root.path(), &source);
        request.overlay_root = Some(root.path().join("overlays/../outside"));
        let error = build_bezel_apply_plan(&request).unwrap_err();
        assert_eq!(
            error.refusal,
            BezelPlanRefusal::DestinationOutsideApprovedRoot
        );
    }

    #[test]
    fn source_symlink_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real.png");
        let link = root.path().join("link.png");
        fs::write(&real, b"safe-source").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let error = build_bezel_apply_plan(&request(root.path(), &link)).unwrap_err();
        assert_eq!(error.refusal, BezelPlanRefusal::SourceSymlink);
    }

    #[test]
    fn plan_records_existing_destination_as_a_conflict_when_future_adapter_populates_it() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("bezel.png");
        let mut file = fs::File::create(&source).unwrap();
        file.write_all(b"safe-source").unwrap();
        let plan = build_bezel_apply_plan(&request(root.path(), &source)).unwrap();
        assert!(plan.files.is_empty());
        assert!(plan.conflicts.is_empty());
        assert_eq!(
            plan.target.destination_root,
            Some(root.path().join("overlays"))
        );
    }
}
