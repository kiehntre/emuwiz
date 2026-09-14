//! RetroArch-specific minimal launch-resource planning and projection.
//!
//! This is the first consumer of `resource_grants`.  Planning is pure and
//! consumes already-selected content, core, save, and BIOS evidence.  The
//! optional materializer creates only an EmuWiz-owned launch view: it never
//! scans a BIOS tree, edits `retroarch.cfg`, or changes an authoritative source.
//! A grant's `READ_ONLY` value is contract intent; FI1 does not claim that a
//! symlink by itself is an OS sandbox.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::bios_projection::{BiosEvidence, BiosMatchStatus};
use crate::launch::resource_grants::{
    LaunchAccessScope, LaunchProjectionMethod, LaunchResourceAccess, LaunchResourceGrant,
    LaunchResourceGrantError, LaunchResourceGrantSet, LaunchResourceLifetime, LaunchResourceRole,
};
use crate::launch::retroarch_command::RetroArchCommand;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchBiosRequirement {
    pub name: String,
    pub filename: String,
    pub required: bool,
    /// Evidence selected by the existing BIOS planner. `None` is not a
    /// candidate and is never replaced by a filename search here.
    pub evidence: Option<BiosEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchResourceRequest {
    pub launch_id: String,
    pub launch_root: PathBuf,
    pub content_path: PathBuf,
    pub core_path: PathBuf,
    pub core_stem: String,
    pub platform_id: String,
    pub save_directory: PathBuf,
    pub bios_root: PathBuf,
    pub bios: Vec<RetroArchBiosRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchResourcePlan {
    pub launch_id: String,
    pub launch_root: PathBuf,
    pub system_directory: PathBuf,
    pub config_path: PathBuf,
    pub save_directory: PathBuf,
    pub core_path: PathBuf,
    pub core_stem: String,
    pub platform_id: String,
    pub config_contents: String,
    pub grants: LaunchResourceGrantSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetroArchResourcePlanError {
    EmptyLaunchId,
    EmptyCoreStem,
    UnsafePath(PathBuf),
    MissingSaveDirectory,
    MissingRequiredBios(String),
    AmbiguousBios(String),
    UnverifiedBios(String),
    BiosFilenameMismatch(String),
    ConflictingPresentedPath(PathBuf),
    Grant(LaunchResourceGrantError),
}

impl std::fmt::Display for RetroArchResourcePlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyLaunchId => write!(f, "RetroArch launch id must not be empty"),
            Self::EmptyCoreStem => write!(f, "RetroArch core stem must not be empty"),
            Self::UnsafePath(path) => {
                write!(f, "unsafe RetroArch resource path: {}", path.display())
            }
            Self::MissingSaveDirectory => {
                write!(f, "a writable RetroArch save directory is required")
            }
            Self::MissingRequiredBios(name) => {
                write!(f, "required RetroArch BIOS is missing: {name}")
            }
            Self::AmbiguousBios(name) => write!(f, "RetroArch BIOS evidence is ambiguous: {name}"),
            Self::UnverifiedBios(name) => {
                write!(f, "RetroArch BIOS evidence is not verified: {name}")
            }
            Self::BiosFilenameMismatch(name) => {
                write!(f, "selected BIOS evidence does not match {name}")
            }
            Self::ConflictingPresentedPath(path) => {
                write!(f, "conflicting RetroArch resource path: {}", path.display())
            }
            Self::Grant(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RetroArchResourcePlanError {}

impl From<LaunchResourceGrantError> for RetroArchResourcePlanError {
    fn from(value: LaunchResourceGrantError) -> Self {
        Self::Grant(value)
    }
}

fn safe_absolute(path: &Path) -> Result<(), RetroArchResourcePlanError> {
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(RetroArchResourcePlanError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn config_quote(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn grant(
    launch_id: &str,
    role: LaunchResourceRole,
    source_path: Option<PathBuf>,
    presented_path: Option<PathBuf>,
    access: LaunchResourceAccess,
    projection: LaunchProjectionMethod,
    lifetime: LaunchResourceLifetime,
    scope: LaunchAccessScope,
    provenance: String,
    reason: String,
) -> LaunchResourceGrant {
    LaunchResourceGrant {
        launch_id: launch_id.into(),
        role,
        source_path,
        presented_path,
        access,
        projection,
        lifetime,
        scope,
        provenance,
        reason,
    }
}

/// Build the minimal RetroArch contract from already-resolved evidence.
///
/// BIOS evidence must be `VerifiedMatch`; filename-only evidence is retained
/// by the caller as evidence but cannot become a grant. Optional missing BIOS
/// requirements are omitted. The master BIOS root is never granted.
pub fn plan_retroarch_resource_grants(
    request: &RetroArchResourceRequest,
) -> Result<RetroArchResourcePlan, RetroArchResourcePlanError> {
    if request.launch_id.is_empty() {
        return Err(RetroArchResourcePlanError::EmptyLaunchId);
    }
    if request.core_stem.is_empty() {
        return Err(RetroArchResourcePlanError::EmptyCoreStem);
    }
    for path in [
        &request.launch_root,
        &request.content_path,
        &request.core_path,
        &request.save_directory,
        &request.bios_root,
    ] {
        safe_absolute(path)?;
    }
    if request.save_directory.as_os_str().is_empty() {
        return Err(RetroArchResourcePlanError::MissingSaveDirectory);
    }

    let system_directory = request.launch_root.join("retroarch/system");
    let config_path = request
        .launch_root
        .join("retroarch/config/emuwiz-append.cfg");
    safe_absolute(&system_directory)?;
    safe_absolute(&config_path)?;

    let mut grants = LaunchResourceGrantSet::default();
    grants.try_insert(grant(
        &request.launch_id,
        LaunchResourceRole::GameMedia,
        Some(request.content_path.clone()),
        Some(request.content_path.clone()),
        LaunchResourceAccess::ReadOnly,
        LaunchProjectionMethod::DirectPath,
        LaunchResourceLifetime::LaunchOnly,
        LaunchAccessScope::StrictMinimal,
        "existing launch content evidence".into(),
        "selected RetroArch game/content".into(),
    ))?;
    grants.try_insert(grant(
        &request.launch_id,
        LaunchResourceRole::TemporaryRuntime,
        None,
        Some(system_directory.clone()),
        LaunchResourceAccess::CreateOnly,
        LaunchProjectionMethod::GeneratedDirectory,
        LaunchResourceLifetime::Session,
        LaunchAccessScope::StrictMinimal,
        "FI1 RetroArch launch view".into(),
        "minimal RetroArch system directory".into(),
    ))?;

    let mut presented_names = BTreeSet::new();
    for requirement in &request.bios {
        let Some(evidence) = requirement.evidence.as_ref() else {
            if requirement.required {
                return Err(RetroArchResourcePlanError::MissingRequiredBios(
                    requirement.name.clone(),
                ));
            }
            continue;
        };
        match evidence.match_status {
            BiosMatchStatus::VerifiedMatch => {}
            BiosMatchStatus::Ambiguous => {
                return Err(RetroArchResourcePlanError::AmbiguousBios(
                    requirement.name.clone(),
                ));
            }
            _ => {
                return Err(RetroArchResourcePlanError::UnverifiedBios(
                    requirement.name.clone(),
                ));
            }
        }
        if !evidence
            .filename
            .eq_ignore_ascii_case(&requirement.filename)
            || !presented_names.insert(requirement.filename.to_ascii_lowercase())
        {
            return Err(RetroArchResourcePlanError::BiosFilenameMismatch(
                requirement.name.clone(),
            ));
        }
        if evidence.relative_path.is_absolute() || evidence.relative_path.as_os_str().is_empty() {
            return Err(RetroArchResourcePlanError::UnsafePath(
                evidence.relative_path.clone(),
            ));
        }
        let source = evidence
            .relative_path
            .components()
            .any(|component| matches!(component, Component::ParentDir));
        if source {
            return Err(RetroArchResourcePlanError::UnsafePath(
                evidence.relative_path.clone(),
            ));
        }
        let source = request.bios_root.join(&evidence.relative_path);
        safe_absolute(&source)?;
        grants.try_insert(grant(
            &request.launch_id,
            LaunchResourceRole::BiosFirmware,
            Some(source),
            Some(system_directory.join(&requirement.filename)),
            LaunchResourceAccess::ReadOnly,
            LaunchProjectionMethod::SymlinkFile,
            LaunchResourceLifetime::Session,
            LaunchAccessScope::StrictMinimal,
            format!("existing BIOS evidence: {:?}", evidence.source),
            requirement.name.clone(),
        ))?;
    }

    grants.try_insert(grant(
        &request.launch_id,
        LaunchResourceRole::SaveData,
        Some(request.save_directory.clone()),
        Some(request.save_directory.clone()),
        LaunchResourceAccess::ReadWrite,
        LaunchProjectionMethod::DirectPath,
        LaunchResourceLifetime::Persistent,
        LaunchAccessScope::StrictMinimal,
        "existing RetroArch save-path policy".into(),
        "writable save data only".into(),
    ))?;
    grants.try_insert(grant(
        &request.launch_id,
        LaunchResourceRole::Config,
        None,
        Some(config_path.clone()),
        LaunchResourceAccess::CreateOnly,
        LaunchProjectionMethod::GeneratedFile,
        LaunchResourceLifetime::LaunchOnly,
        LaunchAccessScope::StrictMinimal,
        "FI1 generated RetroArch append configuration".into(),
        "point RetroArch at the minimal system directory and selected save path".into(),
    ))?;

    let config_contents = format!(
        "system_directory = \"{}\"\nsavefile_directory = \"{}\"\n",
        config_quote(&system_directory),
        config_quote(&request.save_directory),
    );
    Ok(RetroArchResourcePlan {
        launch_id: request.launch_id.clone(),
        launch_root: request.launch_root.clone(),
        system_directory,
        config_path,
        save_directory: request.save_directory.clone(),
        core_path: request.core_path.clone(),
        core_stem: request.core_stem.clone(),
        platform_id: request.platform_id.clone(),
        config_contents,
        grants,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchProjectionReceipt {
    pub launch_id: String,
    pub launch_root: PathBuf,
    pub created_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetroArchProjectionError {
    Plan(RetroArchResourcePlanError),
    UnsupportedPlatform,
    RootConflict(PathBuf),
    SourceInvalid(PathBuf),
    DestinationConflict(PathBuf),
    Io(String),
}

impl std::fmt::Display for RetroArchProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plan(error) => error.fmt(f),
            Self::UnsupportedPlatform => {
                write!(f, "RetroArch file projection requires Unix symlink support")
            }
            Self::RootConflict(path) => write!(
                f,
                "launch root is not an EmuWiz-owned directory: {}",
                path.display()
            ),
            Self::SourceInvalid(path) => write!(
                f,
                "source is not a regular non-symlink file: {}",
                path.display()
            ),
            Self::DestinationConflict(path) => write!(
                f,
                "launch destination already conflicts: {}",
                path.display()
            ),
            Self::Io(detail) => write!(f, "RetroArch resource projection failed: {detail}"),
        }
    }
}

impl std::error::Error for RetroArchProjectionError {}

#[cfg(unix)]
fn create_link(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(not(unix))]
fn create_link(_source: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlink support unavailable",
    ))
}

fn ensure_directory(
    path: &Path,
    created: &mut Vec<PathBuf>,
) -> Result<(), RetroArchProjectionError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(RetroArchProjectionError::RootConflict(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .map_err(|error| RetroArchProjectionError::Io(error.to_string()))?;
            created.push(path.to_path_buf());
            Ok(())
        }
        Err(error) => Err(RetroArchProjectionError::Io(error.to_string())),
    }
}

fn ensure_parents(
    path: &Path,
    root: &Path,
    created: &mut Vec<PathBuf>,
) -> Result<(), RetroArchProjectionError> {
    let parent = path
        .parent()
        .ok_or_else(|| RetroArchProjectionError::RootConflict(path.to_path_buf()))?;
    if !parent.starts_with(root) {
        return Err(RetroArchProjectionError::RootConflict(parent.to_path_buf()));
    }
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| RetroArchProjectionError::RootConflict(parent.to_path_buf()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(RetroArchProjectionError::RootConflict(parent.to_path_buf()));
        };
        current.push(name);
        ensure_directory(&current, created)?;
    }
    Ok(())
}

/// Materialize only the generated system/config view described by a plan.
/// This is not a general projection executor and never projects a directory
/// from the BIOS warehouse.
pub fn materialize_retroarch_resource_plan(
    plan: &RetroArchResourcePlan,
) -> Result<RetroArchProjectionReceipt, RetroArchProjectionError> {
    plan.grants
        .validate()
        .map_err(RetroArchResourcePlanError::from)
        .map_err(RetroArchProjectionError::Plan)?;
    if !plan.launch_root.is_absolute()
        || plan
            .launch_root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(RetroArchProjectionError::RootConflict(
            plan.launch_root.clone(),
        ));
    }
    let mut created = Vec::new();
    match fs::symlink_metadata(&plan.launch_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(RetroArchProjectionError::RootConflict(
                plan.launch_root.clone(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&plan.launch_root)
                .map_err(|error| RetroArchProjectionError::Io(error.to_string()))?;
            created.push(plan.launch_root.clone());
        }
        Err(error) => return Err(RetroArchProjectionError::Io(error.to_string())),
    }
    let marker = plan.launch_root.join(".emuwiz-retroarch-launch");
    if fs::symlink_metadata(&marker).is_ok() {
        return Err(RetroArchProjectionError::RootConflict(marker));
    }
    fs::write(
        &marker,
        format!("EMUWIZ_RETROARCH_LAUNCH\n{}\n", plan.launch_id),
    )
    .map_err(|error| RetroArchProjectionError::Io(error.to_string()))?;
    created.push(marker.clone());

    for grant in &plan.grants.grants {
        match grant.projection {
            LaunchProjectionMethod::GeneratedDirectory => {
                let destination = grant.presented_path.as_ref().expect("validated path");
                if !destination.starts_with(&plan.launch_root) {
                    return Err(RetroArchProjectionError::RootConflict(destination.clone()));
                }
                ensure_parents(destination, &plan.launch_root, &mut created)?;
                ensure_directory(destination, &mut created)?;
            }
            LaunchProjectionMethod::SymlinkFile => {
                let source = grant.source_path.as_ref().expect("validated source");
                let destination = grant.presented_path.as_ref().expect("validated path");
                let metadata = fs::symlink_metadata(source)
                    .map_err(|_| RetroArchProjectionError::SourceInvalid(source.clone()))?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(RetroArchProjectionError::SourceInvalid(source.clone()));
                }
                if !destination.starts_with(&plan.launch_root) {
                    return Err(RetroArchProjectionError::RootConflict(destination.clone()));
                }
                ensure_parents(destination, &plan.launch_root, &mut created)?;
                match fs::symlink_metadata(destination) {
                    Ok(existing)
                        if existing.file_type().is_symlink()
                            && fs::read_link(destination).ok().as_deref() == Some(source) => {}
                    Ok(_) => {
                        return Err(RetroArchProjectionError::DestinationConflict(
                            destination.clone(),
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        create_link(source, destination).map_err(|error| {
                            if error.kind() == std::io::ErrorKind::Unsupported {
                                RetroArchProjectionError::UnsupportedPlatform
                            } else {
                                RetroArchProjectionError::Io(error.to_string())
                            }
                        })?;
                        created.push(destination.clone());
                    }
                    Err(error) => return Err(RetroArchProjectionError::Io(error.to_string())),
                }
            }
            LaunchProjectionMethod::GeneratedFile => {
                let destination = grant.presented_path.as_ref().expect("validated path");
                if destination != &plan.config_path || !destination.starts_with(&plan.launch_root) {
                    return Err(RetroArchProjectionError::RootConflict(destination.clone()));
                }
                ensure_parents(destination, &plan.launch_root, &mut created)?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(destination)
                    .map_err(|error| RetroArchProjectionError::Io(error.to_string()))?;
                file.write_all(plan.config_contents.as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(|error| RetroArchProjectionError::Io(error.to_string()))?;
                created.push(destination.clone());
            }
            LaunchProjectionMethod::DirectPath => {
                let source = grant.source_path.as_ref().expect("validated source");
                let metadata = fs::symlink_metadata(source)
                    .map_err(|_| RetroArchProjectionError::SourceInvalid(source.clone()))?;
                if metadata.file_type().is_symlink()
                    || (grant.role == LaunchResourceRole::GameMedia && !metadata.is_file())
                    || (grant.role == LaunchResourceRole::SaveData && !metadata.is_dir())
                {
                    return Err(RetroArchProjectionError::SourceInvalid(source.clone()));
                }
            }
            _ => {}
        }
    }
    Ok(RetroArchProjectionReceipt {
        launch_id: plan.launch_id.clone(),
        launch_root: plan.launch_root.clone(),
        created_paths: created,
    })
}

/// Remove only paths recorded by the receipt, deepest first, after verifying
/// the ownership marker. Source files, saves, and unknown paths are untouched.
pub fn cleanup_retroarch_projection(
    receipt: &RetroArchProjectionReceipt,
) -> Result<(), RetroArchProjectionError> {
    let marker = receipt.launch_root.join(".emuwiz-retroarch-launch");
    let marker_text = fs::read_to_string(&marker)
        .map_err(|error| RetroArchProjectionError::Io(error.to_string()))?;
    if marker_text != format!("EMUWIZ_RETROARCH_LAUNCH\n{}\n", receipt.launch_id) {
        return Err(RetroArchProjectionError::RootConflict(marker));
    }
    let mut paths = receipt.created_paths.clone();
    paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in paths {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
                fs::remove_file(&path)
                    .map_err(|error| RetroArchProjectionError::Io(error.to_string()))?;
            }
            Ok(metadata) if metadata.is_dir() => {
                if fs::remove_dir(&path).is_err() {
                    // Never recursively delete a non-empty directory.
                }
            }
            Ok(_) => return Err(RetroArchProjectionError::RootConflict(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(RetroArchProjectionError::Io(error.to_string())),
        }
    }
    Ok(())
}

/// Add the generated append-config argument to an already-built command.
/// Existing `-L`, core, and content arguments remain untouched.
pub fn command_with_retroarch_resource_plan(
    command: &RetroArchCommand,
    plan: &RetroArchResourcePlan,
) -> Result<RetroArchCommand, RetroArchResourcePlanError> {
    plan.grants.validate()?;
    let mut command = command.clone();
    command.arguments.push("--appendconfig".into());
    command
        .arguments
        .push(plan.config_path.clone().into_os_string());
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bios_projection::{BiosEvidenceSource, BiosMatchStatus};

    fn request(temp: &tempfile::TempDir) -> RetroArchResourceRequest {
        let root = temp.path().join("launch");
        let bios_root = temp.path().join("bios");
        let source = bios_root.join("kick40068.A1200");
        std::fs::create_dir_all(&bios_root).unwrap();
        std::fs::write(&source, b"immutable firmware").unwrap();
        RetroArchResourceRequest {
            launch_id: "launch-1".into(),
            launch_root: root,
            content_path: temp.path().join("game.hdf"),
            core_path: temp.path().join("cores/puae_libretro.so"),
            core_stem: "puae".into(),
            platform_id: "AMIGA".into(),
            save_directory: temp.path().join("saves"),
            bios_root,
            bios: vec![RetroArchBiosRequirement {
                name: "A1200 Kickstart".into(),
                filename: "kick40068.A1200".into(),
                required: true,
                evidence: Some(BiosEvidence {
                    relative_path: PathBuf::from("kick40068.A1200"),
                    filename: "kick40068.A1200".into(),
                    size_bytes: 17,
                    sha256: Some("verified".into()),
                    source: BiosEvidenceSource::Hash,
                    match_status: BiosMatchStatus::VerifiedMatch,
                    platform: Some("AMIGA".into()),
                }),
            }],
        }
    }

    #[test]
    fn plans_game_bios_save_and_config_without_bios_root_grant() {
        let temp = tempfile::tempdir().unwrap();
        let plan = plan_retroarch_resource_grants(&request(&temp)).unwrap();
        assert_eq!(plan.grants.grants.len(), 5);
        assert!(
            plan.grants
                .grants
                .iter()
                .all(|grant| grant.source_path.as_deref()
                    != Some(Path::new("/mnt/usbdrive/[BIOS]/bios")))
        );
        assert!(
            plan.grants
                .grants
                .iter()
                .any(|grant| grant.role == LaunchResourceRole::BiosFirmware
                    && grant.projection == LaunchProjectionMethod::SymlinkFile)
        );
    }

    #[test]
    fn weak_or_missing_required_bios_fails_closed_and_optional_is_omitted() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = request(&temp);
        request.bios[0].evidence.as_mut().unwrap().match_status = BiosMatchStatus::FilenameOnly;
        assert!(matches!(
            plan_retroarch_resource_grants(&request),
            Err(RetroArchResourcePlanError::UnverifiedBios(_))
        ));
        request.bios[0].required = false;
        request.bios[0].evidence = None;
        assert!(plan_retroarch_resource_grants(&request).is_ok());
    }

    #[test]
    fn materialize_and_cleanup_touch_only_owned_launch_view() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = request(&temp);
        std::fs::write(&request.content_path, b"game").unwrap();
        std::fs::create_dir_all(&request.save_directory).unwrap();
        let plan = plan_retroarch_resource_grants(&request).unwrap();
        let source_hash = std::fs::read(&request.bios_root.join("kick40068.A1200")).unwrap();
        let receipt = materialize_retroarch_resource_plan(&plan).unwrap();
        let link = plan.system_directory.join("kick40068.A1200");
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            request.bios_root.join("kick40068.A1200")
        );
        assert!(plan.config_path.exists());
        cleanup_retroarch_projection(&receipt).unwrap();
        assert!(!link.exists());
        assert!(!plan.config_path.exists());
        assert_eq!(
            std::fs::read(&request.bios_root.join("kick40068.A1200")).unwrap(),
            source_hash
        );
        request.launch_id = "second".into();
    }

    #[test]
    fn command_uses_append_config_without_replacing_existing_arguments() {
        let temp = tempfile::tempdir().unwrap();
        let request = request(&temp);
        let plan = plan_retroarch_resource_grants(&request).unwrap();
        let command = RetroArchCommand {
            executable: PathBuf::from("/retroarch"),
            arguments: vec!["-L".into(), "/core.so".into(), "/game.hdf".into()],
            working_directory: None,
            selection: crate::launch::retroarch_command::RetroArchCommandSelection {
                profile: crate::emulator_environment::retroarch::ProfileRef {
                    profile_kind: crate::emulator_environment::retroarch::ProfileKind::Native,
                    scope: crate::emulator_environment::retroarch::ProfileScope::User,
                },
                core_stem: "puae".into(),
                platform_id: "AMIGA".into(),
                core_library: PathBuf::from("/core.so"),
                content_path: PathBuf::from("/game.hdf"),
            },
        };
        let updated = command_with_retroarch_resource_plan(&command, &plan).unwrap();
        assert_eq!(&updated.arguments[..3], &command.arguments[..]);
        assert_eq!(updated.arguments[3].to_str(), Some("--appendconfig"));
        assert_eq!(
            updated.arguments[4],
            plan.config_path.clone().into_os_string()
        );
    }
}
