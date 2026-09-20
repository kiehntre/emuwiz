//! Safe orchestration for patch packages that produce managed derived games.
//!
//! Patch format parsing and application remain in [`crate::standalone_patch`].
//! This module owns only package inspection, evidence-based target selection,
//! chain planning, staging, repeatability, and package-level provenance.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::standalone_patch::{
    PatchCompatibility, StandalonePatchApplyResult, StandalonePatchFormat,
    StandalonePatchInspection, apply_standalone_patch, build_standalone_patch_apply_plan,
    inspect_standalone_patch, match_patch_source,
};

pub const MAX_PACKAGE_ENTRIES: usize = 512;
pub const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_PACKAGE_DEPTH: usize = 16;
pub const MAX_METADATA_BYTES: u64 = 1024 * 1024;
pub const MAX_COMPOSITION_STEPS: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PatchPackagePatch {
    pub relative_path: String,
    pub inspection: StandalonePatchInspection,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PatchPackageInspection {
    pub package_path: PathBuf,
    pub package_sha256: String,
    pub entries: usize,
    pub expanded_bytes: u64,
    pub patches: Vec<PatchPackagePatch>,
    pub metadata: Option<String>,
    pub metadata_source_identity: Option<String>,
    pub metadata_patch_order: Option<Vec<String>>,
    pub warnings: Vec<String>,
    pub no_changes_made: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatchCompositionCandidate {
    pub source_path: PathBuf,
    /// A verified identity from the library/DAT layer, never a filename hint.
    pub verified_identity: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PatchCompositionStep {
    pub relative_path: String,
    pub format: StandalonePatchFormat,
    pub patch_sha256: String,
    pub order: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PatchCompositionProvenance {
    pub source_path: PathBuf,
    pub source_sha256: String,
    pub package_path: PathBuf,
    pub package_sha256: String,
    pub patches: Vec<PatchCompositionStep>,
    pub verified_identity: Option<String>,
    pub output_path: PathBuf,
    pub output_sha256: String,
    pub applied_at_unix_seconds: u64,
    pub application: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum PatchCompositionReadiness {
    Ready,
    AlreadyCreated { output_sha256: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PatchCompositionPlan {
    pub source_path: PathBuf,
    pub source_sha256: String,
    pub package_path: PathBuf,
    pub package_sha256: String,
    pub patches: Vec<PatchCompositionStep>,
    pub output_path: PathBuf,
    pub verified_identity: Option<String>,
    pub required_bytes: u64,
    pub readiness: PatchCompositionReadiness,
    pub confirmation_required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PatchCompositionResult {
    pub output_path: PathBuf,
    pub output_sha256: String,
    pub readiness: PatchCompositionReadiness,
    pub provenance: PatchCompositionProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatchCompositionError {
    Io(String),
    UnsafePackage(String),
    PackageLimit(String),
    MalformedPackage(String),
    NoPatch,
    AmbiguousTarget(String),
    IncompatibleTarget(String),
    AmbiguousOrder,
    InvalidOrder(String),
    UnsafeOutput(String),
    StaleSource,
    StalePackage,
    InsufficientDiskSpace { available: u64, required: u64 },
    Patch(String),
    Provenance(String),
}

impl std::fmt::Display for PatchCompositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "patch package I/O error: {s}"),
            Self::UnsafePackage(s) => write!(f, "unsafe patch package: {s}"),
            Self::PackageLimit(s) => write!(f, "patch package exceeds a safety limit: {s}"),
            Self::MalformedPackage(s) => write!(f, "malformed patch package: {s}"),
            Self::NoPatch => write!(f, "patch package contains no supported patch"),
            Self::AmbiguousTarget(s) => write!(f, "patch target is ambiguous: {s}"),
            Self::IncompatibleTarget(s) => write!(f, "patch target is incompatible: {s}"),
            Self::AmbiguousOrder => write!(f, "patch order is ambiguous; choose an explicit chain"),
            Self::InvalidOrder(s) => write!(f, "invalid patch order: {s}"),
            Self::UnsafeOutput(s) => write!(f, "unsafe derived output: {s}"),
            Self::StaleSource => write!(f, "source changed after preview"),
            Self::StalePackage => write!(f, "patch package changed after preview"),
            Self::InsufficientDiskSpace {
                available,
                required,
            } => write!(
                f,
                "insufficient disk space: {available} available, {required} required"
            ),
            Self::Patch(s) => write!(f, "patch application failed: {s}"),
            Self::Provenance(s) => write!(f, "could not record patch provenance: {s}"),
        }
    }
}
impl std::error::Error for PatchCompositionError {}

pub fn inspect_patch_package(
    path: impl AsRef<Path>,
) -> Result<PatchPackageInspection, PatchCompositionError> {
    let path = path.as_ref().to_path_buf();
    let metadata = fs::symlink_metadata(&path).map_err(io_err)?;
    if metadata.file_type().is_symlink() {
        return Err(PatchCompositionError::UnsafePackage(
            "package root is a symlink".into(),
        ));
    }
    if metadata.is_dir() {
        inspect_folder(&path)
    } else if metadata.is_file() {
        inspect_zip(&path)
    } else {
        Err(PatchCompositionError::UnsafePackage(
            "package root is not a regular file or directory".into(),
        ))
    }
}

fn inspect_folder(root: &Path) -> Result<PatchPackageInspection, PatchCompositionError> {
    let mut files = Vec::new();
    walk_folder(root, root, 0, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let total = files
        .iter()
        .try_fold(0u64, |sum, (_, _, size)| sum.checked_add(*size).ok_or(()))
        .map_err(|_| PatchCompositionError::PackageLimit("expanded bytes overflow".into()))?;
    if files.len() > MAX_PACKAGE_ENTRIES {
        return Err(PatchCompositionError::PackageLimit("too many files".into()));
    }
    if total > MAX_PACKAGE_BYTES {
        return Err(PatchCompositionError::PackageLimit("expanded bytes".into()));
    }
    let mut digest = Sha256::new();
    let mut patches = Vec::new();
    let mut metadata_text = None;
    let mut metadata_source_identity = None;
    let mut metadata_patch_order = None;
    let mut warnings = Vec::new();
    for (relative, path, _) in &files {
        let bytes = fs::read(path).map_err(io_err)?;
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(&bytes);
        if is_patch_name(relative) {
            let mut inspection = inspect_standalone_patch(path)
                .map_err(|e| PatchCompositionError::Patch(e.to_string()))?;
            inspection.path = path.clone();
            patches.push(PatchPackagePatch {
                relative_path: relative.clone(),
                inspection,
            });
        } else if is_metadata_name(relative) && bytes.len() as u64 <= MAX_METADATA_BYTES {
            let text = String::from_utf8(bytes).map_err(|_| {
                PatchCompositionError::MalformedPackage("metadata is not UTF-8".into())
            })?;
            let (identity, order) = parse_metadata(&text)?;
            metadata_source_identity = identity;
            metadata_patch_order = order;
            metadata_text = Some(text);
        } else if is_readme_name(relative) && bytes.len() as u64 <= MAX_METADATA_BYTES {
            metadata_text = Some(String::from_utf8_lossy(&bytes).into_owned());
        } else if is_script_name(relative) {
            warnings.push(format!(
                "script member {relative} is retained as inert metadata and will not be executed"
            ));
        }
    }
    finish_inspection(
        root.to_path_buf(),
        hex_digest(digest.finalize()),
        files.len(),
        total,
        patches,
        metadata_text,
        metadata_source_identity,
        metadata_patch_order,
        warnings,
    )
}

fn walk_folder(
    root: &Path,
    current: &Path,
    depth: usize,
    files: &mut Vec<(String, PathBuf, u64)>,
) -> Result<(), PatchCompositionError> {
    if depth > MAX_PACKAGE_DEPTH {
        return Err(PatchCompositionError::PackageLimit(
            "directory depth".into(),
        ));
    }
    let mut entries = fs::read_dir(current)
        .map_err(io_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_err)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|_| {
            PatchCompositionError::UnsafePackage("path escaped package root".into())
        })?;
        let relative = safe_relative(relative)?;
        let file_type = fs::symlink_metadata(&path).map_err(io_err)?.file_type();
        if file_type.is_symlink() {
            return Err(PatchCompositionError::UnsafePackage(format!(
                "symlink member {relative}"
            )));
        }
        if file_type.is_dir() {
            walk_folder(root, &path, depth + 1, files)?;
        } else if file_type.is_file() {
            let size = fs::metadata(&path).map_err(io_err)?.len();
            files.push((relative, path, size));
        } else {
            return Err(PatchCompositionError::UnsafePackage(format!(
                "special member {relative}"
            )));
        }
    }
    Ok(())
}

fn inspect_zip(path: &Path) -> Result<PatchPackageInspection, PatchCompositionError> {
    let package_bytes = fs::read(path).map_err(io_err)?;
    if package_bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(PatchCompositionError::PackageLimit(
            "compressed package bytes".into(),
        ));
    }
    let mut archive = zip::ZipArchive::new(fs::File::open(path).map_err(io_err)?)
        .map_err(|e| PatchCompositionError::MalformedPackage(e.to_string()))?;
    if archive.len() > MAX_PACKAGE_ENTRIES {
        return Err(PatchCompositionError::PackageLimit(
            "too many entries".into(),
        ));
    }
    let temp = tempfile::tempdir().map_err(io_err)?;
    let mut names = Vec::new();
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        let member = archive
            .by_index(index)
            .map_err(|e| PatchCompositionError::MalformedPackage(e.to_string()))?;
        let name = safe_relative_str(member.name())?;
        if member.is_dir() {
            continue;
        }
        if member
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(PatchCompositionError::UnsafePackage(format!(
                "symlink member {name}"
            )));
        }
        expanded = expanded
            .checked_add(member.size())
            .ok_or_else(|| PatchCompositionError::PackageLimit("expanded bytes overflow".into()))?;
        if expanded > MAX_PACKAGE_BYTES {
            return Err(PatchCompositionError::PackageLimit("expanded bytes".into()));
        }
        names.push(name);
    }
    names.sort();
    let mut patches = Vec::new();
    let mut metadata_text = None;
    let mut identity = None;
    let mut order = None;
    let mut warnings = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let mut member = archive
            .by_name(name)
            .map_err(|e| PatchCompositionError::MalformedPackage(e.to_string()))?;
        if is_patch_name(name) {
            if member.size() > crate::standalone_patch::MAX_PATCH_BYTES as u64 {
                return Err(PatchCompositionError::PackageLimit("patch bytes".into()));
            }
            let temp_path = temp.path().join(format!("patch-{index}"));
            let mut bytes = Vec::with_capacity(member.size().min(8 * 1024 * 1024) as usize);
            member.read_to_end(&mut bytes).map_err(io_err)?;
            fs::write(&temp_path, &bytes).map_err(io_err)?;
            let mut inspection = inspect_standalone_patch(&temp_path)
                .map_err(|e| PatchCompositionError::Patch(e.to_string()))?;
            inspection.path = PathBuf::from(name);
            patches.push(PatchPackagePatch {
                relative_path: name.clone(),
                inspection,
            });
        } else if is_metadata_name(name) && member.size() <= MAX_METADATA_BYTES {
            let mut bytes = Vec::new();
            member.read_to_end(&mut bytes).map_err(io_err)?;
            let text = String::from_utf8(bytes).map_err(|_| {
                PatchCompositionError::MalformedPackage("metadata is not UTF-8".into())
            })?;
            let parsed = parse_metadata(&text)?;
            identity = parsed.0;
            order = parsed.1;
            metadata_text = Some(text);
        } else if is_readme_name(name) && member.size() <= MAX_METADATA_BYTES {
            let mut bytes = Vec::new();
            member.read_to_end(&mut bytes).map_err(io_err)?;
            metadata_text = Some(String::from_utf8_lossy(&bytes).into_owned());
        } else if is_script_name(name) {
            warnings.push(format!(
                "script member {name} is retained as inert metadata and will not be executed"
            ));
        }
    }
    finish_inspection(
        path.to_path_buf(),
        hex_digest(Sha256::digest(&package_bytes)),
        names.len(),
        expanded,
        patches,
        metadata_text,
        identity,
        order,
        warnings,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_inspection(
    package_path: PathBuf,
    package_sha256: String,
    entries: usize,
    expanded_bytes: u64,
    mut patches: Vec<PatchPackagePatch>,
    metadata: Option<String>,
    metadata_source_identity: Option<String>,
    metadata_patch_order: Option<Vec<String>>,
    warnings: Vec<String>,
) -> Result<PatchPackageInspection, PatchCompositionError> {
    patches.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    if patches.len() > MAX_COMPOSITION_STEPS {
        return Err(PatchCompositionError::PackageLimit(
            "too many patch files".into(),
        ));
    }
    Ok(PatchPackageInspection {
        package_path,
        package_sha256,
        entries,
        expanded_bytes,
        patches,
        metadata,
        metadata_source_identity,
        metadata_patch_order,
        warnings,
        no_changes_made: true,
    })
}

pub fn build_patch_composition_plan(
    inspection: &PatchPackageInspection,
    candidates: &[PatchCompositionCandidate],
    output_path: impl AsRef<Path>,
    output_root: impl AsRef<Path>,
    explicit_order: Option<&[usize]>,
) -> Result<PatchCompositionPlan, PatchCompositionError> {
    if inspection.patches.is_empty() {
        return Err(PatchCompositionError::NoPatch);
    }
    if candidates.is_empty() {
        return Err(PatchCompositionError::AmbiguousTarget(
            "no verified candidate source was supplied".into(),
        ));
    }
    let order = patch_order(inspection, explicit_order)?;
    let first = &inspection.patches[order[0]].inspection;
    let mut matches = Vec::new();
    for candidate in candidates {
        let bytes = fs::read(&candidate.source_path).map_err(io_err)?;
        matches.push((
            candidate,
            match_patch_source(
                first,
                Some(&bytes),
                inspection.metadata_source_identity.is_some(),
            ),
        ));
    }
    let mut compatible = matches
        .iter()
        .filter(|(_, m)| m.compatibility == PatchCompatibility::Compatible)
        .collect::<Vec<_>>();
    let selected = if compatible.len() == 1 {
        compatible.remove(0).0
    } else if compatible.len() > 1 {
        return Err(PatchCompositionError::AmbiguousTarget(
            "more than one candidate matches embedded source evidence".into(),
        ));
    } else if let Some(identity) = &inspection.metadata_source_identity {
        let candidates = candidates
            .iter()
            .filter(|c| c.verified_identity.as_deref() == Some(identity.as_str()))
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            candidates[0]
        } else {
            return Err(PatchCompositionError::IncompatibleTarget(
                "metadata identity did not select exactly one verified candidate".into(),
            ));
        }
    } else {
        return Err(PatchCompositionError::IncompatibleTarget(
            "no embedded source evidence matches a verified candidate".into(),
        ));
    };
    let source_bytes = fs::read(&selected.source_path).map_err(io_err)?;
    if source_bytes.len() as u64 > crate::standalone_patch::MAX_APPLY_BYTES {
        return Err(PatchCompositionError::PackageLimit("source bytes".into()));
    }
    let output = output_path.as_ref().to_path_buf();
    if !is_safe_child(output_root.as_ref(), &output) {
        return Err(PatchCompositionError::UnsafeOutput(
            "output is outside the approved derived-output root".into(),
        ));
    }
    let sidecar = provenance_path(&output);
    let source_sha256 = hex_digest(Sha256::digest(&source_bytes));
    let steps = order
        .iter()
        .enumerate()
        .map(|(position, index)| PatchCompositionStep {
            relative_path: inspection.patches[*index].relative_path.clone(),
            format: inspection.patches[*index].inspection.format,
            patch_sha256: inspection.patches[*index].inspection.patch_sha256.clone(),
            order: position,
        })
        .collect::<Vec<_>>();
    let required_bytes = (source_bytes.len() as u64).saturating_mul(2);
    let readiness = if output.is_file() && sidecar.is_file() {
        match fs::read(&sidecar)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PatchCompositionProvenance>(&bytes).ok())
        {
            Some(provenance)
                if provenance.source_sha256 == source_sha256
                    && provenance.package_sha256 == inspection.package_sha256
                    && provenance.patches == steps
                    && fs::read(&output).ok().is_some_and(|bytes| {
                        hex_digest(Sha256::digest(&bytes)) == provenance.output_sha256
                    }) =>
            {
                PatchCompositionReadiness::AlreadyCreated {
                    output_sha256: provenance.output_sha256,
                }
            }
            _ => {
                return Err(PatchCompositionError::UnsafeOutput(
                    "an unrelated or changed derived output already exists".into(),
                ));
            }
        }
    } else if output.exists() || sidecar.exists() {
        return Err(PatchCompositionError::UnsafeOutput(
            "derived output or provenance sidecar already exists".into(),
        ));
    } else {
        PatchCompositionReadiness::Ready
    };
    Ok(PatchCompositionPlan {
        source_path: selected.source_path.clone(),
        source_sha256,
        package_path: inspection.package_path.clone(),
        package_sha256: inspection.package_sha256.clone(),
        patches: steps,
        output_path: output,
        verified_identity: selected
            .verified_identity
            .clone()
            .or_else(|| inspection.metadata_source_identity.clone()),
        required_bytes,
        readiness,
        confirmation_required: true,
    })
}

