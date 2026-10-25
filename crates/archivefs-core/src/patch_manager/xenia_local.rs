//! Bounded, read-only discovery of local Xenia Canary installations.
//!
//! Xenia Canary has no single standard install location: real installs are
//! typically portable (a folder holding an executable,
//! `xenia-canary.config.toml`, and its own `patches` directory). No native
//! path is guessed here without real evidence for one - discovery is
//! deliberately limited to caller-supplied explicit directories, each
//! validated the same way as every other emulator adapter's profile roots.
//!
//! [`resolve_xenia_launch_binding`] (below) only ever authorizes a native
//! Linux executable found in that directory - `xenia_canary` or `xenia`,
//! never `xenia_canary.exe`. A portable folder that only carries the
//! Windows binary is a real, eligible profile (its `patches`/config
//! evidence is unaffected), but it has no valid *launch binding* until a
//! native Linux executable is placed there too; this crate never assumes or
//! configures Wine/Proton to run the `.exe` for it.
//!
//! This module never starts Xenia and has no write or network capability.

use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::emulator_environment::EncodedPath;

use super::destination_safety::{
    DestinationRootState, DestinationSafetyFailureReason, validate_destination_root,
};
use super::xenia_install_plan::{
    LoadedXeniaDestination, StagedXeniaPatchFile, XeniaCandidate, XeniaCandidateCompatibility,
    XeniaInstallPlanError, XeniaPatchSelection, build_xenia_candidates, load_xenia_destination,
    stage_xenia_patch_file,
};
use super::xenia_patch_document::{XeniaPatchDocument, parse_xenia_patch_toml};

/// Bound for a user-selected local Xenia patch file. It is deliberately the
/// same order of magnitude as the existing parser bound, with a separate
/// local adapter check so unsafe sources are rejected before parsing.
pub const MAX_LOCAL_XENIA_PATCH_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalXeniaFileError {
    NotFound { path: PathBuf, detail: String },
    IsDirectory { path: PathBuf },
    IsSymlink { path: PathBuf },
    NotRegularFile { path: PathBuf },
    UnsupportedExtension { path: PathBuf },
    TooLarge { path: PathBuf },
    Malformed { path: PathBuf, detail: String },
    NoPatchesFound { path: PathBuf },
    IdentityUnavailable { detail: String },
    IdentityConflict { detail: String },
    PartialIdentityVerification { detail: String },
    Plan(XeniaInstallPlanError),
}

