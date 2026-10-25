//! Manifest-backed ownership and side-by-side managed AppImage installs.
//!
//! This is deliberately a narrow M0/M1 boundary.  It reuses the existing
//! official release resolver and staged AppImage downloader, but publishes
//! into a dedicated EmuWiz-owned tree only after the artifact has a published
//! SHA-256.  Detection of an external install never creates ownership.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::emulator_download::{
    EmulatorDistribution, EmulatorDownloadOptions, EmulatorDownloadPlan, EmulatorDownloadTransport,
    download_and_install_resolved, emulator_download_spec, validate_appimage,
};
use crate::emulator_inventory::{BuildChannel, InstallationType};

pub const MANAGED_INSTALL_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MANAGED_INSTALL_MAX_MANIFEST_BYTES: u64 = 128 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ManagedOwnershipState {
    Managed,
    AdoptedManaged,
    DetectedUnmanaged,
    ExternalPackageManager,
    SystemInstall,
    FlatpakManagedExternally,
    UnknownProvenance,
    BrokenManagedState,
    StaleManifest,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct EmulatorInstallationId(pub String);

impl EmulatorInstallationId {
    fn generate(emulator_id: &str, version: &str, digest: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut hash = Sha256::new();
        hash.update(emulator_id.as_bytes());
        hash.update([0]);
        hash.update(version.as_bytes());
        hash.update([0]);
        hash.update(digest.as_bytes());
        hash.update(nonce.to_le_bytes());
        let suffix = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Self(format!("install-{}", &suffix[..24]))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedInstallManifest {
    pub manifest_schema_version: u32,
    pub emulator_id: String,
    pub install_id: EmulatorInstallationId,
    pub ownership_state: ManagedOwnershipState,
    pub install_type: InstallationType,
    pub installed_version: String,
    pub channel: BuildChannel,
    pub architecture: String,
    pub platform: String,
    pub install_root: PathBuf,
    pub executable_path: PathBuf,
    pub official_source: String,
    pub artifact_provenance: String,
    pub artifact_filename: String,
    pub artifact_size_bytes: u64,
    pub artifact_sha256: String,
    pub installed_executable_sha256: String,
    pub installation_timestamp_unix: u64,
    pub installing_emuwiz_version: Option<String>,
    pub previous_managed_install: Option<EmulatorInstallationId>,
    pub update_eligibility: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedInstallRecord {
    pub manifest: ManagedInstallManifest,
    pub ownership: ManagedOwnershipState,
    pub current: bool,
    pub manifest_path: PathBuf,
    pub executable_path: PathBuf,
    pub executable_hash_matches: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ManagedInstallHealth {
    Healthy,
    StaleManifest,
    BrokenManagedState,
    MissingExecutable,
    HashMismatch,
    InvalidManifest,
    OutsideManagedRoot,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ManagedInstallVersionRole {
    Current,
    Previous,
    Historical,
    BrokenReference,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ManagedCurrentPointerState {
    Valid,
    Missing,
    OutsideManagedRoot,
    MissingInstall,
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedInstallInventoryEntry {
    pub emulator_id: String,
    pub install_id: Option<EmulatorInstallationId>,
    pub installed_version: Option<String>,
    pub channel: Option<BuildChannel>,
    pub install_root: PathBuf,
    pub executable_path: Option<PathBuf>,
    pub ownership: ManagedOwnershipState,
    pub health: ManagedInstallHealth,
    pub role: ManagedInstallVersionRole,
    pub manifest: Option<ManagedInstallManifest>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ManagedInstallInventory {
    pub data_root: PathBuf,
    pub entries: Vec<ManagedInstallInventoryEntry>,
    pub pointer_states: Vec<(String, ManagedCurrentPointerState)>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ManagedInstallError {
    Unsupported(String),
    InvalidPath(String),
    Verification(String),
    Stale(String),
    Io(String),
    Manifest(String),
}

impl std::fmt::Display for ManagedInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(detail)
            | Self::InvalidPath(detail)
            | Self::Verification(detail)
            | Self::Stale(detail)
            | Self::Io(detail)
            | Self::Manifest(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for ManagedInstallError {}

pub fn managed_install_root(
    data_root: &Path,
    emulator_id: &str,
) -> Result<PathBuf, ManagedInstallError> {
    validate_component(emulator_id)?;
    if !data_root.is_absolute() {
        return Err(ManagedInstallError::InvalidPath(
            "managed data root must be absolute".into(),
        ));
    }
    Ok(data_root.join("emulators").join(emulator_id))
}

fn validate_component(value: &str) -> Result<(), ManagedInstallError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.bytes().any(|byte| byte == 0 || byte < 32)
    {
        return Err(ManagedInstallError::InvalidPath(format!(
            "unsafe managed-install path component: {value:?}"
        )));
    }
    Ok(())
}

pub fn ownership_for_installation_type(
    installation_type: InstallationType,
) -> ManagedOwnershipState {
    match installation_type {
        InstallationType::Flatpak => ManagedOwnershipState::FlatpakManagedExternally,
        InstallationType::SystemPackage => ManagedOwnershipState::SystemInstall,
        InstallationType::AppImage | InstallationType::Portable | InstallationType::Manual => {
            ManagedOwnershipState::DetectedUnmanaged
        }
        InstallationType::Managed => ManagedOwnershipState::UnknownProvenance,
        InstallationType::Unknown => ManagedOwnershipState::UnknownProvenance,
    }
}

fn install_base(root: &Path) -> PathBuf {
    root.join("installs")
}

fn current_path(root: &Path) -> PathBuf {
    root.join("current")
}

fn manifest_path(install_root: &Path) -> PathBuf {
    install_root.join("manifest.json")
}

/// Enumerate only manifests beneath the dedicated EmuWiz data root.  This is
/// deliberately non-recursive beyond `emulators/<id>/installs/<install>` and
/// performs at most one executable hash per valid install.
pub fn enumerate_managed_installations(data_root: &Path) -> ManagedInstallInventory {
    let mut inventory = ManagedInstallInventory {
        data_root: data_root.to_path_buf(),
        ..Default::default()
    };
    let emulators_root = data_root.join("emulators");
    let emulator_entries = match fs::read_dir(&emulators_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return inventory,
        Err(error) => {
            inventory
                .warnings
                .push(format!("managed emulator root could not be read: {error}"));
            return inventory;
        }
    };
    let mut emulator_dirs = emulator_entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .collect::<Vec<_>>();
    emulator_dirs.sort_by_key(|entry| entry.file_name());
    for emulator_entry in emulator_dirs {
        let emulator_id = emulator_entry.file_name().to_string_lossy().into_owned();
        if validate_component(&emulator_id).is_err() {
            inventory.warnings.push(format!(
                "ignored unsafe managed emulator directory: {}",
                emulator_entry.path().display()
            ));
            continue;
        }
        let emulator_root = emulator_entry.path();
        let (current_target, mut pointer_state) = match current_target(&emulator_root) {
            Ok(Some(path)) => (Some(path), ManagedCurrentPointerState::Valid),
            Ok(None) => (None, ManagedCurrentPointerState::Missing),
            Err(error) => {
                let state = if error.to_string().contains("outside") {
                    ManagedCurrentPointerState::OutsideManagedRoot
                } else {
                    ManagedCurrentPointerState::Invalid
                };
                inventory.warnings.push(format!(
                    "managed {emulator_id} current pointer is invalid: {error}"
                ));
                (None, state)
            }
        };
        let previous_id = current_target.as_ref().and_then(|path| {
            fs::read(manifest_path(path))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ManagedInstallManifest>(&bytes).ok())
                .and_then(|manifest| manifest.previous_managed_install)
        });
        let installs_root = install_base(&emulator_root);
        let install_entries = match fs::read_dir(&installs_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                inventory.warnings.push(format!(
                    "managed {emulator_id} install directory could not be read: {error}"
                ));
                continue;
            }
        };
        let mut install_dirs = install_entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .collect::<Vec<_>>();
        install_dirs.sort_by_key(|entry| entry.file_name());
        for install_entry in install_dirs {
            let install_root = install_entry.path();
            let path_to_manifest = manifest_path(&install_root);
            let role = if current_target.as_ref() == Some(&install_root) {
                ManagedInstallVersionRole::Current
            } else if previous_id.as_ref()
                == Some(&EmulatorInstallationId(
                    install_entry.file_name().to_string_lossy().into_owned(),
                ))
            {
                ManagedInstallVersionRole::Previous
            } else {
                ManagedInstallVersionRole::Historical
            };
            let bytes = match fs::symlink_metadata(&path_to_manifest) {
                Ok(metadata)
                    if metadata.is_file()
                        && !metadata.file_type().is_symlink()
                        && metadata.len() <= MANAGED_INSTALL_MAX_MANIFEST_BYTES =>
                {
                    match fs::read(&path_to_manifest) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            inventory.entries.push(invalid_entry(
                                &emulator_id,
                                &install_root,
                                role,
                                format!("cannot read manifest: {error}"),
                            ));
                            continue;
                        }
                    }
                }
                _ => {
                    inventory.entries.push(invalid_entry(
                        &emulator_id,
                        &install_root,
                        role,
                        "manifest is missing, unsafe, or too large".into(),
                    ));
                    continue;
                }
            };
            let manifest: ManagedInstallManifest = match serde_json::from_slice(&bytes) {
                Ok(manifest) => manifest,
                Err(error) => {
                    inventory.entries.push(invalid_entry(
                        &emulator_id,
                        &install_root,
                        role,
                        format!("manifest is invalid: {error}"),
                    ));
                    continue;
                }
            };
            let mut health = ManagedInstallHealth::Healthy;
            let executable = manifest.executable_path.clone();
            if manifest.manifest_schema_version != MANAGED_INSTALL_MANIFEST_SCHEMA_VERSION
                || manifest.ownership_state != ManagedOwnershipState::Managed
                || manifest.emulator_id != emulator_id
                || manifest.install_root != install_root
            {
                health = ManagedInstallHealth::BrokenManagedState;
            } else if !executable.starts_with(&install_root) {
                health = ManagedInstallHealth::OutsideManagedRoot;
            } else if fs::symlink_metadata(&executable).is_err() {
                health = ManagedInstallHealth::MissingExecutable;
            } else if let Ok((_, hash)) = executable_hash(&executable)
                && hash != manifest.installed_executable_sha256
            {
                health = ManagedInstallHealth::HashMismatch;
            } else if executable_hash(&executable).is_err() {
                health = ManagedInstallHealth::StaleManifest;
            }
            inventory.entries.push(ManagedInstallInventoryEntry {
                emulator_id: emulator_id.clone(),
                install_id: Some(manifest.install_id.clone()),
                installed_version: Some(manifest.installed_version.clone()),
                channel: Some(manifest.channel),
                install_root,
                executable_path: Some(executable),
                ownership: if health == ManagedInstallHealth::Healthy {
                    ManagedOwnershipState::Managed
                } else {
                    ManagedOwnershipState::BrokenManagedState
                },
                health,
                role,
                manifest: Some(manifest),
            });
        }
        if let Some(current_target) = current_target
            && !inventory
                .entries
                .iter()
                .any(|entry| entry.install_root == current_target)
        {
            pointer_state = ManagedCurrentPointerState::MissingInstall;
        }
        inventory.pointer_states.push((emulator_id, pointer_state));
    }
    inventory
}

fn invalid_entry(
    emulator_id: &str,
    install_root: &Path,
    role: ManagedInstallVersionRole,
    detail: String,
) -> ManagedInstallInventoryEntry {
    ManagedInstallInventoryEntry {
        emulator_id: emulator_id.into(),
        install_id: None,
        installed_version: Some(detail),
        channel: None,
        install_root: install_root.to_path_buf(),
        executable_path: None,
        ownership: ManagedOwnershipState::BrokenManagedState,
        health: ManagedInstallHealth::InvalidManifest,
        role,
        manifest: None,
    }
}

fn executable_hash(path: &Path) -> Result<(u64, String), ManagedInstallError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| ManagedInstallError::Io(error.to_string()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(ManagedInstallError::Verification(format!(
            "managed executable is not a regular file: {}",
            path.display()
        )));
    }
    let mut file = File::open(path).map_err(|error| ManagedInstallError::Io(error.to_string()))?;
    let mut hash = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ManagedInstallError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        hash.update(&buffer[..read]);
    }
    Ok((
        total,
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ManagedInstallError> {
    let parent = path
        .parent()
        .ok_or_else(|| ManagedInstallError::InvalidPath("manifest has no parent".into()))?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| ManagedInstallError::Io(error.to_string()))?;
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(ManagedInstallError::Io(error.to_string()));
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ManagedInstallError::Io(error.to_string())
    })
}

fn validate_owned_root(root: &Path) -> Result<(), ManagedInstallError> {
    if !root.is_absolute() {
        return Err(ManagedInstallError::InvalidPath(
            "managed root must be absolute".into(),
        ));
    }
    for ancestor in root.ancestors().filter(|path| path.parent().is_some()) {
        if let Ok(metadata) = fs::symlink_metadata(ancestor)
            && (metadata.file_type().is_symlink() || (!metadata.is_dir() && ancestor != root))
        {
            return Err(ManagedInstallError::InvalidPath(format!(
                "managed root contains an unsafe ancestor: {}",
                ancestor.display()
            )));
        }
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<(), ManagedInstallError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(ManagedInstallError::InvalidPath(format!(
                "managed path is not a real directory: {}",
                path.display()
            )))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| ManagedInstallError::Io(error.to_string()))
        }
        Err(error) => Err(ManagedInstallError::Io(error.to_string())),
    }
}

fn install_id_path(
    root: &Path,
    id: &EmulatorInstallationId,
) -> Result<PathBuf, ManagedInstallError> {
    validate_component(&id.0)?;
    Ok(install_base(root).join(&id.0))
}

fn current_target(root: &Path) -> Result<Option<PathBuf>, ManagedInstallError> {
    let current = current_path(root);
    let target = match fs::read_link(&current) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ManagedInstallError::Manifest(error.to_string())),
    };
    if target.is_absolute()
        || target
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ManagedInstallError::Stale(
            "current pointer escapes managed root".into(),
        ));
    }
    let resolved = root.join(target);
    let installs = install_base(root);
    if !resolved.starts_with(&installs) {
        return Err(ManagedInstallError::Stale(
            "current pointer is outside installs".into(),
        ));
    }
    Ok(Some(resolved))
}