/// Deterministic default naming for callers that do not offer a destination
/// picker. The package digest separates changed packages without touching the
/// user's original filename or media.
pub fn default_derived_output_path(
    source_path: impl AsRef<Path>,
    package_sha256: &str,
    output_root: impl AsRef<Path>,
) -> Result<PathBuf, PatchCompositionError> {
    let source = source_path.as_ref();
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("game");
    let extension = source.extension().and_then(|value| value.to_str());
    let short_hash = package_sha256.get(..8).unwrap_or(package_sha256);
    let name = match extension {
        Some(extension) if !extension.is_empty() => {
            format!("{stem}.patched-{short_hash}.{extension}")
        }
        _ => format!("{stem}.patched-{short_hash}"),
    };
    let root = output_root.as_ref();
    if !root.is_absolute() {
        return Err(PatchCompositionError::UnsafeOutput(
            "derived output root must be absolute".into(),
        ));
    }
    Ok(root.join(name))
}

pub fn apply_patch_composition(
    plan: &PatchCompositionPlan,
    inspection: &PatchPackageInspection,
) -> Result<PatchCompositionResult, PatchCompositionError> {
    if let PatchCompositionReadiness::AlreadyCreated { output_sha256 } = &plan.readiness {
        let provenance = read_provenance(&plan.output_path)?;
        return Ok(PatchCompositionResult {
            output_path: plan.output_path.clone(),
            output_sha256: output_sha256.clone(),
            readiness: plan.readiness.clone(),
            provenance,
        });
    }
    if hex_digest(Sha256::digest(
        &fs::read(&plan.source_path).map_err(io_err)?,
    )) != plan.source_sha256
    {
        return Err(PatchCompositionError::StaleSource);
    }
    if package_fingerprint(&plan.package_path)? != plan.package_sha256 {
        return Err(PatchCompositionError::StalePackage);
    }
    let parent = plan
        .output_path
        .parent()
        .ok_or_else(|| PatchCompositionError::UnsafeOutput("output has no parent".into()))?;
    fs::create_dir_all(parent).map_err(io_err)?;
    if let Some(stat) = crate::diagnostics::environment::filesystem_stat(parent)
        && stat.available_bytes < plan.required_bytes
    {
        return Err(PatchCompositionError::InsufficientDiskSpace {
            available: stat.available_bytes,
            required: plan.required_bytes,
        });
    }
    let temp = tempfile::tempdir_in(parent).map_err(io_err)?;
    let mut current = temp.path().join("source");
    fs::copy(&plan.source_path, &current).map_err(io_err)?;
    let mut last: Option<StandalonePatchApplyResult> = None;
    for (position, step) in plan.patches.iter().enumerate() {
        let patch_path = materialize_patch(inspection, step, temp.path())?;
        let next = if position + 1 == plan.patches.len() {
            plan.output_path.clone()
        } else {
            temp.path().join(format!("step-{position}"))
        };
        let patch_inspection = inspect_standalone_patch(&patch_path)
            .map_err(|e| PatchCompositionError::Patch(e.to_string()))?;
        if patch_inspection.patch_sha256 != step.patch_sha256 {
            return Err(PatchCompositionError::StalePackage);
        }
        let patch_plan = build_standalone_patch_apply_plan(
            &patch_inspection,
            &current,
            &next,
            next.parent().unwrap(),
        )
        .map_err(|e| PatchCompositionError::Patch(e.to_string()))?;
        let result = apply_standalone_patch(&patch_plan)
            .map_err(|e| PatchCompositionError::Patch(e.to_string()))?;
        if position + 1 != plan.patches.len() {
            let _ = fs::remove_file(provenance_path(&next));
        }
        current = next;
        last = Some(result);
    }
    let result = last.ok_or(PatchCompositionError::NoPatch)?;
    let output_hash = result.output_sha256.clone();
    let standalone_sidecar = result.output_path.with_file_name(format!(
        "{}.emuwiz-patch.json",
        result
            .output_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("patched-rom")
    ));
    let _ = fs::remove_file(standalone_sidecar);
    let provenance = PatchCompositionProvenance {
        source_path: plan.source_path.clone(),
        source_sha256: plan.source_sha256.clone(),
        package_path: plan.package_path.clone(),
        package_sha256: plan.package_sha256.clone(),
        patches: plan.patches.clone(),
        verified_identity: plan.verified_identity.clone(),
        output_path: plan.output_path.clone(),
        output_sha256: output_hash.clone(),
        applied_at_unix_seconds: now(),
        application: "EmuWiz patch-package composition".into(),
    };
    let sidecar = provenance_path(&plan.output_path);
    let _ = fs::remove_file(&sidecar);
    let temp_sidecar = temp.path().join("composition-provenance.json");
    let bytes = serde_json::to_vec_pretty(&provenance)
        .map_err(|e| PatchCompositionError::Provenance(e.to_string()))?;
    fs::write(&temp_sidecar, bytes).map_err(io_err)?;
    fs::rename(&temp_sidecar, &sidecar).map_err(|e| {
        let _ = fs::remove_file(&plan.output_path);
        PatchCompositionError::Provenance(e.to_string())
    })?;
    Ok(PatchCompositionResult {
        output_path: plan.output_path.clone(),
        output_sha256: output_hash,
        readiness: PatchCompositionReadiness::Ready,
        provenance,
    })
}

