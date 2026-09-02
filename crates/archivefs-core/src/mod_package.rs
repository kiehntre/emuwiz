//! Safe, local, non-cheat game-mod package inspection.
//!
//! This phase accepts a **directory** chosen by the caller; archive extraction,
//! downloads, executable installers, and applying a plan are deliberately out
//! of scope. A package contains a UTF-8 JSON manifest named
//! [`LOCAL_MOD_PACKAGE_MANIFEST`] at its root and any payload files it names.
//! The package and selected game root are read only. Symlinks, special files,
//! absolute paths, traversal components, excessive packages, and unrecognised
//! operations are blockers.
//!
//! The strict v1 manifest is:
//!
//! ```json
//! {
//!   "format_version": 1,
//!   "package_id": "example.translation",
//!   "title": "Example Translation",
//!   "version": "1.0.0",
//!   "supported_platform": "snes",
//!   "supported_game": {
//!     "identities": [{ "kind": "loose_rom_sha256", "value": "..." }],
//!     "region": "USA",
//!     "revision": "1"
//!   },
//!   "operations": [{
//!     "kind": "replace",
//!     "payload": "payload/translation.bin",
//!     "destination": "game.bin",
//!     "required_source_sha256": "...",
//!     "expected_result_sha256": "..."
//!   }],
//!   "provenance": { "source": "local user-selected package" }
//! }
//! ```
//!
//! Only `create`, `replace`, and explicitly declared `delete` operations can
//! become eligible in this initial foundation. `patch` is parsed solely to
//! provide a precise, fail-closed unsupported-format blocker; IPS, BPS, UPS,
//! xdelta, and PPF are not applied or interpreted here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Take};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::game_identity::{GameIdentityReport, IdentityKind, IdentityPlatform, IdentityStatus};
use crate::patch_manager::{
    PreviewAdapter, PreviewDestinationState, PreviewEligibility, PreviewMatchStrength,
    PreviewProposedAction, PreviewState, PreviewWarning, PreviewWarningKind, SharedPreviewEntry,
    SharedPreviewReport, SharedTransactionPlan, build_shared_transaction_plan,
    require_local_mod_package_verification,
};