/// Read-only managed-install inspection.  A changed executable degrades the
/// record to `STALE_MANIFEST`; it is never overwritten automatically.
pub fn inspect_managed_install(
    root: &Path,
) -> Result<Option<ManagedInstallRecord>, ManagedInstallError> {
    validate_owned_root(root)?;
    let Some(install_root) = current_target(root)? else {
        return Ok(None);
    };
    let manifest_file = manifest_path(&install_root);
    let metadata = fs::symlink_metadata(&manifest_file)
        .map_err(|error| ManagedInstallError::Manifest(error.to_string()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MANAGED_INSTALL_MAX_MANIFEST_BYTES
    {
        return Err(ManagedInstallError::Manifest(
            "managed manifest is unsafe or too large".into(),
        ));
    }
    let bytes =
        fs::read(&manifest_file).map_err(|error| ManagedInstallError::Io(error.to_string()))?;
    let manifest: ManagedInstallManifest = serde_json::from_slice(&bytes)
        .map_err(|error| ManagedInstallError::Manifest(error.to_string()))?;
    if manifest.manifest_schema_version != MANAGED_INSTALL_MANIFEST_SCHEMA_VERSION
        || manifest.ownership_state != ManagedOwnershipState::Managed
        || manifest.install_root != install_root
        || !manifest.executable_path.starts_with(&install_root)
    {
        return Ok(Some(ManagedInstallRecord {
            manifest,
            ownership: ManagedOwnershipState::BrokenManagedState,
            current: true,
            manifest_path: manifest_file,
            executable_path: install_root,
            executable_hash_matches: false,
        }));
    }
    let (size, hash) = executable_hash(&manifest.executable_path)?;
    let matches = size > 0 && hash == manifest.installed_executable_sha256;
    Ok(Some(ManagedInstallRecord {
        ownership: if matches {
            ManagedOwnershipState::Managed
        } else {
            ManagedOwnershipState::StaleManifest
        },
        manifest,
        current: true,
        manifest_path: manifest_file,
        executable_path: install_root,
        executable_hash_matches: matches,
    }))
}

/// Publish one verified release into the EmuWiz-owned side-by-side tree.
/// The existing downloader performs the bounded HTTPS transfer and checksum
/// check in a disposable staging root; this function performs only the
/// manifest-backed publication and current-pointer switch.
pub fn install_managed_appimage(
    root: &Path,
    plan: &EmulatorDownloadPlan,
    transport: &dyn EmulatorDownloadTransport,
    options: &EmulatorDownloadOptions,
    installing_emuwiz_version: Option<String>,
) -> Result<ManagedInstallManifest, ManagedInstallError> {
    let spec = emulator_download_spec(&plan.emulator_id).ok_or_else(|| {
        ManagedInstallError::Unsupported("unknown emulator download specification".into())
    })?;
    if plan.distribution != EmulatorDistribution::GithubAppImage || plan.expected_sha256.is_none() {
        return Err(ManagedInstallError::Verification(
            "managed installation requires an official AppImage and published SHA-256".into(),
        ));
    }
    validate_owned_root(root)?;
    let root = root.to_path_buf();
    let temp_path = std::env::temp_dir().join(format!(
        "emuwiz-managed-install-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir(&temp_path).map_err(|error| ManagedInstallError::Io(error.to_string()))?;
    let mut staged_plan = plan.clone();
    staged_plan.destination_path =
        crate::emulator_download::managed_appimage_destination(&temp_path, spec);
    let receipt = download_and_install_resolved(&temp_path, &staged_plan, transport, options)
        .map_err(|error| ManagedInstallError::Io(error.to_string()));
    let receipt = match receipt {
        Ok(receipt) => receipt,
        Err(error) => {
            let _ = fs::remove_dir_all(&temp_path);
            return Err(error);
        }
    };
    let bytes = fs::read(&receipt.installed_path).map_err(|error| {
        let _ = fs::remove_dir_all(&temp_path);
        ManagedInstallError::Io(error.to_string())
    })?;
    let _ = fs::remove_dir_all(&temp_path);
    let digest = validate_appimage(&bytes)
        .map_err(|error| ManagedInstallError::Verification(error.to_string()))?;
    if Some(digest.as_str()) != plan.expected_sha256.as_deref() {
        return Err(ManagedInstallError::Verification(
            "staged artifact digest changed".into(),
        ));
    }

    ensure_directory(&root)?;
    let emulator_root = root.join("emulators");
    ensure_directory(&emulator_root)?;
    let owned_root = emulator_root.join(&plan.emulator_id);
    ensure_directory(&owned_root)?;
    let installs = install_base(&owned_root);
    ensure_directory(&installs)?;
    let install_id =
        EmulatorInstallationId::generate(&plan.emulator_id, &plan.release_tag, &digest);
    let install_root = install_id_path(&owned_root, &install_id)?;
    if fs::symlink_metadata(&install_root).is_ok() {
        return Err(ManagedInstallError::Io(
            "generated managed install ID already exists".into(),
        ));
    }
    fs::create_dir(&install_root).map_err(|error| ManagedInstallError::Io(error.to_string()))?;
    let binary_path = install_root.join(spec.installed_binary);
    let binary_temp = install_root.join(format!(".{}.staged", spec.installed_binary));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&binary_temp)
            .map_err(|error| ManagedInstallError::Io(error.to_string()))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| ManagedInstallError::Io(error.to_string()))?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary_temp, fs::Permissions::from_mode(0o755))
                .map_err(|error| ManagedInstallError::Io(error.to_string()))?;
        }
        fs::rename(&binary_temp, &binary_path)
            .map_err(|error| ManagedInstallError::Io(error.to_string()))?;
        let (size, installed_hash) = executable_hash(&binary_path)?;
        if installed_hash != digest || size != bytes.len() as u64 {
            return Err(ManagedInstallError::Verification(
                "published executable verification failed".into(),
            ));
        }
        let previous = current_target(&owned_root)?
            .and_then(|path| {
                serde_json::from_slice::<ManagedInstallManifest>(
                    &fs::read(manifest_path(&path)).ok()?,
                )
                .ok()
            })
            .map(|manifest| manifest.install_id);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let manifest = ManagedInstallManifest {
            manifest_schema_version: MANAGED_INSTALL_MANIFEST_SCHEMA_VERSION,
            emulator_id: plan.emulator_id.clone(),
            install_id,
            ownership_state: ManagedOwnershipState::Managed,
            install_type: InstallationType::Managed,
            installed_version: plan.release_tag.clone(),
            channel: BuildChannel::Stable,
            architecture: "x86_64".into(),
            platform: "linux".into(),
            install_root: install_root.clone(),
            executable_path: binary_path,
            official_source: plan.project_url.clone(),
            artifact_provenance: plan.asset_url.clone(),
            artifact_filename: plan.asset_name.clone(),
            artifact_size_bytes: bytes.len() as u64,
            artifact_sha256: digest.clone(),
            installed_executable_sha256: installed_hash,
            installation_timestamp_unix: timestamp,
            installing_emuwiz_version,
            previous_managed_install: previous,
            update_eligibility: "provenance_verified".into(),
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| ManagedInstallError::Manifest(error.to_string()))?;
        write_atomic(&manifest_path(&install_root), &manifest_bytes)?;
        let pointer_temp = owned_root.join(format!(".current.tmp-{}", std::process::id()));
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            Path::new("installs").join(&manifest.install_id.0),
            &pointer_temp,
        )
        .map_err(|error| ManagedInstallError::Io(error.to_string()))?;
        #[cfg(not(unix))]
        return Err(ManagedInstallError::Unsupported(
            "managed current pointer requires symbolic links".into(),
        ));
        fs::rename(&pointer_temp, current_path(&owned_root))
            .map_err(|error| ManagedInstallError::Io(error.to_string()))?;
        Ok(manifest)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&binary_temp);
        let _ = fs::remove_dir_all(&install_root);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(
        root: &Path,
        id: &str,
        version: &str,
        previous: Option<EmulatorInstallationId>,
    ) -> ManagedInstallManifest {
        let install_root = root.join("installs").join(id);
        let executable_path = install_root.join("pcsx2.AppImage");
        let (_, digest) = executable_hash(&executable_path).unwrap();
        ManagedInstallManifest {
            manifest_schema_version: MANAGED_INSTALL_MANIFEST_SCHEMA_VERSION,
            emulator_id: "pcsx2".into(),
            install_id: EmulatorInstallationId(id.into()),
            ownership_state: ManagedOwnershipState::Managed,
            install_type: InstallationType::Managed,
            installed_version: version.into(),
            channel: BuildChannel::Unknown,
            architecture: "x86_64".into(),
            platform: "linux".into(),
            install_root,
            executable_path,
            official_source: "https://pcsx2.net".into(),
            artifact_provenance: "https://example.invalid/pcsx2.AppImage".into(),
            artifact_filename: "pcsx2.AppImage".into(),
            artifact_size_bytes: 4,
            artifact_sha256: digest.clone(),
            installed_executable_sha256: digest,
            installation_timestamp_unix: 1,
            installing_emuwiz_version: None,
            previous_managed_install: previous,
            update_eligibility: "provenance_verified".into(),
        }
    }

    fn write_install(
        root: &Path,
        id: &str,
        version: &str,
        previous: Option<EmulatorInstallationId>,
    ) {
        let install_root = root.join("installs").join(id);
        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("pcsx2.AppImage"), b"appimage").unwrap();
        let manifest = manifest(root, id, version, previous);
        fs::write(
            install_root.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn external_install_types_never_become_managed() {
        assert_eq!(
            ownership_for_installation_type(InstallationType::Flatpak),
            ManagedOwnershipState::FlatpakManagedExternally
        );
        assert_eq!(
            ownership_for_installation_type(InstallationType::AppImage),
            ManagedOwnershipState::DetectedUnmanaged
        );
        assert_eq!(
            ownership_for_installation_type(InstallationType::SystemPackage),
            ManagedOwnershipState::SystemInstall
        );
    }

    #[test]
    fn managed_root_rejects_traversal_and_relative_roots() {
        assert!(managed_install_root(Path::new("/tmp/emuwiz"), "pcsx2").is_ok());
        assert!(managed_install_root(Path::new("relative"), "pcsx2").is_err());
        assert!(managed_install_root(Path::new("/tmp/emuwiz"), "../escape").is_err());
    }

    #[test]
    fn missing_current_is_read_only_empty_inventory() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(inspect_managed_install(root.path()).unwrap(), None);
    }

    #[test]
    #[cfg(unix)]
    fn enumeration_preserves_current_previous_and_history() {
        let root = tempfile::tempdir().unwrap();
        let managed = root.path().join("emulators/pcsx2");
        fs::create_dir_all(managed.join("installs")).unwrap();
        write_install(&managed, "old", "1.0", None);
        write_install(
            &managed,
            "new",
            "2.0",
            Some(EmulatorInstallationId("old".into())),
        );
        std::os::unix::fs::symlink("installs/new", managed.join("current")).unwrap();
        let inventory = enumerate_managed_installations(root.path());
        assert_eq!(inventory.entries.len(), 2);
        assert_eq!(
            inventory
                .entries
                .iter()
                .find(|entry| entry.install_id == Some(EmulatorInstallationId("new".into())))
                .unwrap()
                .role,
            ManagedInstallVersionRole::Current
        );
        assert_eq!(
            inventory
                .entries
                .iter()
                .find(|entry| entry.install_id == Some(EmulatorInstallationId("old".into())))
                .unwrap()
                .role,
            ManagedInstallVersionRole::Previous
        );
    }

    #[test]
    #[cfg(unix)]
    fn enumeration_marks_changed_executable_without_repairing_it() {
        let root = tempfile::tempdir().unwrap();
        let managed = root.path().join("emulators/pcsx2");
        fs::create_dir_all(managed.join("installs")).unwrap();
        write_install(&managed, "one", "1.0", None);
        std::os::unix::fs::symlink("installs/one", managed.join("current")).unwrap();
        fs::write(managed.join("installs/one/pcsx2.AppImage"), b"changed").unwrap();
        let inventory = enumerate_managed_installations(root.path());
        assert_eq!(
            inventory.entries[0].health,
            ManagedInstallHealth::HashMismatch
        );
        assert_eq!(
            fs::read(managed.join("installs/one/pcsx2.AppImage")).unwrap(),
            b"changed"
        );
    }
}