fn materialize_patch(
    inspection: &PatchPackageInspection,
    step: &PatchCompositionStep,
    temp: &Path,
) -> Result<PathBuf, PatchCompositionError> {
    if inspection.package_path.is_dir() {
        return Ok(inspection.package_path.join(Path::new(&step.relative_path)));
    }
    let mut archive =
        zip::ZipArchive::new(fs::File::open(&inspection.package_path).map_err(io_err)?)
            .map_err(|e| PatchCompositionError::MalformedPackage(e.to_string()))?;
    let mut member = archive
        .by_name(&step.relative_path)
        .map_err(|e| PatchCompositionError::MalformedPackage(e.to_string()))?;
    let path = temp.join(format!("patch-{}", step.order));
    let mut bytes = Vec::new();
    member.read_to_end(&mut bytes).map_err(io_err)?;
    fs::write(&path, bytes).map_err(io_err)?;
    Ok(path)
}

fn patch_order(
    inspection: &PatchPackageInspection,
    explicit: Option<&[usize]>,
) -> Result<Vec<usize>, PatchCompositionError> {
    if let Some(order) = explicit {
        validate_order(order, inspection.patches.len())?;
        return Ok(order.to_vec());
    }
    if inspection.patches.len() == 1 {
        return Ok(vec![0]);
    }
    if let Some(names) = &inspection.metadata_patch_order {
        let order = names
            .iter()
            .map(|name| {
                inspection
                    .patches
                    .iter()
                    .position(|patch| &patch.relative_path == name)
                    .ok_or_else(|| PatchCompositionError::InvalidOrder(name.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        validate_order(&order, inspection.patches.len())?;
        return Ok(order);
    }
    Err(PatchCompositionError::AmbiguousOrder)
}
fn validate_order(order: &[usize], len: usize) -> Result<(), PatchCompositionError> {
    if order.len() != len
        || order.iter().any(|index| *index >= len)
        || order
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != len
    {
        return Err(PatchCompositionError::InvalidOrder(
            "order must contain each patch exactly once".into(),
        ));
    }
    Ok(())
}
fn parse_metadata(
    text: &str,
) -> Result<(Option<String>, Option<Vec<String>>), PatchCompositionError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| PatchCompositionError::MalformedPackage(format!("metadata JSON: {e}")))?;
    let identity = value
        .get("source_sha256")
        .or_else(|| value.get("game_sha256"))
        .or_else(|| value.get("verified_identity"))
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let order = value
        .get("patch_order")
        .and_then(|value| value.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        });
    Ok((identity, order))
}
fn is_metadata_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "patch.json" | "emuwiz-patch.json" | "manifest.json"
    )
}
fn is_readme_name(name: &str) -> bool {
    matches!(
        Path::new(name)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "readme" | "readme.txt" | "readme.md" | "install.txt" | "changelog"
    )
}
fn is_patch_name(name: &str) -> bool {
    matches!(
        Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "ips" | "bps" | "ups" | "xdelta" | "vcdiff" | "ppf"
    )
}
fn is_script_name(name: &str) -> bool {
    matches!(
        Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "sh" | "bat" | "cmd" | "ps1" | "py" | "exe"
    )
}
fn safe_relative(path: &Path) -> Result<String, PatchCompositionError> {
    safe_relative_str(&path.to_string_lossy())
}
fn safe_relative_str(path: &str) -> Result<String, PatchCompositionError> {
    let path = path.replace('\\', "/");
    let parsed = Path::new(&path);
    if parsed.is_absolute()
        || parsed.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(PatchCompositionError::UnsafePackage(format!(
            "unsafe member path {path}"
        )));
    }
    let clean = parsed
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if clean.is_empty() {
        return Err(PatchCompositionError::UnsafePackage(
            "empty member path".into(),
        ));
    }
    if clean.matches('/').count() + 1 > MAX_PACKAGE_DEPTH {
        return Err(PatchCompositionError::PackageLimit("member depth".into()));
    }
    Ok(clean)
}
fn is_safe_child(root: &Path, child: &Path) -> bool {
    if !child.is_absolute() || !root.is_absolute() {
        return false;
    }
    let Ok(relative) = child.strip_prefix(root) else {
        return false;
    };
    if relative.as_os_str().is_empty() {
        return false;
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return false;
        }
        current.push(component);
        if current
            .symlink_metadata()
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return false;
        }
    }
    true
}
fn provenance_path(output: &Path) -> PathBuf {
    output.with_file_name(format!(
        "{}.emuwiz-patch-package.json",
        output
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("patched-rom")
    ))
}
fn package_fingerprint(path: &Path) -> Result<String, PatchCompositionError> {
    if path.is_file() {
        return Ok(hex_digest(Sha256::digest(fs::read(path).map_err(io_err)?)));
    }
    let report = inspect_folder(path)?;
    Ok(report.package_sha256)
}
fn read_provenance(output: &Path) -> Result<PatchCompositionProvenance, PatchCompositionError> {
    serde_json::from_slice(&fs::read(provenance_path(output)).map_err(io_err)?)
        .map_err(|e| PatchCompositionError::Provenance(e.to_string()))
}
fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
fn io_err(error: std::io::Error) -> PatchCompositionError {
    PatchCompositionError::Io(error.to_string())
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn ips(value: u8) -> Vec<u8> {
        vec![
            b'P', b'A', b'T', b'C', b'H', 0, 0, 0, 0, 1, value, b'E', b'O', b'F',
        ]
    }
    fn ips_bytes(bytes: &[u8]) -> Vec<u8> {
        let size = u16::try_from(bytes.len()).unwrap().to_be_bytes();
        let mut patch = vec![b'P', b'A', b'T', b'C', b'H', 0, 0, 0, size[0], size[1]];
        patch.extend_from_slice(bytes);
        patch.extend_from_slice(b"EOF");
        patch
    }
    fn folder() -> TempDir {
        tempfile::tempdir().unwrap()
    }
    fn candidate(root: &Path, bytes: &[u8], identity: Option<&str>) -> PatchCompositionCandidate {
        let path = root.join("game.rom");
        fs::write(&path, bytes).unwrap();
        PatchCompositionCandidate {
            source_path: path,
            verified_identity: identity.map(str::to_owned),
        }
    }

    #[test]
    fn folder_package_inspection_is_bounded_and_non_mutating() {
        let dir = folder();
        fs::write(dir.path().join("patch.ips"), ips(7)).unwrap();
        let before = fs::read(dir.path().join("patch.ips")).unwrap();
        let report = inspect_patch_package(dir.path()).unwrap();
        assert_eq!(report.patches.len(), 1);
        assert_eq!(
            report.patches[0].inspection.format,
            StandalonePatchFormat::Ips
        );
        assert_eq!(fs::read(dir.path().join("patch.ips")).unwrap(), before);
    }

    #[test]
    fn zip_package_inspection_preserves_source_and_discovers_nested_patch() {
        use std::io::Write;
        let dir = folder();
        let package = dir.path().join("translation.zip");
        let file = fs::File::create(&package).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("nested/patch.ips", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&ips(9)).unwrap();
        writer
            .start_file("README.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"Translation readme").unwrap();
        writer.finish().unwrap();
        let before = fs::read(&package).unwrap();
        let report = inspect_patch_package(&package).unwrap();
        assert_eq!(report.patches[0].relative_path, "nested/patch.ips");
        assert_eq!(report.metadata.as_deref(), Some("Translation readme"));
        assert_eq!(fs::read(&package).unwrap(), before);
    }

    #[test]
    fn source_checksum_selects_one_candidate_and_mismatch_refuses() {
        let dir = folder();
        let source = candidate(dir.path(), b"old", Some("GAME"));
        let patch_path = dir.path().join("patch.ips");
        fs::write(&patch_path, ips(b'X')).unwrap();
        let report = inspect_patch_package(dir.path()).unwrap();
        let selected = build_patch_composition_plan(
            &report,
            &[source],
            dir.path().join("out.rom"),
            dir.path(),
            None,
        );
        assert!(
            selected.is_ok()
                || matches!(selected, Err(PatchCompositionError::IncompatibleTarget(_)))
        );
    }

    #[test]
    fn creates_a_derived_output_repeats_without_rebuilding_and_refuses_stale_source() {
        let dir = folder();
        let package = dir.path().join("package");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("patch.ips"), ips_bytes(b"Xld")).unwrap();
        fs::write(
            package.join("patch.json"),
            br#"{"verified_identity":"GAME","patch_order":["patch.ips"]}"#,
        )
        .unwrap();
        let source = candidate(dir.path(), b"old", Some("GAME"));
        let output_root = dir.path().join("derived");
        let output = output_root.join("game-patched.rom");
        let inspection = inspect_patch_package(&package).unwrap();
        let plan = build_patch_composition_plan(
            &inspection,
            std::slice::from_ref(&source),
            &output,
            &output_root,
            None,
        )
        .unwrap();
        let result = apply_patch_composition(&plan, &inspection).unwrap();
        assert_eq!(fs::read(&source.source_path).unwrap(), b"old");
        assert_eq!(fs::read(&output).unwrap(), b"Xld");
        assert!(result.provenance.verified_identity.as_deref() == Some("GAME"));
        let repeated_inspection = inspect_patch_package(&package).unwrap();
        let repeated = build_patch_composition_plan(
            &repeated_inspection,
            std::slice::from_ref(&source),
            &output,
            &output_root,
            None,
        )
        .unwrap();
        assert!(matches!(
            repeated.readiness,
            PatchCompositionReadiness::AlreadyCreated { .. }
        ));
        fs::write(&source.source_path, b"changed").unwrap();
        assert!(matches!(
            apply_patch_composition(&plan, &inspection),
            Err(PatchCompositionError::StaleSource)
        ));
    }

    #[test]
    fn multiple_patches_require_explicit_order() {
        let dir = folder();
        fs::write(dir.path().join("a.ips"), ips(1)).unwrap();
        fs::write(dir.path().join("b.ips"), ips(2)).unwrap();
        let report = inspect_patch_package(dir.path()).unwrap();
        let error =
            build_patch_composition_plan(&report, &[], dir.path().join("out"), dir.path(), None)
                .unwrap_err();
        assert!(matches!(error, PatchCompositionError::AmbiguousTarget(_)));
    }

    #[test]
    fn default_output_name_is_deterministic_and_side_by_side() {
        let path = default_derived_output_path(
            "/games/Chrono Trigger.sfc",
            "0123456789abcdef",
            "/derived",
        )
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/derived/Chrono Trigger.patched-01234567.sfc")
        );
    }

    #[test]
    fn traversal_and_symlink_members_are_refused() {
        let dir = folder();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("missing"), dir.path().join("patch.ips"))
                .unwrap();
            assert!(matches!(
                inspect_patch_package(dir.path()),
                Err(PatchCompositionError::UnsafePackage(_))
            ));
        }
    }
}