pub const LOCAL_MOD_PACKAGE_MANIFEST: &str = "emuwiz.mod.json";
pub const LOCAL_MOD_PACKAGE_FORMAT_VERSION: u32 = 1;
pub const MAX_LOCAL_MOD_PACKAGE_CANDIDATES: usize = 32;
pub const MAX_LOCAL_MOD_PACKAGE_ENTRIES: usize = 256;
pub const MAX_LOCAL_MOD_PACKAGE_OPERATIONS: usize = 128;
pub const MAX_LOCAL_MOD_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_LOCAL_MOD_PACKAGE_MANIFEST_BYTES: u64 = 256 * 1024;
pub const MAX_LOCAL_MOD_PACKAGE_PATH_BYTES: usize = 1024;
const MAX_TEXT_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedGameForMod {
    /// Existing directory which bounds every proposed destination.
    pub game_root: PathBuf,
    /// The authoritative identity result for the selected game.
    pub identity: GameIdentityReport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalModPackageRequest {
    pub selected_game: SelectedGameForMod,
    /// Caller-selected package directory. It is never extracted or modified.
    pub package_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LocalModPackageCandidateInspection {
    pub plans: Vec<LocalModPackagePlan>,
    pub blockers: Vec<ModPlanIssue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCanonicalPlatform {
    PlayStation2,
    GameCube,
    Wii,
    MegaDrive,
    Snes,
    Xbox360,
}

impl ModCanonicalPlatform {
    fn identity_platform(self) -> IdentityPlatform {
        match self {
            Self::PlayStation2 => IdentityPlatform::PlayStation2,
            Self::GameCube => IdentityPlatform::GameCube,
            Self::Wii => IdentityPlatform::Wii,
            Self::MegaDrive => IdentityPlatform::MegaDrive,
            Self::Snes => IdentityPlatform::Snes,
            Self::Xbox360 => IdentityPlatform::Xbox360,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModIdentityKind {
    Ps2Serial,
    Pcsx2ExecutableCrc,
    DolphinGameId,
    LooseRomSha256,
    XexTitleId,
    XexMediaId,
}

impl ModIdentityKind {
    fn identity_kind(self) -> IdentityKind {
        match self {
            Self::Ps2Serial => IdentityKind::Ps2Serial,
            Self::Pcsx2ExecutableCrc => IdentityKind::Pcsx2ExecutableCrc,
            Self::DolphinGameId => IdentityKind::DolphinGameId,
            Self::LooseRomSha256 => IdentityKind::LooseRomSha256,
            Self::XexTitleId => IdentityKind::XexTitleId,
            Self::XexMediaId => IdentityKind::XexMediaId,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModIdentityRequirement {
    pub kind: ModIdentityKind,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSupportedGame {
    pub identities: Vec<ModIdentityRequirement>,
    pub region: Option<String>,
    pub revision: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModPackageProvenance {
    /// Human-readable provenance supplied by the package author. It is never
    /// fetched or otherwise acted upon in this local-only phase.
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalModPackageMetadata {
    pub package_id: String,
    pub title: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub supported_platform: ModCanonicalPlatform,
    pub supported_game: ModSupportedGame,
    pub provenance: ModPackageProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCompatibilityState {
    Compatible,
    Incompatible,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModCompatibilityResult {
    pub state: ModCompatibilityState,
    pub selected_platform: IdentityPlatform,
    pub package_platform: Option<ModCanonicalPlatform>,
    pub matching_identity: Option<ModIdentityRequirement>,
    pub region_matches: Option<bool>,
    pub revision_matches: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModOperationKind {
    CreateFile,
    ReplaceFile,
    PatchFile,
    DeleteFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPatchFormat {
    Ips,
    Bps,
    Ups,
    Xdelta,
    Ppf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposedFileState {
    Missing,
    ExistingRegularFile,
    ExistingDirectory,
    ExistingSpecialFile,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProposedModFileOperation {
    pub kind: ModOperationKind,
    pub payload_path: Option<PathBuf>,
    pub source_path: Option<PathBuf>,
    pub destination_path: PathBuf,
    pub destination_state: ProposedFileState,
    pub required_source_sha256: Option<String>,
    pub observed_source_sha256: Option<String>,
    pub expected_result_sha256: Option<String>,
    pub observed_payload_sha256: Option<String>,
    pub patch_format: Option<ModPatchFormat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPlanConflictKind {
    DestinationAlreadyExists,
    DestinationIsNotRegularFile,
    RequiredSourceMissing,
    DuplicateDestination,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModPlanConflict {
    pub kind: ModPlanConflictKind,
    pub destination_path: PathBuf,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPlanBlockerKind {
    CandidateLimitExceeded,
    PackagePathUnsafe,
    PackageMissing,
    PackageNotDirectory,
    PackageLimitExceeded,
    UnsafeSymlink,
    UnsafePackageEntry,
    ManifestMissing,
    ManifestMalformed,
    ManifestUnsupportedVersion,
    GameRootUnsafe,
    GameOutsideRoot,
    PlatformUnknown,
    PlatformMismatch,
    GameIdentityUnknown,
    GameIdentityConflicting,
    GameIdentityAmbiguous,
    GameIdentityMismatch,
    RegionUnavailable,
    RegionMismatch,
    RevisionUnavailable,
    RevisionMismatch,
    PayloadPathUnsafe,
    PayloadMissing,
    PayloadNotRegularFile,
    DestinationPathUnsafe,
    DestinationEscapesGameRoot,
    DuplicateDestination,
    SourceHashMismatch,
    ExpectedResultHashMismatch,
    UnsupportedPatchFormat,
    UnsupportedOperation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModPlanIssue {
    pub kind: ModPlanBlockerKind,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LocalModPackagePlan {
    pub selected_game: SelectedGameForModSummary,
    pub package_root: PathBuf,
    pub manifest_path: PathBuf,
    pub package: Option<LocalModPackageMetadata>,
    pub compatibility: ModCompatibilityResult,
    pub operations: Vec<ProposedModFileOperation>,
    pub conflicts: Vec<ModPlanConflict>,
    pub warnings: Vec<String>,
    pub blockers: Vec<ModPlanIssue>,
    /// True only when the separate transactional apply phase can consider
    /// this immutable inspection plan. It does not authorise writes.
    pub eligible_for_later_apply: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SelectedGameForModSummary {
    pub game_root: PathBuf,
    pub archive_path: PathBuf,
    pub platform: IdentityPlatform,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    format_version: u32,
    package_id: String,
    title: String,
    version: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    description: Option<String>,
    supported_platform: ModCanonicalPlatform,
    supported_game: RawSupportedGame,
    operations: Vec<RawOperation>,
    provenance: RawProvenance,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSupportedGame {
    identities: Vec<RawIdentityRequirement>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    revision: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIdentityRequirement {
    kind: ModIdentityKind,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProvenance {
    source: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RawOperationKind {
    Create,
    Replace,
    Patch,
    Delete,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOperation {
    kind: RawOperationKind,
    #[serde(default)]
    payload: Option<String>,
    destination: String,
    #[serde(default)]
    required_source_sha256: Option<String>,
    #[serde(default)]
    expected_result_sha256: Option<String>,
    #[serde(default)]
    patch_format: Option<ModPatchFormat>,
}

/// Inspect exactly one caller-supplied local package directory without writes.
pub fn inspect_local_mod_package(request: LocalModPackageRequest) -> LocalModPackagePlan {
    let manifest_path = request.package_root.join(LOCAL_MOD_PACKAGE_MANIFEST);
    let selected_game = SelectedGameForModSummary {
        game_root: request.selected_game.game_root.clone(),
        archive_path: request.selected_game.identity.archive_path.clone(),
        platform: request.selected_game.identity.platform,
    };
    let mut plan = LocalModPackagePlan {
        selected_game,
        package_root: request.package_root.clone(),
        manifest_path,
        package: None,
        compatibility: ModCompatibilityResult {
            state: ModCompatibilityState::Unknown,
            selected_platform: request.selected_game.identity.platform,
            package_platform: None,
            matching_identity: None,
            region_matches: None,
            revision_matches: None,
        },
        operations: Vec::new(),
        conflicts: Vec::new(),
        warnings: Vec::new(),
        blockers: Vec::new(),
        eligible_for_later_apply: false,
    };

    if !validate_root(&request.selected_game.game_root, true, &mut plan, false) {
        return finish(plan);
    }
    if !request
        .selected_game
        .identity
        .archive_path
        .starts_with(&request.selected_game.game_root)
    {
        block(
            &mut plan,
            ModPlanBlockerKind::GameOutsideRoot,
            "the authoritative selected-game path is not beneath the supplied game root",
        );
        return finish(plan);
    }
    if !validate_root(&request.package_root, false, &mut plan, false) {
        return finish(plan);
    }
    if !scan_package(&request.package_root, &mut plan) {
        return finish(plan);
    }
    let Some(raw) = read_manifest(&request.package_root, &mut plan) else {
        return finish(plan);
    };
    if raw.format_version != LOCAL_MOD_PACKAGE_FORMAT_VERSION {
        block(
            &mut plan,
            ModPlanBlockerKind::ManifestUnsupportedVersion,
            format!(
                "manifest format_version {} is unsupported (expected {})",
                raw.format_version, LOCAL_MOD_PACKAGE_FORMAT_VERSION
            ),
        );
        return finish(plan);
    }
    let Some(metadata) = validate_metadata(&raw, &mut plan) else {
        return finish(plan);
    };
    plan.compatibility.package_platform = Some(metadata.supported_platform);
    assess_compatibility(&request.selected_game.identity, &metadata, &mut plan);
    plan.package = Some(metadata);

    if raw.operations.len() > MAX_LOCAL_MOD_PACKAGE_OPERATIONS {
        block(
            &mut plan,
            ModPlanBlockerKind::PackageLimitExceeded,
            format!(
                "manifest declares {} operations; limit is {MAX_LOCAL_MOD_PACKAGE_OPERATIONS}",
                raw.operations.len()
            ),
        );
        return finish(plan);
    }
    for operation in raw.operations {
        inspect_operation(&request, operation, &mut plan);
    }
    detect_duplicate_destinations(&mut plan);
    plan.operations.sort_by(|left, right| {
        left.destination_path
            .cmp(&right.destination_path)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    plan.conflicts.sort_by(|left, right| {
        left.destination_path
            .cmp(&right.destination_path)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    finish(plan)
}

/// Bounded convenience API for a caller presenting several explicitly supplied
/// local package candidates. It never searches the filesystem itself.
pub fn inspect_local_mod_package_candidates(
    selected_game: SelectedGameForMod,
    package_roots: &[PathBuf],
) -> LocalModPackageCandidateInspection {
    if package_roots.len() > MAX_LOCAL_MOD_PACKAGE_CANDIDATES {
        return LocalModPackageCandidateInspection {
            plans: Vec::new(),
            blockers: vec![ModPlanIssue {
                kind: ModPlanBlockerKind::CandidateLimitExceeded,
                detail: format!(
                    "{} caller-supplied package candidates exceed the {} limit",
                    package_roots.len(),
                    MAX_LOCAL_MOD_PACKAGE_CANDIDATES
                ),
            }],
        };
    }
    LocalModPackageCandidateInspection {
        plans: package_roots
            .iter()
            .cloned()
            .map(|package_root| {
                inspect_local_mod_package(LocalModPackageRequest {
                    selected_game: selected_game.clone(),
                    package_root,
                })
            })
            .collect(),
        blockers: Vec::new(),
    }
}

/// Convert a completed local-package inspection into the existing shared
/// transaction plan. Only create/replace operations with exact verified
/// package compatibility are admitted; delete and patch operations remain
/// explicitly unsupported so applying a package can never guess semantics.
pub fn build_local_mod_package_transaction_plan(
    inspected: &LocalModPackagePlan,
) -> Result<SharedTransactionPlan, crate::patch_manager::SharedApplyFailure> {
    if !inspected.eligible_for_later_apply
        || !matches!(
            inspected.compatibility.state,
            ModCompatibilityState::Compatible
        )
        || !inspected.blockers.is_empty()
        || !inspected.conflicts.is_empty()
    {
        return Err(crate::patch_manager::SharedApplyFailure {
            kind: crate::patch_manager::SharedApplyFailureKind::InvalidPlan,
            path: None,
            detail: "local mod package inspection is not unambiguously eligible".to_string(),
        });
    }
    let identity = inspected
        .compatibility
        .matching_identity
        .as_ref()
        .map(|value| value.value.clone())
        .ok_or_else(|| crate::patch_manager::SharedApplyFailure {
            kind: crate::patch_manager::SharedApplyFailureKind::InvalidPlan,
            path: None,
            detail: "local mod package has no exact verified identity".to_string(),
        })?;
    let mut entries = Vec::new();
    for operation in &inspected.operations {
        let (source, source_digest) = match (
            operation.source_path.as_ref(),
            operation.observed_payload_sha256.as_ref(),
        ) {
            (Some(source), Some(digest)) => (source.clone(), digest.clone()),
            _ => {
                return Err(crate::patch_manager::SharedApplyFailure {
                    kind: crate::patch_manager::SharedApplyFailureKind::InvalidPlan,
                    path: Some(crate::patch_manager::SharedTransactionPath::from_path(
                        &operation.destination_path,
                    )),
                    detail: "local mod operation has no verified payload".to_string(),
                });
            }
        };
        let relative = operation
            .destination_path
            .strip_prefix(&inspected.selected_game.game_root)
            .map_err(|_| crate::patch_manager::SharedApplyFailure {
                kind: crate::patch_manager::SharedApplyFailureKind::InvalidPlan,
                path: Some(crate::patch_manager::SharedTransactionPath::from_path(
                    &operation.destination_path,
                )),
                detail: "local mod destination is outside the game root".to_string(),
            })?
            .to_path_buf();
        let (destination_state, action) = match operation.destination_state {
            ProposedFileState::Missing => (
                PreviewDestinationState::Missing,
                PreviewProposedAction::Install,
            ),
            ProposedFileState::ExistingRegularFile => {
                let digest = operation.observed_source_sha256.as_deref();
                if digest == Some(source_digest.as_str()) {
                    (
                        PreviewDestinationState::RegularFileIdentical,
                        PreviewProposedAction::Skip,
                    )
                } else {
                    (
                        PreviewDestinationState::RegularFileDifferent,
                        PreviewProposedAction::Replace,
                    )
                }
            }
            ProposedFileState::ExistingDirectory => (
                PreviewDestinationState::Directory,
                PreviewProposedAction::Blocked,
            ),
            ProposedFileState::ExistingSpecialFile => (
                PreviewDestinationState::SpecialFile,
                PreviewProposedAction::Blocked,
            ),
            ProposedFileState::Unavailable => (
                PreviewDestinationState::Unavailable,
                PreviewProposedAction::Blocked,
            ),
        };
        if action == PreviewProposedAction::Blocked {
            return Err(crate::patch_manager::SharedApplyFailure {
                kind: crate::patch_manager::SharedApplyFailureKind::InvalidPlan,
                path: Some(crate::patch_manager::SharedTransactionPath::from_path(
                    &operation.destination_path,
                )),
                detail: "local mod destination is not safely actionable".to_string(),
            });
        }
        let parent = operation
            .destination_path
            .parent()
            .unwrap_or(inspected.selected_game.game_root.as_path());
        let mut warnings = Vec::new();
        if !parent.exists() {
            warnings.push(PreviewWarning {
                kind: PreviewWarningKind::DestinationParentsMissing,
                path: Some(parent.to_path_buf()),
            });
        }
        if action == PreviewProposedAction::Replace {
            warnings.push(PreviewWarning {
                kind: PreviewWarningKind::BackupWouldBeRequired,
                path: Some(operation.destination_path.clone()),
            });
        }
        entries.push(SharedPreviewEntry {
            adapter: PreviewAdapter::LocalModPackage,
            selected_archive: inspected.selected_game.archive_path.clone(),
            verified_identity: Some(identity.clone()),
            match_strength: PreviewMatchStrength::VerifiedExact,
            source_path: Some(source),
            source_digest: Some(source_digest),
            destination_root: inspected.selected_game.game_root.clone(),
            destination_relative_path: Some(relative),
            destination_path: Some(operation.destination_path.clone()),
            destination_state,
            existing_destination_digest: operation.observed_source_sha256.clone(),
            state: match action {
                PreviewProposedAction::Install => PreviewState::InstallNew,
                PreviewProposedAction::Skip => PreviewState::AlreadyInstalled,
                PreviewProposedAction::Replace => PreviewState::ReplaceDifferent,
                PreviewProposedAction::Blocked => PreviewState::Unsupported,
            },
            proposed_action: action,
            eligibility: PreviewEligibility::Eligible,
            blockers: Vec::new(),
            warnings,
            backup_required: action == PreviewProposedAction::Replace,
            explicit_replacement_permission_required: action == PreviewProposedAction::Replace,
        });
    }
    let report = SharedPreviewReport {
        request_archive: inspected.selected_game.archive_path.clone(),
        adapter: PreviewAdapter::LocalModPackage,
        entries,
        conflicts: Vec::new(),
        warnings: Vec::new(),
        summary: Default::default(),
        complete: true,
    };
    let mut transaction = build_shared_transaction_plan(
        &report,
        "local-game",
        "local_mod_package",
        &inspected.package_root,
    )?;
    require_local_mod_package_verification(&mut transaction)?;
    Ok(transaction)
}

fn finish(mut plan: LocalModPackagePlan) -> LocalModPackagePlan {
    plan.blockers.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.detail.cmp(&right.detail))
    });
    plan.warnings.sort();
    plan.warnings.dedup();
    if plan.blockers.is_empty() && plan.conflicts.is_empty() {
        plan.compatibility.state = ModCompatibilityState::Compatible;
        plan.eligible_for_later_apply = true;
    } else if plan.compatibility.state == ModCompatibilityState::Compatible {
        plan.compatibility.state = ModCompatibilityState::Incompatible;
    }
    plan
}

fn block(plan: &mut LocalModPackagePlan, kind: ModPlanBlockerKind, detail: impl Into<String>) {
    plan.blockers.push(ModPlanIssue {
        kind,
        detail: detail.into(),
    });
}

fn validate_root(
    root: &Path,
    is_game_root: bool,
    plan: &mut LocalModPackagePlan,
    _allow_missing: bool,
) -> bool {
    let unsafe_kind = if is_game_root {
        ModPlanBlockerKind::GameRootUnsafe
    } else {
        ModPlanBlockerKind::PackagePathUnsafe
    };
    if !root.is_absolute() || root.parent().is_none() || has_traversal_components(root) {
        block(
            plan,
            unsafe_kind,
            format!(
                "root must be an absolute non-root path without traversal: {}",
                root.display()
            ),
        );
        return false;
    }
    let mut current = PathBuf::new();
    for component in root.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                block(
                    plan,
                    ModPlanBlockerKind::UnsafeSymlink,
                    format!("refusing symlinked root component {}", current.display()),
                );
                return false;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                block(
                    plan,
                    if is_game_root {
                        ModPlanBlockerKind::GameRootUnsafe
                    } else {
                        ModPlanBlockerKind::PackageMissing
                    },
                    format!("root is unavailable: {}", root.display()),
                );
                return false;
            }
            Err(error) => {
                block(
                    plan,
                    unsafe_kind,
                    format!("cannot inspect root {}: {error}", root.display()),
                );
                return false;
            }
        }
    }
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => {
            block(
                plan,
                if is_game_root {
                    ModPlanBlockerKind::GameRootUnsafe
                } else {
                    ModPlanBlockerKind::PackageNotDirectory
                },
                format!("root is not a directory: {}", root.display()),
            );
            false
        }
        Err(error) => {
            block(
                plan,
                unsafe_kind,
                format!("cannot inspect root {}: {error}", root.display()),
            );
            false
        }
    }
}

fn scan_package(package_root: &Path, plan: &mut LocalModPackagePlan) -> bool {
    let mut pending = vec![package_root.to_path_buf()];
    let mut entries = 0_usize;
    let mut bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        let read_dir = match fs::read_dir(&directory) {
            Ok(read_dir) => read_dir,
            Err(error) => {
                block(
                    plan,
                    ModPlanBlockerKind::PackagePathUnsafe,
                    format!("cannot read {}: {error}", directory.display()),
                );
                return false;
            }
        };
        for item in read_dir {
            let item = match item {
                Ok(item) => item,
                Err(error) => {
                    block(
                        plan,
                        ModPlanBlockerKind::PackagePathUnsafe,
                        format!("cannot enumerate package: {error}"),
                    );
                    return false;
                }
            };
            let path = item.path();
            if path.to_string_lossy().len() > MAX_LOCAL_MOD_PACKAGE_PATH_BYTES {
                block(
                    plan,
                    ModPlanBlockerKind::PackageLimitExceeded,
                    "package entry path exceeds the path-length limit",
                );
                return false;
            }
            entries = entries.saturating_add(1);
            if entries > MAX_LOCAL_MOD_PACKAGE_ENTRIES {
                block(
                    plan,
                    ModPlanBlockerKind::PackageLimitExceeded,
                    "package entry count exceeds the limit",
                );
                return false;
            }
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    block(
                        plan,
                        ModPlanBlockerKind::PackagePathUnsafe,
                        format!("cannot inspect {}: {error}", path.display()),
                    );
                    return false;
                }
            };
            if metadata.file_type().is_symlink() {
                block(
                    plan,
                    ModPlanBlockerKind::UnsafeSymlink,
                    format!("package contains symlink {}", path.display()),
                );
                return false;
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                bytes = bytes.saturating_add(metadata.len());
                if bytes > MAX_LOCAL_MOD_PACKAGE_BYTES {
                    block(
                        plan,
                        ModPlanBlockerKind::PackageLimitExceeded,
                        "package byte size exceeds the limit",
                    );
                    return false;
                }
            } else {
                block(
                    plan,
                    ModPlanBlockerKind::UnsafePackageEntry,
                    format!("package contains non-regular entry {}", path.display()),
                );
                return false;
            }
        }
    }
    true
}

fn read_manifest(package_root: &Path, plan: &mut LocalModPackagePlan) -> Option<RawManifest> {
    let path = package_root.join(LOCAL_MOD_PACKAGE_MANIFEST);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        Ok(metadata) if metadata.file_type().is_symlink() => {
            block(
                plan,
                ModPlanBlockerKind::UnsafeSymlink,
                "manifest must not be a symlink",
            );
            return None;
        }
        Ok(_) => {
            block(
                plan,
                ModPlanBlockerKind::ManifestMalformed,
                "manifest is not a regular file",
            );
            return None;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            block(
                plan,
                ModPlanBlockerKind::ManifestMissing,
                "package has no emuwiz.mod.json manifest",
            );
            return None;
        }
        Err(error) => {
            block(
                plan,
                ModPlanBlockerKind::ManifestMalformed,
                format!("cannot inspect manifest: {error}"),
            );
            return None;
        }
    };
    if metadata.len() > MAX_LOCAL_MOD_PACKAGE_MANIFEST_BYTES {
        block(
            plan,
            ModPlanBlockerKind::PackageLimitExceeded,
            "manifest exceeds the byte limit",
        );
        return None;
    }
    let bytes = read_regular_file(
        &path,
        MAX_LOCAL_MOD_PACKAGE_MANIFEST_BYTES,
        plan,
        ModPlanBlockerKind::ManifestMalformed,
    )?;
    match serde_json::from_slice(&bytes) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            block(
                plan,
                ModPlanBlockerKind::ManifestMalformed,
                format!("manifest JSON is invalid: {error}"),
            );
            None
        }
    }
}

fn validate_metadata(
    raw: &RawManifest,
    plan: &mut LocalModPackagePlan,
) -> Option<LocalModPackageMetadata> {
    let required = [
        &raw.package_id,
        &raw.title,
        &raw.version,
        &raw.provenance.source,
    ];
    if required.iter().any(|value| !valid_text(value))
        || raw
            .author
            .as_deref()
            .is_some_and(|value| !valid_text(value))
        || raw
            .description
            .as_deref()
            .is_some_and(|value| !valid_text(value))
        || raw.supported_game.identities.is_empty()
        || raw.supported_game.identities.len() > MAX_LOCAL_MOD_PACKAGE_OPERATIONS
        || raw
            .supported_game
            .identities
            .iter()
            .any(|identity| !valid_text(&identity.value))
        || raw
            .supported_game
            .region
            .as_deref()
            .is_some_and(|value| !valid_text(value))
        || raw
            .supported_game
            .revision
            .as_deref()
            .is_some_and(|value| !valid_text(value))
    {
        block(
            plan,
            ModPlanBlockerKind::ManifestMalformed,
            "manifest metadata is missing, empty, or exceeds text limits",
        );
        return None;
    }
    let mut identities = BTreeSet::new();
    for identity in &raw.supported_game.identities {
        if !identities.insert((identity.kind, identity.value.clone())) {
            block(
                plan,
                ModPlanBlockerKind::ManifestMalformed,
                "manifest repeats a supported identity requirement",
            );
            return None;
        }
    }
    Some(LocalModPackageMetadata {
        package_id: raw.package_id.clone(),
        title: raw.title.clone(),
        version: raw.version.clone(),
        author: raw.author.clone(),
        description: raw.description.clone(),
        supported_platform: raw.supported_platform,
        supported_game: ModSupportedGame {
            identities: raw
                .supported_game
                .identities
                .iter()
                .map(|identity| ModIdentityRequirement {
                    kind: identity.kind,
                    value: identity.value.clone(),
                })
                .collect(),
            region: raw.supported_game.region.clone(),
            revision: raw.supported_game.revision.clone(),
        },
        provenance: ModPackageProvenance {
            source: raw.provenance.source.clone(),
        },
    })
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_TEXT_BYTES && !value.contains('\0')
}

fn assess_compatibility(
    identity: &GameIdentityReport,
    package: &LocalModPackageMetadata,
    plan: &mut LocalModPackagePlan,
) {
    if identity.platform == IdentityPlatform::Other {
        block(
            plan,
            ModPlanBlockerKind::PlatformUnknown,
            "selected game has no supported canonical platform",
        );
        return;
    }
    if identity.platform != package.supported_platform.identity_platform() {
        block(
            plan,
            ModPlanBlockerKind::PlatformMismatch,
            "package canonical platform does not match the selected game",
        );
        plan.compatibility.state = ModCompatibilityState::Incompatible;
        return;
    }
    let mut matched = None;
    let mut had_verified_value = false;
    for required in &package.supported_game.identities {
        let evidence: Vec<_> = identity
            .evidence
            .iter()
            .filter(|item| item.kind == required.kind.identity_kind())
            .collect();
        if evidence
            .iter()
            .any(|item| item.status == IdentityStatus::Ambiguous)
        {
            block(
                plan,
                ModPlanBlockerKind::GameIdentityAmbiguous,
                "selected game has ambiguous declared identity evidence",
            );
            return;
        }
        let verified: BTreeSet<_> = evidence
            .iter()
            .filter(|item| item.status == IdentityStatus::Verified)
            .filter_map(|item| item.value.as_deref())
            .collect();
        if verified.len() > 1 {
            block(
                plan,
                ModPlanBlockerKind::GameIdentityConflicting,
                "selected game has conflicting verified identity evidence",
            );
            return;
        }
        if let Some(value) = verified.first() {
            had_verified_value = true;
            if *value == required.value {
                matched = Some(required.clone());
                break;
            }
        }
    }
    let Some(matched) = matched else {
        block(
            plan,
            if had_verified_value {
                ModPlanBlockerKind::GameIdentityMismatch
            } else {
                ModPlanBlockerKind::GameIdentityUnknown
            },
            "selected game lacks an exact verified identity declared by the package",
        );
        return;
    };
    plan.compatibility.matching_identity = Some(matched);
    if let Some(required_region) = package.supported_game.region.as_deref() {
        match verified_identity_value(identity, IdentityKind::DolphinRegion) {
            Some(value) if value.eq_ignore_ascii_case(required_region) => {
                plan.compatibility.region_matches = Some(true)
            }
            Some(_) => {
                plan.compatibility.region_matches = Some(false);
                block(
                    plan,
                    ModPlanBlockerKind::RegionMismatch,
                    "package region does not match selected game evidence",
                );
            }
            None => block(
                plan,
                ModPlanBlockerKind::RegionUnavailable,
                "package requires a region but selected game has no verified region evidence",
            ),
        }
    }
    if let Some(required_revision) = package.supported_game.revision.as_deref() {
        match verified_identity_value(identity, IdentityKind::DolphinRevision) {
            Some(value) if value == required_revision => {
                plan.compatibility.revision_matches = Some(true)
            }
            Some(_) => {
                plan.compatibility.revision_matches = Some(false);
                block(
                    plan,
                    ModPlanBlockerKind::RevisionMismatch,
                    "package revision does not match selected game evidence",
                );
            }
            None => block(
                plan,
                ModPlanBlockerKind::RevisionUnavailable,
                "package requires a revision but selected game has no verified revision evidence",
            ),
        }
    }
    if plan.blockers.is_empty() {
        plan.compatibility.state = ModCompatibilityState::Compatible;
    } else {
        plan.compatibility.state = ModCompatibilityState::Incompatible;
    }
}

fn verified_identity_value(identity: &GameIdentityReport, kind: IdentityKind) -> Option<&str> {
    let values: BTreeSet<_> = identity
        .evidence
        .iter()
        .filter(|item| item.kind == kind && item.status == IdentityStatus::Verified)
        .filter_map(|item| item.value.as_deref())
        .collect();
    (values.len() == 1)
        .then(|| values.into_iter().next())
        .flatten()
}

fn inspect_operation(
    request: &LocalModPackageRequest,
    raw: RawOperation,
    plan: &mut LocalModPackagePlan,
) {
    let kind = match raw.kind {
        RawOperationKind::Create => ModOperationKind::CreateFile,
        RawOperationKind::Replace => ModOperationKind::ReplaceFile,
        RawOperationKind::Patch => ModOperationKind::PatchFile,
        RawOperationKind::Delete => ModOperationKind::DeleteFile,
    };
    let Some(destination_relative) = safe_relative_path(&raw.destination) else {
        block(
            plan,
            ModPlanBlockerKind::DestinationPathUnsafe,
            "operation destination must be a non-empty relative path without traversal",
        );
        return;
    };
    let destination_path = request.selected_game.game_root.join(&destination_relative);
    if !destination_path.starts_with(&request.selected_game.game_root) {
        block(
            plan,
            ModPlanBlockerKind::DestinationEscapesGameRoot,
            "operation destination escapes the selected game root",
        );
        return;
    }
    let destination_state =
        inspect_destination(&destination_path, &request.selected_game.game_root, plan);
    let needs_payload = !matches!(kind, ModOperationKind::DeleteFile);
    if needs_payload != raw.payload.is_some() {
        block(
            plan,
            ModPlanBlockerKind::ManifestMalformed,
            "payload is required for create/replace/patch and forbidden for delete",
        );
        return;
    }
    if matches!(kind, ModOperationKind::PatchFile) {
        block(
            plan,
            ModPlanBlockerKind::UnsupportedPatchFormat,
            format!(
                "patch operation {:?} is unsupported in the inspection-only foundation",
                raw.patch_format
            ),
        );
    } else if raw.patch_format.is_some() {
        block(
            plan,
            ModPlanBlockerKind::ManifestMalformed,
            "patch_format is valid only for patch operations",
        );
        return;
    }

    let mut operation = ProposedModFileOperation {
        kind,
        payload_path: raw.payload.as_deref().and_then(safe_relative_path),
        source_path: None,
        destination_path: destination_path.clone(),
        destination_state,
        required_source_sha256: raw.required_source_sha256.clone(),
        observed_source_sha256: None,
        expected_result_sha256: raw.expected_result_sha256.clone(),
        observed_payload_sha256: None,
        patch_format: raw.patch_format,
    };
    if raw.payload.is_some() && operation.payload_path.is_none() {
        block(
            plan,
            ModPlanBlockerKind::PayloadPathUnsafe,
            "operation payload must be a non-empty relative path without traversal",
        );
        return;
    }
    if let Some(payload_relative) = operation.payload_path.as_ref() {
        let payload_path = request.package_root.join(payload_relative);
        operation.source_path = Some(payload_path.clone());
        if let Some(digest) = hash_path(
            &payload_path,
            MAX_LOCAL_MOD_PACKAGE_BYTES,
            plan,
            ModPlanBlockerKind::PayloadMissing,
        ) {
            operation.observed_payload_sha256 = Some(digest.clone());
            if let Some(expected) = operation.expected_result_sha256.as_deref()
                && (!valid_sha256(expected) || !digest.eq_ignore_ascii_case(expected))
            {
                block(
                    plan,
                    ModPlanBlockerKind::ExpectedResultHashMismatch,
                    "payload hash does not match expected_result_sha256",
                );
            }
        }
    }
    match kind {
        ModOperationKind::CreateFile => match operation.destination_state {
            ProposedFileState::Missing => {}
            _ => plan.conflicts.push(ModPlanConflict {
                kind: ModPlanConflictKind::DestinationAlreadyExists,
                destination_path,
                detail: "create operation would overwrite an existing destination".to_string(),
            }),
        },
        ModOperationKind::ReplaceFile | ModOperationKind::PatchFile => {
            match operation.destination_state {
                ProposedFileState::ExistingRegularFile => {
                    if let Some(expected) = operation.required_source_sha256.as_deref() {
                        if !valid_sha256(expected) {
                            block(
                                plan,
                                ModPlanBlockerKind::ManifestMalformed,
                                "required_source_sha256 must be a SHA-256 hex digest",
                            );
                        } else if let Some(observed) = hash_path(
                            &destination_path,
                            MAX_LOCAL_MOD_PACKAGE_BYTES,
                            plan,
                            ModPlanBlockerKind::PayloadMissing,
                        ) {
                            operation.observed_source_sha256 = Some(observed.clone());
                            if !observed.eq_ignore_ascii_case(expected) {
                                block(
                                    plan,
                                    ModPlanBlockerKind::SourceHashMismatch,
                                    "destination source hash does not match required_source_sha256",
                                );
                            }
                        }
                    }
                }
                ProposedFileState::Missing => plan.conflicts.push(ModPlanConflict {
                    kind: ModPlanConflictKind::RequiredSourceMissing,
                    destination_path,
                    detail: "replace/patch operation requires an existing source file".to_string(),
                }),
                _ => plan.conflicts.push(ModPlanConflict {
                    kind: ModPlanConflictKind::DestinationIsNotRegularFile,
                    destination_path,
                    detail: "replace/patch destination is not a regular file".to_string(),
                }),
            }
        }
        ModOperationKind::DeleteFile => {
            block(
                plan,
                ModPlanBlockerKind::UnsupportedOperation,
                "delete operations are not supported by the reversible local-mod apply path",
            );
            if !matches!(
                operation.destination_state,
                ProposedFileState::ExistingRegularFile
            ) {
                plan.conflicts.push(ModPlanConflict {
                    kind: ModPlanConflictKind::RequiredSourceMissing,
                    destination_path,
                    detail: "delete operation requires an existing regular file".to_string(),
                });
            }
        }
    }
    plan.operations.push(operation);
}

fn safe_relative_path(raw: &str) -> Option<PathBuf> {
    if raw.is_empty()
        || raw.len() > MAX_LOCAL_MOD_PACKAGE_PATH_BYTES
        || raw.starts_with('/')
        || raw.starts_with('\\')
        || raw.as_bytes().get(1) == Some(&b':')
    {
        return None;
    }
    let path = Path::new(raw);
    if path.is_absolute() || has_unsafe_path_components(path) {
        return None;
    }
    let normalized: PathBuf = path.components().collect();
    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

fn has_unsafe_path_components(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

/// Traversal-only check for a path that is *expected* to be absolute (a
/// caller-supplied game or package root). Absoluteness is verified separately,
/// so a leading [`Component::RootDir`] is legitimate here and only
/// `..`/`.` components make the path unsafe.
fn has_traversal_components(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::Prefix(_)
        )
    })
}

fn inspect_destination(
    path: &Path,
    game_root: &Path,
    plan: &mut LocalModPackagePlan,
) -> ProposedFileState {
    let relative = match path.strip_prefix(game_root) {
        Ok(relative) => relative,
        Err(_) => {
            block(
                plan,
                ModPlanBlockerKind::DestinationEscapesGameRoot,
                "destination is outside selected game root",
            );
            return ProposedFileState::Unavailable;
        }
    };
    let mut current = game_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                block(
                    plan,
                    ModPlanBlockerKind::UnsafeSymlink,
                    format!(
                        "destination has unsafe symlink component {}",
                        current.display()
                    ),
                );
                return ProposedFileState::Unavailable;
            }
            Ok(metadata) if current == path && metadata.is_file() => {
                return ProposedFileState::ExistingRegularFile;
            }
            Ok(metadata) if current == path && metadata.is_dir() => {
                return ProposedFileState::ExistingDirectory;
            }
            Ok(_) if current == path => return ProposedFileState::ExistingSpecialFile,
            Ok(metadata) if !metadata.is_dir() => return ProposedFileState::Unavailable,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ProposedFileState::Missing;
            }
            Err(_) => return ProposedFileState::Unavailable,
        }
    }
    ProposedFileState::ExistingDirectory
}

fn hash_path(
    path: &Path,
    maximum: u64,
    plan: &mut LocalModPackagePlan,
    missing_kind: ModPlanBlockerKind,
) -> Option<String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            block(
                plan,
                ModPlanBlockerKind::UnsafeSymlink,
                format!("refusing symlink {}", path.display()),
            );
            return None;
        }
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => {
            block(
                plan,
                ModPlanBlockerKind::PayloadNotRegularFile,
                format!("not a regular file: {}", path.display()),
            );
            return None;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            block(
                plan,
                missing_kind,
                format!("missing required file: {}", path.display()),
            );
            return None;
        }
        Err(error) => {
            block(
                plan,
                ModPlanBlockerKind::PayloadNotRegularFile,
                format!("cannot inspect {}: {error}", path.display()),
            );
            return None;
        }
    };
    if metadata.len() > maximum {
        block(
            plan,
            ModPlanBlockerKind::PackageLimitExceeded,
            format!("file exceeds byte limit: {}", path.display()),
        );
        return None;
    }
    let bytes = read_regular_file(
        path,
        maximum,
        plan,
        ModPlanBlockerKind::PayloadNotRegularFile,
    )?;
    Some(sha256_hex(&bytes))
}

fn read_regular_file(
    path: &Path,
    maximum: u64,
    plan: &mut LocalModPackagePlan,
    error_kind: ModPlanBlockerKind,
) -> Option<Vec<u8>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        Ok(_) => {
            block(
                plan,
                error_kind,
                format!("not a safe regular file: {}", path.display()),
            );
            return None;
        }
        Err(error) => {
            block(
                plan,
                error_kind,
                format!("cannot inspect {}: {error}", path.display()),
            );
            return None;
        }
    };
    if before.len() > maximum {
        block(
            plan,
            ModPlanBlockerKind::PackageLimitExceeded,
            format!("file exceeds byte limit: {}", path.display()),
        );
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) => {
            block(
                plan,
                error_kind,
                format!("cannot read {}: {error}", path.display()),
            );
            return None;
        }
    };
    let mut limited: Take<File> = file.take(maximum.saturating_add(1));
    let mut bytes = Vec::with_capacity(before.len() as usize);
    if limited.read_to_end(&mut bytes).is_err() || bytes.len() as u64 > maximum {
        block(
            plan,
            error_kind,
            format!("cannot safely read {}", path.display()),
        );
        return None;
    }
    let after = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            block(
                plan,
                error_kind,
                format!("file changed during inspection: {}", path.display()),
            );
            return None;
        }
    };
    if after.file_type().is_symlink() || after.len() != before.len() {
        block(
            plan,
            error_kind,
            format!("file changed during inspection: {}", path.display()),
        );
        return None;
    }
    Some(bytes)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut result = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(result, "{byte:02x}");
    }
    result
}

fn detect_duplicate_destinations(plan: &mut LocalModPackagePlan) {
    let mut seen = BTreeMap::<PathBuf, usize>::new();
    for operation in &plan.operations {
        *seen.entry(operation.destination_path.clone()).or_default() += 1;
    }
    for (destination_path, count) in seen.into_iter().filter(|(_, count)| *count > 1) {
        plan.conflicts.push(ModPlanConflict {
            kind: ModPlanConflictKind::DuplicateDestination,
            destination_path: destination_path.clone(),
            detail: format!("{count} package operations declare the same destination"),
        });
        block(
            plan,
            ModPlanBlockerKind::DuplicateDestination,
            "package contains duplicate operation destinations",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;
    use crate::game_identity::{
        IdentityConfidence, IdentityEvidence, IdentityImageFormat, IdentityProvenance,
    };

    fn report(
        path: PathBuf,
        platform: IdentityPlatform,
        evidence: Vec<(IdentityKind, IdentityStatus, &str)>,
    ) -> GameIdentityReport {
        GameIdentityReport {
            archive_path: path,
            platform,
            format: IdentityImageFormat::LooseCartridgeRom,
            evidence: evidence
                .into_iter()
                .map(|(kind, status, value)| IdentityEvidence {
                    kind,
                    status,
                    value: Some(value.to_string()),
                    confidence: IdentityConfidence::ExactBytes,
                    provenance: IdentityProvenance {
                        archive_path: PathBuf::from("/fixture/game.bin"),
                        member_path: None,
                        member_index: None,
                        method: "fixture".to_string(),
                    },
                    diagnostic: String::new(),
                })
                .collect(),
            warnings: Vec::new(),
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        }
    }

    fn setup() -> (TempDir, LocalModPackageRequest, PathBuf) {
        let temp = TempDir::new().unwrap();
        let game_root = temp.path().join("game root");
        fs::create_dir(&game_root).unwrap();
        let game = game_root.join("game.bin");
        fs::write(&game, b"original").unwrap();
        let package_root = temp.path().join("local mod");
        fs::create_dir(&package_root).unwrap();
        (
            temp,
            LocalModPackageRequest {
                selected_game: SelectedGameForMod {
                    game_root,
                    identity: report(
                        game,
                        IdentityPlatform::Snes,
                        vec![(
                            IdentityKind::LooseRomSha256,
                            IdentityStatus::Verified,
                            "game-sha",
                        )],
                    ),
                },
                package_root: package_root.clone(),
            },
            package_root,
        )
    }

    fn manifest(operation: &str, extra_game: &str) -> String {
        format!(
            r#"{{"format_version":1,"package_id":"example.mod","title":"Example Mod","version":"1.0","supported_platform":"snes","supported_game":{{"identities":[{{"kind":"loose_rom_sha256","value":"game-sha"}}]{extra_game}}},"operations":[{operation}],"provenance":{{"source":"local test"}}}}"#
        )
    }

    fn write_package(root: &Path, manifest: &str, payload: Option<(&str, &[u8])>) {
        fs::write(root.join(LOCAL_MOD_PACKAGE_MANIFEST), manifest).unwrap();
        if let Some((path, bytes)) = payload {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    fn valid_replace() -> (String, Vec<u8>) {
        let payload = b"replacement".to_vec();
        let hash = sha256_hex(&payload);
        (
            manifest(
                &format!(
                    r#"{{"kind":"replace","payload":"payload/new.bin","destination":"game.bin","required_source_sha256":"{}","expected_result_sha256":"{}"}}"#,
                    sha256_hex(b"original"),
                    hash
                ),
                "",
            ),
            payload,
        )
    }

    fn has_blocker(plan: &LocalModPackagePlan, kind: ModPlanBlockerKind) -> bool {
        plan.blockers.iter().any(|blocker| blocker.kind == kind)
    }

    #[test]
    fn mod_package_valid_compatible_local_package_is_eligible() {
        let (_temp, request, root) = setup();
        let (manifest, payload) = valid_replace();
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        let plan = inspect_local_mod_package(request);
        assert!(plan.eligible_for_later_apply);
        assert_eq!(plan.compatibility.state, ModCompatibilityState::Compatible);
        assert_eq!(plan.operations.len(), 1);
    }

    #[test]
    fn local_mod_package_builds_shared_reversible_transaction_plan() {
        let (_temp, request, root) = setup();
        let (manifest, payload) = valid_replace();
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        let plan = inspect_local_mod_package(request);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();
        assert_eq!(transaction.entries.len(), 1);
        assert_eq!(transaction.context.source_mode, "local_mod_package");
        assert!(matches!(
            transaction.entries[0].content_verification,
            Some(crate::patch_manager::SharedContentVerification::LocalModPackage)
        ));
        assert_eq!(
            transaction.entries[0]
                .destination_relative_path
                .to_path_buf()
                .unwrap(),
            PathBuf::from("game.bin")
        );
    }

    #[test]
    fn mod_package_rejects_wrong_platform_and_identity() {
        let (_temp, mut request, root) = setup();
        let (manifest, payload) = valid_replace();
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        request.selected_game.identity.platform = IdentityPlatform::Wii;
        let plan = inspect_local_mod_package(request);
        assert!(has_blocker(&plan, ModPlanBlockerKind::PlatformMismatch));

        let (_temp, request, root) = setup();
        let (manifest, payload) = valid_replace();
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        let mut request = request;
        request.selected_game.identity.evidence[0].value = Some("other-sha".to_string());
        let plan = inspect_local_mod_package(request);
        assert!(has_blocker(&plan, ModPlanBlockerKind::GameIdentityMismatch));
    }

    #[test]
    fn mod_package_rejects_wrong_region_revision_and_ambiguous_identity() {
        let (_temp, mut request, root) = setup();
        request.selected_game.identity.platform = IdentityPlatform::GameCube;
        request.selected_game.identity.evidence = vec![
            request.selected_game.identity.evidence[0].clone(),
            IdentityEvidence {
                kind: IdentityKind::DolphinRegion,
                status: IdentityStatus::Verified,
                value: Some("EUR".to_string()),
                confidence: IdentityConfidence::ExactBytes,
                provenance: request.selected_game.identity.evidence[0]
                    .provenance
                    .clone(),
                diagnostic: String::new(),
            },
            IdentityEvidence {
                kind: IdentityKind::DolphinRevision,
                status: IdentityStatus::Verified,
                value: Some("2".to_string()),
                confidence: IdentityConfidence::ExactBytes,
                provenance: request.selected_game.identity.evidence[0]
                    .provenance
                    .clone(),
                diagnostic: String::new(),
            },
        ];
        let (manifest, payload) = valid_replace();
        let manifest = manifest.replace("\"snes\"", "\"game_cube\"").replace("\"identities\":[{\"kind\":\"loose_rom_sha256\",\"value\":\"game-sha\"}]", "\"identities\":[{\"kind\":\"loose_rom_sha256\",\"value\":\"game-sha\"}],\"region\":\"USA\",\"revision\":\"1\"");
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        let plan = inspect_local_mod_package(request);
        assert!(has_blocker(&plan, ModPlanBlockerKind::RegionMismatch));
        assert!(has_blocker(&plan, ModPlanBlockerKind::RevisionMismatch));

        let (_temp, mut request, root) = setup();
        request.selected_game.identity.evidence[0].status = IdentityStatus::Ambiguous;
        let (manifest, payload) = valid_replace();
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        assert!(has_blocker(
            &inspect_local_mod_package(request),
            ModPlanBlockerKind::GameIdentityAmbiguous
        ));
    }

    #[test]
    fn mod_package_rejects_missing_and_malformed_manifest() {
        let (_temp, request, _root) = setup();
        assert!(has_blocker(
            &inspect_local_mod_package(request),
            ModPlanBlockerKind::ManifestMissing
        ));
        let (_temp, request, root) = setup();
        write_package(&root, "{not json", None);
        assert!(has_blocker(
            &inspect_local_mod_package(request),
            ModPlanBlockerKind::ManifestMalformed
        ));
    }

    #[test]
    fn mod_package_rejects_missing_payload_and_unsafe_payload_paths() {
        let (_temp, request, root) = setup();
        let (manifest, _) = valid_replace();
        write_package(&root, &manifest, None);
        assert!(has_blocker(
            &inspect_local_mod_package(request),
            ModPlanBlockerKind::PayloadMissing
        ));

        for destination in ["/absolute.bin", "../escape.bin"] {
            let (_temp, request, root) = setup();
            let (manifest, payload) = valid_replace();
            let manifest = manifest.replace("\"game.bin\"", &format!("\"{destination}\""));
            write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
            assert!(has_blocker(
                &inspect_local_mod_package(request),
                ModPlanBlockerKind::DestinationPathUnsafe
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn mod_package_rejects_unsafe_symlink() {
        use std::os::unix::fs::symlink;
        let (_temp, request, root) = setup();
        let (manifest, _) = valid_replace();
        write_package(&root, &manifest, None);
        symlink("/tmp", root.join("payload")).unwrap();
        assert!(has_blocker(
            &inspect_local_mod_package(request),
            ModPlanBlockerKind::UnsafeSymlink
        ));
    }

    #[test]
    fn mod_package_rejects_duplicate_destinations_and_source_hash_mismatch() {
        let (_temp, request, root) = setup();
        let (operation, payload) = valid_replace();
        let duplicate = operation.replace("] ,", "],");
        let operation_json = duplicate
            .split("\"operations\":[")
            .nth(1)
            .unwrap()
            .split("],\"provenance")
            .next()
            .unwrap();
        let manifest = manifest(&format!("{operation_json},{operation_json}"), "");
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        assert!(has_blocker(
            &inspect_local_mod_package(request),
            ModPlanBlockerKind::DuplicateDestination
        ));

        let (_temp, request, root) = setup();
        let (manifest, payload) = valid_replace();
        let manifest = manifest.replace(&sha256_hex(b"original"), &"0".repeat(64));
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        assert!(has_blocker(
            &inspect_local_mod_package(request),
            ModPlanBlockerKind::SourceHashMismatch
        ));
    }

    #[test]
    fn mod_package_rejects_unsupported_patch_and_enforces_limits() {
        let (_temp, request, root) = setup();
        let patch = r#"{"kind":"patch","payload":"payload/p.ips","destination":"game.bin","patch_format":"ips"}"#;
        write_package(
            &root,
            &manifest(patch, ""),
            Some(("payload/p.ips", b"patch")),
        );
        assert!(has_blocker(
            &inspect_local_mod_package(request),
            ModPlanBlockerKind::UnsupportedPatchFormat
        ));

        let (_temp, request, root) = setup();
        let roots = vec![root; MAX_LOCAL_MOD_PACKAGE_CANDIDATES + 1];
        let candidates = inspect_local_mod_package_candidates(request.selected_game, &roots);
        assert_eq!(
            candidates.blockers[0].kind,
            ModPlanBlockerKind::CandidateLimitExceeded
        );

        let (_temp, request, root) = setup();
        let (manifest, payload) = valid_replace();
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        for index in 0..MAX_LOCAL_MOD_PACKAGE_ENTRIES {
            fs::write(root.join(format!("extra-{index}")), b"x").unwrap();
        }
        assert!(has_blocker(
            &inspect_local_mod_package(request),
            ModPlanBlockerKind::PackageLimitExceeded
        ));
    }

    #[test]
    fn mod_package_paths_with_unicode_spaces_and_shell_text_are_safe_and_ordered() {
        let (_temp, request, root) = setup();
        let payload = b"new";
        let expected = sha256_hex(payload);
        let operations = format!(
            r#"{{"kind":"create","payload":"payload/é $().bin","destination":"z $()/é.bin","expected_result_sha256":"{expected}"}},{{"kind":"create","payload":"payload/a.bin","destination":"a space.bin","expected_result_sha256":"{expected}"}}"#
        );
        write_package(
            &root,
            &manifest(&operations, ""),
            Some(("payload/é $().bin", payload)),
        );
        fs::write(root.join("payload/a.bin"), payload).unwrap();
        let plan = inspect_local_mod_package(request);
        assert!(plan.eligible_for_later_apply);
        assert!(plan.operations[0].destination_path.ends_with("a space.bin"));
        assert!(plan.operations[1].destination_path.ends_with("z $()/é.bin"));
    }

    #[test]
    fn mod_package_preview_performs_no_writes() {
        let (_temp, request, root) = setup();
        let game = request.selected_game.game_root.join("game.bin");
        let (manifest, payload) = valid_replace();
        write_package(&root, &manifest, Some(("payload/new.bin", &payload)));
        let before = fs::read(&game).unwrap();
        let plan = inspect_local_mod_package(request);
        assert!(plan.eligible_for_later_apply);
        assert_eq!(fs::read(game).unwrap(), before);
        assert!(!root.join("applied").exists());
    }

    // -----------------------------------------------------------------
    // Execution-level coverage for the generic local-mod seam: the whole
    // LocalModPackagePlan -> SharedTransactionPlan -> execute_shared_apply
    // -> shared rollback round trip, against genuinely nested targets,
    // using only the existing shared backup/journal/rollback machinery.
    // -----------------------------------------------------------------

    use crate::patch_manager::{
        SharedApplyConfirmation, SharedApplyOptions, SharedApplyStatus, SharedRollbackConfirmation,
        SharedRollbackOptions, discover_shared_apply_history, execute_shared_apply,
        execute_shared_rollback, preview_shared_rollback,
    };

    /// history_root + backup_root under a fresh temp dir, plus an apply
    /// options value that reuses the plan's own context - the same shape
    /// every other adapter's tests use.
    fn shared_roots(temp: &TempDir) -> (PathBuf, PathBuf) {
        (temp.path().join("history"), temp.path().join("backups"))
    }

    fn apply_options(
        plan: &SharedTransactionPlan,
        history_root: &Path,
        backup_root: &Path,
        replacement_approved: bool,
    ) -> SharedApplyOptions {
        SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: true,
                replacement_approved,
            }),
            operation_id: "local-mod-apply".into(),
            timestamp_unix_seconds: 1_700_000_000,
            current_context: plan.context.clone(),
            history_root: history_root.to_path_buf(),
            backup_root: backup_root.to_path_buf(),
        }
    }

    #[test]
    fn local_mod_nested_install_applies_and_rollback_restores_the_tree_exactly() {
        let (temp, request, package_root) = setup();
        let game_root = request.selected_game.game_root.clone();
        let (history_root, backup_root) = shared_roots(&temp);

        // A three-deep target whose parents do not exist yet.
        let payload = b"new-texture-bytes".to_vec();
        let manifest = manifest(
            &format!(
                r#"{{"kind":"create","payload":"payload/tex.bin","destination":"assets/textures/hi/tex.bin","expected_result_sha256":"{}"}}"#,
                sha256_hex(&payload)
            ),
            "",
        );
        write_package(
            &package_root,
            &manifest,
            Some(("payload/tex.bin", &payload)),
        );

        let inspected = inspect_local_mod_package(request);
        assert!(inspected.eligible_for_later_apply);
        assert_eq!(inspected.operations.len(), 1);
        assert_eq!(
            inspected.operations[0].destination_state,
            ProposedFileState::Missing
        );

        let plan = build_local_mod_package_transaction_plan(&inspected).unwrap();
        assert_eq!(plan.context.source_mode, "local_mod_package");
        assert_eq!(
            plan.context.adapter,
            crate::patch_manager::PreviewAdapter::LocalModPackage,
            "provenance must be the local mod package adapter, never a fabricated emulator adapter"
        );

        let target = game_root.join("assets/textures/hi/tex.bin");
        let applied = execute_shared_apply(
            &plan,
            &apply_options(&plan, &history_root, &backup_root, false),
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(&target).unwrap(), payload, "target bytes written");
        assert!(game_root.join("assets").is_dir());
        assert!(game_root.join("assets/textures").is_dir());
        assert!(game_root.join("assets/textures/hi").is_dir());

        let entry = &applied.journal.entries[0];
        assert_eq!(entry.destination_existed_before_apply, Some(false));
        assert_eq!(
            entry.created_directories.len(),
            3,
            "every created directory level is recorded for rollback, not just the immediate parent"
        );
        assert!(entry.backup_path.is_none(), "an install backs nothing up");

        // The journal is written into the *shared* history root, discoverable
        // by the normal shared-history reader.
        let journal_path = applied.journal_path.clone().unwrap();
        assert!(journal_path.starts_with(&history_root));
        assert_eq!(
            discover_shared_apply_history(&history_root).journals.len(),
            1
        );

        // Rollback through the existing shared API.
        let preview = preview_shared_rollback(&journal_path, &game_root, &backup_root);
        assert!(preview.available);
        let rolled_back = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "local-mod-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: history_root.clone(),
                backup_root: backup_root.clone(),
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);

        // Exact restoration: the file and every directory level this
        // transaction created are gone; the pre-existing game file remains.
        assert!(!target.exists());
        assert!(!game_root.join("assets/textures/hi").exists());
        assert!(!game_root.join("assets/textures").exists());
        assert!(!game_root.join("assets").exists());
        assert!(game_root.join("game.bin").is_file());
        assert!(game_root.is_dir());
    }

    #[test]
    fn local_mod_nested_replace_round_trip_restores_original_bytes_from_the_shared_backup() {
        let (temp, request, package_root) = setup();
        let game_root = request.selected_game.game_root.clone();
        let (history_root, backup_root) = shared_roots(&temp);

        // A nested existing file; its parent directories pre-exist.
        fs::create_dir_all(game_root.join("save/slots")).unwrap();
        let target = game_root.join("save/slots/data.bin");
        let original = b"ORIGINAL-SAVE-BYTES".to_vec();
        fs::write(&target, &original).unwrap();

        let modded = b"MODDED-SAVE-BYTES".to_vec();
        let manifest = manifest(
            &format!(
                r#"{{"kind":"replace","payload":"payload/data.bin","destination":"save/slots/data.bin","required_source_sha256":"{}","expected_result_sha256":"{}"}}"#,
                sha256_hex(&original),
                sha256_hex(&modded)
            ),
            "",
        );
        write_package(
            &package_root,
            &manifest,
            Some(("payload/data.bin", &modded)),
        );

        let inspected = inspect_local_mod_package(request);
        assert!(inspected.eligible_for_later_apply);
        assert_eq!(
            inspected.operations[0].destination_state,
            ProposedFileState::ExistingRegularFile
        );

        let plan = build_local_mod_package_transaction_plan(&inspected).unwrap();
        let applied = execute_shared_apply(
            &plan,
            &apply_options(&plan, &history_root, &backup_root, true),
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(&target).unwrap(), modded);

        // The original bytes are preserved in the normal shared backup store.
        let entry = &applied.journal.entries[0];
        let backup = entry
            .backup_path
            .as_ref()
            .expect("replace makes a shared backup")
            .to_path_buf()
            .unwrap();
        assert!(backup.starts_with(&backup_root));
        assert_eq!(fs::read(&backup).unwrap(), original);
        assert_eq!(entry.created_directories.len(), 0);

        let journal_path = applied.journal_path.clone().unwrap();
        let preview = preview_shared_rollback(&journal_path, &game_root, &backup_root);
        assert!(preview.available);
        let rolled_back = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "local-mod-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root,
                backup_root,
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);
        assert_eq!(
            fs::read(&target).unwrap(),
            original,
            "rollback restores the exact original bytes, not merely removes a new file"
        );
        assert!(game_root.join("save/slots").is_dir());
        assert!(game_root.join("save").is_dir());
    }

    #[test]
    fn local_mod_symlinked_mid_path_component_is_refused_and_writes_nothing() {
        #[cfg(unix)]
        {
            let (temp, request, package_root) = setup();
            let game_root = request.selected_game.game_root.clone();
            let (history_root, backup_root) = shared_roots(&temp);

            // `a/` is a real dir; `a/b` is a symlink pointing outside the game
            // root; the manifest targets `a/b/c/file.bin`.
            let outside = temp.path().join("outside");
            fs::create_dir_all(outside.join("c")).unwrap();
            fs::create_dir(game_root.join("a")).unwrap();
            std::os::unix::fs::symlink(&outside, game_root.join("a/b")).unwrap();
            let sentinel = outside.join("c/file.bin");
            fs::write(&sentinel, b"UNTOUCHED").unwrap();

            let payload = b"payload".to_vec();
            let manifest = manifest(
                &format!(
                    r#"{{"kind":"create","payload":"payload/f.bin","destination":"a/b/c/file.bin","expected_result_sha256":"{}"}}"#,
                    sha256_hex(&payload)
                ),
                "",
            );
            write_package(&package_root, &manifest, Some(("payload/f.bin", &payload)));

            // Inspection already refuses an unsafe symlink component.
            let inspected = inspect_local_mod_package(request);
            assert!(
                has_blocker(&inspected, ModPlanBlockerKind::UnsafeSymlink),
                "a symlinked mid-path component is an unsafe destination"
            );
            assert!(!inspected.eligible_for_later_apply);
            assert!(build_local_mod_package_transaction_plan(&inspected).is_err());

            // Even a hand-built plan that tries to smuggle the same nested
            // destination past inspection is refused by the shared apply
            // assessment, and nothing is written on either side of the link.
            let mut smuggled = build_shared_plan_for_symlink_case(&game_root, &package_root);
            crate::patch_manager::require_local_mod_package_verification(&mut smuggled).unwrap();
            let applied = execute_shared_apply(
                &smuggled,
                &apply_options(&smuggled, &history_root, &backup_root, false),
            );
            assert_ne!(
                applied.journal.status,
                SharedApplyStatus::Success,
                "the shared apply must refuse a symlinked mid-path component"
            );
            assert!(
                applied.journal.entries[0]
                    .failures
                    .iter()
                    .any(|failure| matches!(
                        failure.kind,
                        crate::patch_manager::SharedApplyFailureKind::DestinationUnsafe
                    )),
                "refused specifically as an unsafe destination"
            );
            // The symlink itself is untouched and its target directory is
            // exactly as it was - nothing was written through the link.
            assert!(
                fs::symlink_metadata(game_root.join("a/b"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(!outside.join("c/file.bin.tmp").exists());
            let target_dir: Vec<_> = fs::read_dir(outside.join("c"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(target_dir, vec![std::ffi::OsString::from("file.bin")]);
            assert_eq!(
                fs::read(&sentinel).unwrap(),
                b"UNTOUCHED",
                "the symlink target is never touched"
            );
        }
    }

    #[cfg(unix)]
    fn build_shared_plan_for_symlink_case(
        game_root: &Path,
        package_root: &Path,
    ) -> SharedTransactionPlan {
        use crate::patch_manager::{
            PreviewAdapter, PreviewDestinationState, PreviewEligibility, PreviewMatchStrength,
            PreviewProposedAction, PreviewState, SharedPreviewEntry, SharedPreviewReport,
        };
        let payload = package_root.join("payload/f.bin");
        let report = SharedPreviewReport {
            request_archive: game_root.join("game.bin"),
            adapter: PreviewAdapter::LocalModPackage,
            entries: vec![SharedPreviewEntry {
                adapter: PreviewAdapter::LocalModPackage,
                selected_archive: game_root.join("game.bin"),
                verified_identity: Some("game-sha".into()),
                match_strength: PreviewMatchStrength::VerifiedExact,
                source_path: Some(payload.clone()),
                source_digest: Some(sha256_hex(b"payload")),
                destination_root: game_root.to_path_buf(),
                destination_relative_path: Some(PathBuf::from("a/b/c/file.bin")),
                destination_path: Some(game_root.join("a/b/c/file.bin")),
                destination_state: PreviewDestinationState::Missing,
                existing_destination_digest: None,
                state: PreviewState::InstallNew,
                proposed_action: PreviewProposedAction::Install,
                eligibility: PreviewEligibility::Eligible,
                blockers: Vec::new(),
                warnings: Vec::new(),
                backup_required: false,
                explicit_replacement_permission_required: false,
            }],
            conflicts: Vec::new(),
            warnings: Vec::new(),
            summary: Default::default(),
            complete: true,
        };
        build_shared_transaction_plan(&report, "local-game", "local_mod_package", package_root)
            .unwrap()
    }

    #[test]
    fn local_mod_delete_and_patch_operations_are_fail_closed_and_produce_no_apply_plan() {
        // Delete.
        let (_t1, request, root) = setup();
        write_package(
            &root,
            &manifest(r#"{"kind":"delete","destination":"game.bin"}"#, ""),
            None,
        );
        let deleted = inspect_local_mod_package(request);
        assert!(has_blocker(
            &deleted,
            ModPlanBlockerKind::UnsupportedOperation
        ));
        assert!(!deleted.eligible_for_later_apply);
        assert!(build_local_mod_package_transaction_plan(&deleted).is_err());

        // Patch.
        let (_t2, request, root) = setup();
        write_package(
            &root,
            &manifest(
                r#"{"kind":"patch","payload":"payload/p.ips","destination":"game.bin","patch_format":"ips"}"#,
                "",
            ),
            Some(("payload/p.ips", b"PATCH00")),
        );
        let patched = inspect_local_mod_package(request);
        assert!(has_blocker(
            &patched,
            ModPlanBlockerKind::UnsupportedPatchFormat
        ));
        assert!(!patched.eligible_for_later_apply);
        assert!(build_local_mod_package_transaction_plan(&patched).is_err());
    }
}