impl std::fmt::Display for LocalXeniaFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::IsDirectory { path } => write!(formatter, "{} is a directory", path.display()),
            Self::IsSymlink { path } => write!(
                formatter,
                "{} is a symlink and is not followed",
                path.display()
            ),
            Self::NotRegularFile { path } => {
                write!(formatter, "{} is not a regular file", path.display())
            }
            Self::UnsupportedExtension { path } => {
                write!(formatter, "{} is not a .patch.toml file", path.display())
            }
            Self::TooLarge { path } => write!(
                formatter,
                "{} exceeds the local Xenia patch size limit",
                path.display()
            ),
            Self::Malformed { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::NoPatchesFound { path } => write!(
                formatter,
                "{} contains no valid patch entries",
                path.display()
            ),
            Self::IdentityUnavailable { detail } => formatter.write_str(detail),
            Self::IdentityConflict { detail } => formatter.write_str(detail),
            Self::PartialIdentityVerification { detail } => formatter.write_str(detail),
            Self::Plan(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for LocalXeniaFileError {}

impl From<XeniaInstallPlanError> for LocalXeniaFileError {
    fn from(error: XeniaInstallPlanError) -> Self {
        Self::Plan(error)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalXeniaDiscovery {
    pub source_path: PathBuf,
    pub source_sha256: String,
    pub document: XeniaPatchDocument,
    pub candidate: XeniaCandidate,
}

/// Inspects one user-selected `.patch.toml` and binds it to the already
/// resolved selected game's exact Xenia Title ID. This function has no
/// network path and refuses unsafe sources before parsing.
pub fn discover_local_xenia_patch_file(
    source_path: &Path,
    selected_title_id: Option<&str>,
) -> Result<LocalXeniaDiscovery, LocalXeniaFileError> {
    let selected_title_id = selected_title_id
        .filter(|value| value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| LocalXeniaFileError::IdentityUnavailable {
            detail: "selected game's verified Xenia Title ID is unresolved or ambiguous"
                .to_string(),
        })?;
    let metadata =
        fs::symlink_metadata(source_path).map_err(|error| LocalXeniaFileError::NotFound {
            path: source_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    if metadata.file_type().is_symlink() {
        return Err(LocalXeniaFileError::IsSymlink {
            path: source_path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        return Err(LocalXeniaFileError::IsDirectory {
            path: source_path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(LocalXeniaFileError::NotRegularFile {
            path: source_path.to_path_buf(),
        });
    }
    let valid_extension = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".patch.toml"));
    if !valid_extension {
        return Err(LocalXeniaFileError::UnsupportedExtension {
            path: source_path.to_path_buf(),
        });
    }
    if metadata.len() > MAX_LOCAL_XENIA_PATCH_BYTES {
        return Err(LocalXeniaFileError::TooLarge {
            path: source_path.to_path_buf(),
        });
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options
        .open(source_path)
        .map_err(|error| LocalXeniaFileError::NotFound {
            path: source_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let mut bytes = Vec::new();
    file.take(MAX_LOCAL_XENIA_PATCH_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| LocalXeniaFileError::Malformed {
            path: source_path.to_path_buf(),
            detail: format!("could not be read: {error}"),
        })?;
    if bytes.len() as u64 > MAX_LOCAL_XENIA_PATCH_BYTES {
        return Err(LocalXeniaFileError::TooLarge {
            path: source_path.to_path_buf(),
        });
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| LocalXeniaFileError::Malformed {
        path: source_path.to_path_buf(),
        detail: "Xenia patch file is not valid UTF-8".to_string(),
    })?;
    let document = parse_xenia_patch_toml(text);
    if document.is_fatally_malformed() || document.has_rewrite_blocking_warnings() {
        return Err(LocalXeniaFileError::Malformed {
            path: source_path.to_path_buf(),
            detail: document
                .warnings
                .first()
                .map(|warning| warning.detail.clone())
                .unwrap_or_else(|| {
                    "Xenia patch TOML contains unsupported or malformed entries".to_string()
                }),
        });
    }
    if document.selectable_patch_count() == 0 {
        return Err(LocalXeniaFileError::NoPatchesFound {
            path: source_path.to_path_buf(),
        });
    }
    if document.title_id != selected_title_id {
        return Err(LocalXeniaFileError::IdentityConflict {
            detail: format!(
                "{} declares Xenia Title ID {}, not selected game's verified {}",
                source_path.display(),
                document.title_id,
                selected_title_id
            ),
        });
    }
    let provider = super::xenia_provider::XeniaProviderResult {
        provider_id: "local_xenia_patch_file".to_string(),
        provider_display_name: "Local Xenia .patch.toml file".to_string(),
        source_repository: "local file".to_string(),
        source_commit: "local".to_string(),
        retrieved_at_unix_seconds: 0,
        title_id: document.title_id.clone(),
        documents: vec![super::xenia_provider::XeniaProviderDocument {
            source_path: source_path.display().to_string(),
            document: document.clone(),
        }],
        attribution: "User-selected local file".to_string(),
        license: "User-provided".to_string(),
        warnings: Vec::new(),
    };
    let outcome = build_xenia_candidates(&provider, Some(selected_title_id), None);
    let candidate =
        outcome
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| LocalXeniaFileError::Malformed {
                path: source_path.to_path_buf(),
                detail: "local Xenia patch did not produce a valid candidate".to_string(),
            })?;
    if candidate.compatibility == XeniaCandidateCompatibility::Incompatible {
        return Err(LocalXeniaFileError::IdentityConflict {
            detail: "local Xenia patch is incompatible with the selected game".to_string(),
        });
    }
    Ok(LocalXeniaDiscovery {
        source_path: source_path.to_path_buf(),
        source_sha256: hex_sha256(&bytes),
        document,
        candidate,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalXeniaInstallState {
    New,
    AlreadyInstalled,
}

#[must_use]
pub fn check_local_xenia_install_state(
    destination: &LoadedXeniaDestination,
    discovery: &LocalXeniaDiscovery,
) -> LocalXeniaInstallState {
    let Some(existing) = destination.document.as_ref() else {
        return LocalXeniaInstallState::New;
    };
    let unchanged = discovery.document.patches.iter().all(|patch| {
        let mut expected = patch.clone();
        expected.enabled_by_default = true;
        existing.patches.iter().any(|candidate| {
            candidate.name == patch.name && candidate == &expected && candidate.enabled_by_default
        })
    });
    if unchanged {
        LocalXeniaInstallState::AlreadyInstalled
    } else {
        LocalXeniaInstallState::New
    }
}

pub fn load_local_xenia_destination(
    configuration_path: &Path,
    file_name: &str,
) -> Result<LoadedXeniaDestination, LocalXeniaFileError> {
    load_xenia_destination(&configuration_path.join("patches"), file_name).map_err(Into::into)
}

pub fn stage_local_xenia_patch_file(
    staging_root: &Path,
    file_name: &str,
    discovery: &LocalXeniaDiscovery,
    existing: Option<&XeniaPatchDocument>,
) -> Result<StagedXeniaPatchFile, LocalXeniaFileError> {
    let mut selected = XeniaPatchSelection::from_candidate(&discovery.candidate, existing);
    selected.select_all();
    // Selecting a concrete local file is the local-workflow equivalent of
    // the existing Xenia picker acknowledgement: the preview still exposes
    // the Strong/partially-verified evidence and confirmation is required
    // before the shared transaction can mutate the profile.
    selected.partial_verification_acknowledged = true;
    stage_xenia_patch_file(
        staging_root,
        file_name,
        &discovery.candidate,
        existing,
        &selected.resolve_names()?,
    )
    .map_err(Into::into)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub const XENIA_MAX_PROFILES: usize = 16;

/// The exact config filename Xenia Canary itself uses
/// (`xe::config::config_name` upstream) - the marker that distinguishes a
/// real Xenia Canary directory from an arbitrary folder.
const XENIA_CONFIG_MARKER: &str = "xenia-canary.config.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XeniaInstallationType {
    /// A caller-supplied directory - the only kind EmuWiz currently
    /// discovers, since Xenia Canary has no single documented native path.
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XeniaProfileScope {
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XeniaProfileBlockerKind {
    PathNotAbsolute,
    FilesystemRoot,
    MissingConfiguration,
    UnsafePath,
    NotDirectory,
    Unreadable,
    MissingXeniaEvidence,
    ProfileLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XeniaProfileBlocker {
    pub kind: XeniaProfileBlockerKind,
    pub path: EncodedPath,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XeniaPatchesDirectoryState {
    Available,
    Missing,
    UnsafePath,
    NotDirectory,
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XeniaDirectoryIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XeniaProfile {
    pub profile_id: String,
    pub installation_type: XeniaInstallationType,
    pub scope: XeniaProfileScope,
    /// The Xenia Canary root directory - alongside `xenia_canary.exe` and
    /// `xenia-canary.config.toml` in a real install.
    pub configuration_path: PathBuf,
    pub provenance: &'static str,
    pub eligible: bool,
    pub blockers: Vec<XeniaProfileBlocker>,
    /// `configuration_path/patches` - the only directory EmuWiz ever
    /// manages for this adapter.
    pub patches_path: PathBuf,
    pub patches_state: XeniaPatchesDirectoryState,
    pub patches_warning: Option<String>,
    pub configuration_identity: Option<XeniaDirectoryIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XeniaProfileDiscovery {
    pub profiles: Vec<XeniaProfile>,
    pub warnings: Vec<XeniaProfileBlocker>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XeniaProfileDiscoveryRoots {
    /// Exact, already-known Xenia Canary directories; never searched for.
    pub explicit_configuration_roots: Vec<PathBuf>,
}

/// Discovers only caller-supplied explicit Xenia Canary directories - see
/// the module documentation for why no native path is guessed.
pub fn discover_xenia_profiles(roots: &XeniaProfileDiscoveryRoots) -> XeniaProfileDiscovery {
    let mut candidates: Vec<PathBuf> = roots.explicit_configuration_roots.clone();
    candidates.sort();
    candidates.dedup();

    let mut profiles = Vec::new();
    let mut warnings = Vec::new();
    for path in candidates {
        if profiles.len() >= XENIA_MAX_PROFILES {
            warnings.push(blocker(
                XeniaProfileBlockerKind::ProfileLimitReached,
                &path,
                format!("profile discovery stopped at the {XENIA_MAX_PROFILES}-profile limit"),
            ));
            break;
        }
        if !path.is_absolute() {
            profiles.push(blocked(
                path,
                XeniaProfileBlockerKind::PathNotAbsolute,
                "configuration path is not absolute",
            ));
            continue;
        }
        if path.parent().is_none() {
            profiles.push(blocked(
                path,
                XeniaProfileBlockerKind::FilesystemRoot,
                "a filesystem root cannot be a Xenia profile",
            ));
            continue;
        }
        let validated = match validate_destination_root(&path) {
            Ok(value) => value,
            Err(error) => {
                let kind = match error.reason {
                    DestinationSafetyFailureReason::RootNotDirectory
                    | DestinationSafetyFailureReason::NonDirectoryParent => {
                        XeniaProfileBlockerKind::NotDirectory
                    }
                    DestinationSafetyFailureReason::InspectionFailed => {
                        XeniaProfileBlockerKind::Unreadable
                    }
                    _ => XeniaProfileBlockerKind::UnsafePath,
                };
                profiles.push(blocked(
                    path,
                    kind,
                    format!("configuration path rejected: {:?}", error.reason),
                ));
                continue;
            }
        };
        if validated.state() == DestinationRootState::Absent {
            profiles.push(blocked(
                path,
                XeniaProfileBlockerKind::MissingConfiguration,
                "configuration directory does not exist",
            ));
            continue;
        }
        if let Err((kind, detail)) = inspect_marker(&path) {
            profiles.push(blocked(path, kind, detail));
            continue;
        }
        let patches_path = path.join("patches");
        let (patches_state, patches_warning, _) = inspect_patches_directory(&patches_path);
        let identity = fs::symlink_metadata(&path)
            .ok()
            .and_then(|metadata| directory_identity(&metadata));
        profiles.push(XeniaProfile {
            profile_id: profile_id(&path),
            installation_type: XeniaInstallationType::Explicit,
            scope: XeniaProfileScope::Explicit,
            configuration_path: path,
            provenance: "Explicitly supplied Xenia Canary directory",
            eligible: true,
            blockers: Vec::new(),
            patches_path,
            patches_state,
            patches_warning,
            configuration_identity: identity,
        });
    }
    profiles.sort_by(|a, b| a.configuration_path.cmp(&b.configuration_path));
    XeniaProfileDiscovery {
        complete: warnings.is_empty(),
        profiles,
        warnings,
    }
}

fn inspect_marker(root: &Path) -> Result<(), (XeniaProfileBlockerKind, &'static str)> {
    let marker = root.join(XENIA_CONFIG_MARKER);
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err((
            XeniaProfileBlockerKind::UnsafePath,
            "xenia-canary.config.toml is a symlink",
        )),
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err((
            XeniaProfileBlockerKind::MissingXeniaEvidence,
            "xenia-canary.config.toml is not a regular file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err((
            XeniaProfileBlockerKind::MissingXeniaEvidence,
            "xenia-canary.config.toml was not found in this directory",
        )),
        Err(_) => Err((
            XeniaProfileBlockerKind::Unreadable,
            "xenia-canary.config.toml is unreadable",
        )),
    }
}

fn inspect_patches_directory(
    path: &Path,
) -> (
    XeniaPatchesDirectoryState,
    Option<String>,
    Option<XeniaDirectoryIdentity>,
) {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => (
            XeniaPatchesDirectoryState::UnsafePath,
            Some("patches is a symlink and will not be followed".into()),
            None,
        ),
        Ok(metadata) if metadata.is_dir() => (
            XeniaPatchesDirectoryState::Available,
            None,
            directory_identity(&metadata),
        ),
        Ok(_) => (
            XeniaPatchesDirectoryState::NotDirectory,
            Some("patches is not a directory".into()),
            None,
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            (XeniaPatchesDirectoryState::Missing, None, None)
        }
        Err(error) => (
            XeniaPatchesDirectoryState::Unreadable,
            Some(format!("patches cannot be inspected: {error}")),
            None,
        ),
    }
}

fn blocked(
    path: PathBuf,
    kind: XeniaProfileBlockerKind,
    detail: impl Into<String>,
) -> XeniaProfile {
    let patches_path = path.join("patches");
    XeniaProfile {
        profile_id: profile_id(&path),
        installation_type: XeniaInstallationType::Explicit,
        scope: XeniaProfileScope::Explicit,
        configuration_path: path.clone(),
        provenance: "Explicitly supplied Xenia Canary directory",
        eligible: false,
        blockers: vec![blocker(kind, &path, detail)],
        patches_path,
        patches_state: XeniaPatchesDirectoryState::Missing,
        patches_warning: None,
        configuration_identity: None,
    }
}

fn blocker(
    kind: XeniaProfileBlockerKind,
    path: &Path,
    detail: impl Into<String>,
) -> XeniaProfileBlocker {
    XeniaProfileBlocker {
        kind,
        path: EncodedPath::from_path(path),
        detail: detail.into(),
    }
}

fn profile_id(path: &Path) -> String {
    let mut digest = Sha256::new();
    #[cfg(unix)]
    digest.update(path.as_os_str().as_bytes());
    #[cfg(not(unix))]
    digest.update(path.as_os_str().to_string_lossy().as_bytes());
    format!(
        "xenia-explicit-{:016x}",
        u64::from_be_bytes(digest.finalize()[..8].try_into().unwrap())
    )
}

#[cfg(unix)]
fn directory_identity(metadata: &fs::Metadata) -> Option<XeniaDirectoryIdentity> {
    metadata.is_dir().then(|| XeniaDirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn directory_identity(_metadata: &fs::Metadata) -> Option<XeniaDirectoryIdentity> {
    None
}

// ---------------------------------------------------------------------------
// Launch binding
// ---------------------------------------------------------------------------
//
// Proves, freshly and read-only, exactly which Xenia Canary executable
// belongs to a discovered profile - the standalone-launch prerequisite
// [`crate::launch::xenia_command`] needs. This never launches Xenia, never
// writes configuration, and never creates a directory.
//
// # Native Linux executable only - `xenia_canary.exe` is never a binding
//
// Upstream Xenia Canary ships both a Windows `xenia_canary.exe` and, in its
// Linux release artifacts, a native ELF binary alongside the same
// `xenia-canary.config.toml`/`patches` layout, conventionally named
// `xenia_canary` (no extension) - `xenia` is also accepted for a mainline
// Xenia portable directory laid out the same way. This resolver only ever
// searches for those extensionless native names: a `.exe` sitting in the
// same directory is never treated as a valid binding, even if it happens to
// carry the executable permission bit, because spawning a Windows PE binary
// directly on native Linux would either fail outright or silently depend on
// a system-wide Wine/binfmt_misc association this crate never assumes,
// configures, or requires - see this module's own doc comment. A profile
// that only has `xenia_canary.exe` and no native binary is refused exactly
// like a profile with no executable at all; Windows/Wine execution remains
// entirely unsupported by this launch slice.
//
// Because Xenia profiles are always [`XeniaInstallationType::Explicit`]
// (there is no XDG-style default location to guess), this resolver has no
// installation-type branch to arbitrate - unlike PCSX2/DuckStation, there is
// exactly one directory to look in; only the base name is ambiguous, not
// the containing directory.

/// The native Linux executable names this resolver accepts, in preference
/// order - never `xenia_canary.exe`. See this section's own doc comment for
/// why the Windows binary is deliberately never a valid binding here.
const XENIA_NATIVE_LINUX_EXECUTABLE_CANDIDATES: &[&str] = &["xenia_canary", "xenia"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XeniaLaunchBlockerKind {
    /// The profile's configuration root (or its Xenia evidence) no longer
    /// matches what discovery originally observed, or the profile itself is
    /// not eligible.
    ProfileRootMismatch,
    /// Neither `xenia_canary` nor `xenia` (the native Linux executable
    /// names) exists in the profile's configuration directory - a
    /// `xenia_canary.exe` alone, if present, does not satisfy this.
    ExecutableMissing,
    /// A candidate native executable exists but is a symlink or not a
    /// regular file.
    ExecutableUnsafe,
    /// A candidate native executable exists as a regular file but lacks the
    /// executable permission bit.
    ExecutableNotExecutable,
    /// More than one native executable candidate matches and no authority
    /// distinguishes them.
    AmbiguousExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XeniaLaunchBlocker {
    pub kind: XeniaLaunchBlockerKind,
    pub detail: String,
}

fn launch_blocker(kind: XeniaLaunchBlockerKind, detail: impl Into<String>) -> XeniaLaunchBlocker {
    XeniaLaunchBlocker {
        kind,
        detail: detail.into(),
    }
}

/// A freshly proven executable/profile pairing, safe to use as the first
/// token of a native Xenia launch command. Must be re-derived, not cached:
/// call [`resolve_xenia_launch_binding`] again at the moment of launch,
/// exactly like every other adapter's equivalent binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XeniaLaunchBinding {
    pub executable: PathBuf,
}

fn is_real_directory_no_follow(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Freshly revalidates `profile` and either proves a launch binding or
/// returns a structured blocker. Pure and read-only: inspects only
/// filesystem metadata, never spawns a process, writes Xenia configuration,
/// or creates a directory. Safe - and intended - to call again at future
/// launch time.
pub fn resolve_xenia_launch_binding(
    profile: &XeniaProfile,
) -> Result<XeniaLaunchBinding, XeniaLaunchBlocker> {
    if !profile.eligible {
        return Err(launch_blocker(
            XeniaLaunchBlockerKind::ProfileRootMismatch,
            "profile is not eligible",
        ));
    }
    if !is_real_directory_no_follow(&profile.configuration_path).unwrap_or(false) {
        return Err(launch_blocker(
            XeniaLaunchBlockerKind::ProfileRootMismatch,
            "configuration root no longer matches the discovered profile",
        ));
    }
    if inspect_marker(&profile.configuration_path).is_err() {
        return Err(launch_blocker(
            XeniaLaunchBlockerKind::ProfileRootMismatch,
            "Xenia Canary configuration evidence (xenia-canary.config.toml) is no longer present",
        ));
    }
    let mut valid = Vec::new();
    let mut most_specific_error: Option<XeniaLaunchBlocker> = None;
    for name in XENIA_NATIVE_LINUX_EXECUTABLE_CANDIDATES {
        let candidate = profile.configuration_path.join(name);
        match validate_native_linux_xenia_executable(&candidate) {
            Ok(()) => valid.push(candidate),
            Err(error) => {
                // Prefer a real-but-unsafe/non-executable finding at one
                // candidate name over a plain "missing" from a different
                // candidate name that simply does not exist at all - never
                // let a later, less informative "missing" overwrite an
                // earlier, more specific refusal.
                let replace = match &most_specific_error {
                    None => true,
                    Some(existing) => {
                        existing.kind == XeniaLaunchBlockerKind::ExecutableMissing
                            && error.kind != XeniaLaunchBlockerKind::ExecutableMissing
                    }
                };
                if replace {
                    most_specific_error = Some(error);
                }
            }
        }
    }
    match valid.len() {
        0 => Err(most_specific_error.unwrap_or_else(|| {
            launch_blocker(
                XeniaLaunchBlockerKind::ExecutableMissing,
                "no native Linux Xenia executable (xenia_canary or xenia, without a .exe \
                 extension) was found in the profile's configuration directory - a \
                 xenia_canary.exe alone is never a valid native launch target",
            )
        })),
        1 => Ok(XeniaLaunchBinding {
            executable: valid.pop().expect("length checked above"),
        }),
        count => Err(launch_blocker(
            XeniaLaunchBlockerKind::AmbiguousExecutable,
            format!(
                "{count} viable native Xenia executables match this profile and none is \
                 distinguished as authoritative"
            ),
        )),
    }
}

fn validate_native_linux_xenia_executable(path: &Path) -> Result<(), XeniaLaunchBlocker> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(launch_blocker(
            XeniaLaunchBlockerKind::ExecutableUnsafe,
            format!("{} is a symlink", path.display()),
        )),
        Ok(metadata) if metadata.is_file() => {
            #[cfg(unix)]
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(launch_blocker(
                    XeniaLaunchBlockerKind::ExecutableNotExecutable,
                    format!("{} is not executable", path.display()),
                ));
            }
            Ok(())
        }
        Ok(_) => Err(launch_blocker(
            XeniaLaunchBlockerKind::ExecutableUnsafe,
            format!("{} is not a regular file", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(launch_blocker(
            XeniaLaunchBlockerKind::ExecutableMissing,
            format!("{} was not found", path.display()),
        )),
        Err(_) => Err(launch_blocker(
            XeniaLaunchBlockerKind::ExecutableMissing,
            format!("{} could not be inspected", path.display()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct FixtureDir(PathBuf);

    impl FixtureDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "archivefs-xenia-local-{label}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_real_xenia_directory_with_the_marker_file_is_eligible() {
        let root = FixtureDir::new("real");
        fs::write(root.0.join(XENIA_CONFIG_MARKER), "").unwrap();
        fs::create_dir_all(root.0.join("patches")).unwrap();
        let discovery = discover_xenia_profiles(&XeniaProfileDiscoveryRoots {
            explicit_configuration_roots: vec![root.0.clone()],
        });
        assert_eq!(discovery.profiles.len(), 1);
        let profile = &discovery.profiles[0];
        assert!(profile.eligible);
        assert_eq!(profile.patches_state, XeniaPatchesDirectoryState::Available);
    }

    #[test]
    fn a_directory_missing_the_marker_file_is_blocked_not_guessed() {
        let root = FixtureDir::new("no-marker");
        let discovery = discover_xenia_profiles(&XeniaProfileDiscoveryRoots {
            explicit_configuration_roots: vec![root.0.clone()],
        });
        assert_eq!(discovery.profiles.len(), 1);
        let profile = &discovery.profiles[0];
        assert!(!profile.eligible);
        assert_eq!(
            profile.blockers[0].kind,
            XeniaProfileBlockerKind::MissingXeniaEvidence
        );
    }

    #[test]
    fn a_missing_patches_directory_is_not_an_eligibility_blocker() {
        let root = FixtureDir::new("no-patches-dir");
        fs::write(root.0.join(XENIA_CONFIG_MARKER), "").unwrap();
        let discovery = discover_xenia_profiles(&XeniaProfileDiscoveryRoots {
            explicit_configuration_roots: vec![root.0.clone()],
        });
        let profile = &discovery.profiles[0];
        assert!(
            profile.eligible,
            "a game with no patches installed yet must still work"
        );
        assert_eq!(profile.patches_state, XeniaPatchesDirectoryState::Missing);
    }

    #[test]
    fn a_nonexistent_explicit_directory_is_reported_as_missing() {
        let root = FixtureDir::new("missing");
        let missing = root.0.join("does-not-exist");
        let discovery = discover_xenia_profiles(&XeniaProfileDiscoveryRoots {
            explicit_configuration_roots: vec![missing],
        });
        assert_eq!(discovery.profiles.len(), 1);
        assert!(!discovery.profiles[0].eligible);
        assert_eq!(
            discovery.profiles[0].blockers[0].kind,
            XeniaProfileBlockerKind::MissingConfiguration
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_patches_directory_is_rejected() {
        use std::os::unix::fs::symlink;
        let root = FixtureDir::new("symlinked-patches");
        fs::write(root.0.join(XENIA_CONFIG_MARKER), "").unwrap();
        let real = root.0.join("real-patches");
        fs::create_dir_all(&real).unwrap();
        symlink(&real, root.0.join("patches")).unwrap();
        let discovery = discover_xenia_profiles(&XeniaProfileDiscoveryRoots {
            explicit_configuration_roots: vec![root.0.clone()],
        });
        let profile = &discovery.profiles[0];
        assert!(profile.eligible);
        assert_eq!(
            profile.patches_state,
            XeniaPatchesDirectoryState::UnsafePath
        );
    }

    #[test]
    fn no_explicit_roots_means_no_profiles_and_no_fabricated_native_guess() {
        let discovery = discover_xenia_profiles(&XeniaProfileDiscoveryRoots::default());
        assert!(discovery.profiles.is_empty());
        assert!(discovery.complete);
    }

    // -----------------------------------------------------------------
    // Launch binding
    // -----------------------------------------------------------------

    fn eligible_profile(root: &Path) -> XeniaProfile {
        fs::write(root.join(XENIA_CONFIG_MARKER), "").unwrap();
        let discovery = discover_xenia_profiles(&XeniaProfileDiscoveryRoots {
            explicit_configuration_roots: vec![root.to_path_buf()],
        });
        discovery.profiles.into_iter().next().unwrap()
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, contents: &[u8]) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_real_native_executable_next_to_the_marker_resolves() {
        let root = FixtureDir::new("binding-real");
        let profile = eligible_profile(&root.0);
        write_executable(&root.0.join("xenia_canary"), b"binary");
        let binding = resolve_xenia_launch_binding(&profile).unwrap();
        assert_eq!(binding.executable, root.0.join("xenia_canary"));
    }

    #[cfg(unix)]
    #[test]
    fn the_mainline_xenia_native_name_also_resolves() {
        let root = FixtureDir::new("binding-mainline");
        let profile = eligible_profile(&root.0);
        write_executable(&root.0.join("xenia"), b"binary");
        let binding = resolve_xenia_launch_binding(&profile).unwrap();
        assert_eq!(binding.executable, root.0.join("xenia"));
    }

    #[test]
    fn a_missing_executable_is_refused_not_guessed() {
        let root = FixtureDir::new("binding-missing");
        let profile = eligible_profile(&root.0);
        let error = resolve_xenia_launch_binding(&profile).unwrap_err();
        assert_eq!(error.kind, XeniaLaunchBlockerKind::ExecutableMissing);
    }

    #[test]
    fn a_windows_exe_alone_is_never_authorized_as_a_native_binding() {
        // The Windows binary may genuinely exist in a real portable Xenia
        // Canary folder, but it must never become this resolver's binding -
        // this crate never assumes or configures Wine/Proton.
        let root = FixtureDir::new("binding-exe-only");
        let profile = eligible_profile(&root.0);
        fs::write(root.0.join("xenia_canary.exe"), b"MZ fake pe").unwrap();
        let error = resolve_xenia_launch_binding(&profile).unwrap_err();
        assert_eq!(error.kind, XeniaLaunchBlockerKind::ExecutableMissing);
    }

    #[cfg(unix)]
    #[test]
    fn an_executable_permission_windows_exe_is_still_never_authorized() {
        // Even if a `.exe` happens to carry the executable permission bit
        // (e.g. a Wine wrapper setup), it must never be treated as native
        // merely because it exists and is marked executable.
        let root = FixtureDir::new("binding-exe-chmod");
        let profile = eligible_profile(&root.0);
        write_executable(&root.0.join("xenia_canary.exe"), b"MZ fake pe");
        let error = resolve_xenia_launch_binding(&profile).unwrap_err();
        assert_eq!(error.kind, XeniaLaunchBlockerKind::ExecutableMissing);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_executable_is_refused() {
        use std::os::unix::fs::symlink;
        let root = FixtureDir::new("binding-symlink");
        let profile = eligible_profile(&root.0);
        let real = root.0.join("real-exe");
        write_executable(&real, b"binary");
        symlink(&real, root.0.join("xenia_canary")).unwrap();
        let error = resolve_xenia_launch_binding(&profile).unwrap_err();
        assert_eq!(error.kind, XeniaLaunchBlockerKind::ExecutableUnsafe);
    }

    #[cfg(unix)]
    #[test]
    fn a_non_executable_native_binary_is_refused() {
        let root = FixtureDir::new("binding-not-executable");
        let profile = eligible_profile(&root.0);
        fs::write(root.0.join("xenia_canary"), b"binary").unwrap();
        let error = resolve_xenia_launch_binding(&profile).unwrap_err();
        assert_eq!(error.kind, XeniaLaunchBlockerKind::ExecutableNotExecutable);
    }

    #[cfg(unix)]
    #[test]
    fn two_native_candidates_present_is_ambiguous() {
        let root = FixtureDir::new("binding-ambiguous");
        let profile = eligible_profile(&root.0);
        write_executable(&root.0.join("xenia_canary"), b"binary");
        write_executable(&root.0.join("xenia"), b"binary");
        let error = resolve_xenia_launch_binding(&profile).unwrap_err();
        assert_eq!(error.kind, XeniaLaunchBlockerKind::AmbiguousExecutable);
    }

    #[test]
    fn an_ineligible_profile_is_refused_before_any_executable_check() {
        let root = FixtureDir::new("binding-ineligible");
        // No marker file written: `discover_xenia_profiles` marks this
        // profile ineligible.
        let discovery = discover_xenia_profiles(&XeniaProfileDiscoveryRoots {
            explicit_configuration_roots: vec![root.0.clone()],
        });
        let profile = &discovery.profiles[0];
        assert!(!profile.eligible);
        let error = resolve_xenia_launch_binding(profile).unwrap_err();
        assert_eq!(error.kind, XeniaLaunchBlockerKind::ProfileRootMismatch);
    }

    #[test]
    fn a_directory_named_like_the_executable_is_refused() {
        let root = FixtureDir::new("binding-exe-is-dir");
        let profile = eligible_profile(&root.0);
        fs::create_dir_all(root.0.join("xenia_canary")).unwrap();
        let error = resolve_xenia_launch_binding(&profile).unwrap_err();
        assert_eq!(error.kind, XeniaLaunchBlockerKind::ExecutableUnsafe);
    }
}
