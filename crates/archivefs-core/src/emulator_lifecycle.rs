//! Provider-neutral, read-only emulator lifecycle health.
//!
//! This module is a composition boundary. It does not replace inventory,
//! profiles, readiness, managed-install manifests, or update metadata. Those
//! systems remain the authorities; this projection keeps their evidence
//! together, including every installation candidate and the exact selected
//! binding. Package managers and Flatpak are queried only for bounded,
//! read-only ownership evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::emulator_inventory::{
    BuildChannel, EmulatorInstallation, EmulatorInventory, InstallationType, InventoryEmulator,
    VersionConfidence, VersionSource,
};
use crate::emulator_update::{UpdateReport, UpdateStatus};
use crate::launch::readiness::LaunchReadiness;
use crate::managed_emulator_install::{
    ManagedInstallHealth, ManagedInstallInventory, ManagedInstallInventoryEntry,
};

pub const LIFECYCLE_MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const LIFECYCLE_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
pub const LIFECYCLE_SCHEMA_VERSION: u32 = 1;

/// Stable, human-facing identity. New adapters can be represented without
/// expanding the older inventory enum or creating a second scanner.
pub type EmulatorId = String;

pub const SUPPORTED_EMULATOR_IDS: &[&str] = &[
    "RetroArch",
    "PCSX2",
    "DuckStation",
    "RPCS3",
    "PPSSPP",
    "Dolphin",
    "Flycast",
    "xemu",
    "Cemu",
    "Vita3K",
    "MAME",
    "FBNeo",
    "Hatari",
    "FS-UAE",
    "Ryujinx",
    "Xenia",
];

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ExactBinding {
    NativeExecutable {
        path: PathBuf,
    },
    FlatpakApp {
        app_id: String,
    },
    ManagedInstall {
        manifest_path: PathBuf,
        executable_path: PathBuf,
    },
    PortableExecutable {
        path: PathBuf,
    },
    UnknownExternal {
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum OwnershipCategory {
    OfficialManaged,
    OfficialBrowserHandoff,
    FlatpakManaged,
    SystemPackageManaged,
    PortableUserManaged,
    Unknown,
    DoNotAutomate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum LifecycleState {
    InstalledCurrent,
    InstalledUpdateAvailable,
    InstalledUnknownVersion,
    InstalledUnsupportedVersion,
    Missing,
    Broken,
    MultipleInstallations,
    ManagedExternally,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum LifecycleChannel {
    Stable,
    Development,
    Nightly,
    ActionBuild,
    Distro,
    Unknown,
    Custom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum LocalHealth {
    Healthy,
    MissingExecutable,
    ChangedExecutable,
    ManifestMismatch,
    PermissionsProblem,
    UnknownVersion,
    StaleConfiguredPath,
    MultipleCandidates,
    BrokenProfile,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum UpdateAuthority {
    EmuWizManaged,
    Flatpak,
    SystemPackageManager,
    OfficialBrowser,
    UserManaged,
    None,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VersionEvidence {
    pub version: Option<String>,
    pub raw_output: Option<String>,
    pub source: VersionSource,
    pub confidence: VersionConfidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LifecycleProvenance {
    pub discovered_by: Vec<String>,
    pub selected_by: Vec<String>,
    pub package_manager: Option<String>,
    pub flatpak_scope: Option<String>,
    pub metadata_timestamp_unix: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackageEvidence {
    pub manager: String,
    pub package_name: String,
    pub package_version: Option<String>,
    pub executable_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FlatpakInstallation {
    pub app_id: String,
    pub installed_ref: Option<String>,
    pub version: Option<String>,
    pub branch: Option<String>,
    pub scope: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmulatorLifecycleInstallation {
    pub emulator_id: EmulatorId,
    pub exact_binding: ExactBinding,
    pub installation_type: InstallationType,
    pub ownership_category: OwnershipCategory,
    pub version: VersionEvidence,
    pub channel: LifecycleChannel,
    pub local_health: LocalHealth,
    /// This is an existing adapter result, never recomputed here.
    pub launch_readiness: Option<LaunchReadiness>,
    pub package: Option<PackageEvidence>,
    pub update_authority: UpdateAuthority,
    pub update_status: Option<UpdateStatus>,
    pub selected: bool,
    pub provenance: LifecycleProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmulatorLifecycleProjection {
    pub schema_version: u32,
    pub emulator_id: EmulatorId,
    pub state: LifecycleState,
    pub selected: Option<ExactBinding>,
    pub stale_selected: bool,
    pub installations: Vec<EmulatorLifecycleInstallation>,
}

#[derive(Clone, Debug, Default)]
pub struct LifecycleContext {
    pub inventory: EmulatorInventory,
    pub managed: ManagedInstallInventory,
    pub updates: UpdateReport,
    /// Existing selection/profile bindings, keyed by canonical emulator ID.
    pub selected_bindings: BTreeMap<EmulatorId, ExactBinding>,
    /// Existing adapter readiness projections, keyed by canonical emulator ID.
    pub launch_readiness: BTreeMap<EmulatorId, LaunchReadiness>,
    /// Raw output from the existing bounded inventory probe, when a caller
    /// retained it for the lifecycle projection.
    pub raw_version_output: BTreeMap<PathBuf, String>,
    /// Results from the bounded Flatpak provider. App IDs are retained as the
    /// binding; they are never flattened into fake executable paths.
    pub flatpak_installations: Vec<FlatpakInstallation>,
    /// Exact package ownership evidence keyed by an already discovered path.
    pub package_evidence: BTreeMap<PathBuf, PackageEvidence>,
    pub offline: bool,
}

impl LifecycleContext {
    pub fn from_inventory(inventory: EmulatorInventory) -> Self {
        let managed = ManagedInstallInventory {
            data_root: PathBuf::new(),
            entries: inventory.managed_installations.clone(),
            ..Default::default()
        };
        Self {
            inventory,
            managed,
            ..Default::default()
        }
    }
}

pub fn canonical_id(emulator: InventoryEmulator) -> EmulatorId {
    emulator.label().to_string()
}

fn channel(channel: BuildChannel) -> LifecycleChannel {
    match channel {
        BuildChannel::Stable => LifecycleChannel::Stable,
        BuildChannel::Development | BuildChannel::Beta | BuildChannel::Canary => {
            LifecycleChannel::Development
        }
        BuildChannel::Nightly => LifecycleChannel::Nightly,
        BuildChannel::Custom => LifecycleChannel::Custom,
        BuildChannel::Unknown => LifecycleChannel::Unknown,
    }
}

fn binding_for(installation: &EmulatorInstallation) -> ExactBinding {
    match installation.installation_type {
        InstallationType::Flatpak => ExactBinding::FlatpakApp {
            app_id: canonical_id(installation.emulator),
        },
        InstallationType::Managed => ExactBinding::ManagedInstall {
            manifest_path: installation.installation_root.join("manifest.json"),
            executable_path: installation.executable_path.clone(),
        },
        InstallationType::AppImage | InstallationType::Portable => {
            ExactBinding::PortableExecutable {
                path: installation.executable_path.clone(),
            }
        }
        InstallationType::SystemPackage | InstallationType::Manual => {
            ExactBinding::NativeExecutable {
                path: installation.executable_path.clone(),
            }
        }
        InstallationType::Unknown => ExactBinding::UnknownExternal {
            path: installation.executable_path.clone(),
        },
    }
}

fn ownership(installation: &EmulatorInstallation) -> OwnershipCategory {
    match installation.installation_type {
        InstallationType::Managed => OwnershipCategory::OfficialManaged,
        InstallationType::Flatpak => OwnershipCategory::FlatpakManaged,
        InstallationType::SystemPackage => OwnershipCategory::SystemPackageManaged,
        InstallationType::AppImage | InstallationType::Portable | InstallationType::Manual => {
            OwnershipCategory::PortableUserManaged
        }
        InstallationType::Unknown => OwnershipCategory::Unknown,
    }
}

fn authority(installation: &EmulatorInstallation) -> UpdateAuthority {
    match installation.installation_type {
        InstallationType::Managed => UpdateAuthority::EmuWizManaged,
        InstallationType::Flatpak => UpdateAuthority::Flatpak,
        InstallationType::SystemPackage => UpdateAuthority::SystemPackageManager,
        InstallationType::AppImage | InstallationType::Portable | InstallationType::Manual => {
            UpdateAuthority::UserManaged
        }
        InstallationType::Unknown => UpdateAuthority::Unknown,
    }
}

fn restricted_authority(emulator_id: &str, installation: &EmulatorInstallation) -> UpdateAuthority {
    if matches!(emulator_id, "Ryujinx" | "Xenia") {
        UpdateAuthority::None
    } else {
        authority(installation)
    }
}

fn restricted_ownership(
    emulator_id: &str,
    installation: &EmulatorInstallation,
) -> OwnershipCategory {
    if matches!(emulator_id, "Ryujinx" | "Xenia") {
        OwnershipCategory::DoNotAutomate
    } else {
        ownership(installation)
    }
}

fn update_for(path: &Path, updates: &UpdateReport) -> Option<UpdateStatus> {
    updates
        .results
        .iter()
        .find(|result| result.executable_path == path)
        .map(|r| r.status)
}

fn local_health(path: &Path, installation_type: InstallationType) -> LocalHealth {
    let metadata = fs::symlink_metadata(path);
    if metadata.as_ref().is_err() {
        return LocalHealth::MissingExecutable;
    }
    let metadata = metadata.expect("checked above");
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return LocalHealth::ChangedExecutable;
    }
    if !is_executable(&metadata) {
        return LocalHealth::PermissionsProblem;
    }
    let _ = installation_type;
    LocalHealth::Healthy
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}
#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    true
}

fn installation_record(
    installation: &EmulatorInstallation,
    context: &LifecycleContext,
    selected: bool,
) -> EmulatorLifecycleInstallation {
    let id = canonical_id(installation.emulator);
    let installation_ownership = restricted_ownership(&id, installation);
    let installation_authority = restricted_authority(&id, installation);
    let package = context
        .package_evidence
        .get(&installation.executable_path)
        .cloned();
    let package_manager = package.as_ref().map(|item| item.manager.clone());
    EmulatorLifecycleInstallation {
        emulator_id: id,
        exact_binding: binding_for(installation),
        installation_type: installation.installation_type,
        ownership_category: installation_ownership,
        version: VersionEvidence {
            version: installation.version.clone(),
            raw_output: context
                .raw_version_output
                .get(&installation.executable_path)
                .cloned(),
            source: installation.version_source,
            confidence: installation.version_confidence,
        },
        channel: channel(installation.channel),
        local_health: local_health(
            &installation.executable_path,
            installation.installation_type,
        ),
        launch_readiness: context
            .launch_readiness
            .get(&canonical_id(installation.emulator))
            .copied(),
        package,
        update_authority: installation_authority,
        update_status: update_for(&installation.executable_path, &context.updates),
        selected,
        provenance: LifecycleProvenance {
            discovered_by: vec!["emulator_inventory".into()],
            selected_by: if selected {
                vec!["emulator_inventory.preferred_or_profile".into()]
            } else {
                Vec::new()
            },
            package_manager,
            flatpak_scope: None,
            metadata_timestamp_unix: None,
        },
    }
}

fn flatpak_emulator_id(app_id: &str) -> Option<&'static str> {
    match app_id {
        "org.libretro.RetroArch" => Some("RetroArch"),
        "net.pcsx2.PCSX2" => Some("PCSX2"),
        "org.DolphinEmu.dolphin-emu" => Some("Dolphin"),
        "org.flycast.Flycast" => Some("Flycast"),
        "app.xemu.xemu" => Some("xemu"),
        "org.ppsspp.PPSSPP" => Some("PPSSPP"),
        "org.duckstation.DuckStation" => Some("DuckStation"),
        "net.rpcs3.RPCS3" => Some("RPCS3"),
        "info.cemu.Cemu" => Some("Cemu"),
        "io.github.ryubing.Ryujinx" => Some("Ryujinx"),
        "com.xenia_project.Xenia" => Some("Xenia"),
        _ => None,
    }
}

fn flatpak_channel(branch: Option<&str>) -> LifecycleChannel {
    match branch.unwrap_or_default().to_ascii_lowercase().as_str() {
        "stable" | "master" => LifecycleChannel::Stable,
        value if value.contains("nightly") => LifecycleChannel::Nightly,
        value if value.contains("beta") || value.contains("dev") => LifecycleChannel::Development,
        "" => LifecycleChannel::Unknown,
        _ => LifecycleChannel::Custom,
    }
}

fn flatpak_record(
    app: &FlatpakInstallation,
    context: &LifecycleContext,
    selected: bool,
) -> Option<EmulatorLifecycleInstallation> {
    let emulator_id = flatpak_emulator_id(&app.app_id)?.to_string();
    let restricted = matches!(emulator_id.as_str(), "Ryujinx" | "Xenia");
    Some(EmulatorLifecycleInstallation {
        emulator_id,
        exact_binding: ExactBinding::FlatpakApp {
            app_id: app.app_id.clone(),
        },
        installation_type: InstallationType::Flatpak,
        ownership_category: if restricted {
            OwnershipCategory::DoNotAutomate
        } else {
            OwnershipCategory::FlatpakManaged
        },
        version: VersionEvidence {
            version: app.version.clone(),
            raw_output: None,
            source: VersionSource::PackageMetadata,
            confidence: VersionConfidence::PackageMetadata,
        },
        channel: flatpak_channel(app.branch.as_deref()),
        local_health: LocalHealth::Healthy,
        launch_readiness: context
            .launch_readiness
            .get(flatpak_emulator_id(&app.app_id)?)
            .copied(),
        package: None,
        update_authority: if restricted {
            UpdateAuthority::None
        } else {
            UpdateAuthority::Flatpak
        },
        update_status: None,
        selected,
        provenance: LifecycleProvenance {
            discovered_by: vec!["flatpak list --app".into()],
            selected_by: if selected {
                vec!["profile_or_user_selection".into()]
            } else {
                Vec::new()
            },
            package_manager: Some("flatpak".into()),
            flatpak_scope: app.scope.clone(),
            metadata_timestamp_unix: None,
        },
    })
}

fn managed_record(
    entry: &ManagedInstallInventoryEntry,
    context: &LifecycleContext,
    selected: bool,
) -> Option<EmulatorLifecycleInstallation> {
    let executable = entry.executable_path.clone()?;
    let install_type = entry
        .manifest
        .as_ref()
        .map(|m| m.install_type)
        .unwrap_or(InstallationType::Managed);
    let health = match entry.health {
        ManagedInstallHealth::Healthy => local_health(&executable, InstallationType::Managed),
        ManagedInstallHealth::MissingExecutable => LocalHealth::MissingExecutable,
        ManagedInstallHealth::HashMismatch => LocalHealth::ChangedExecutable,
        ManagedInstallHealth::StaleManifest => LocalHealth::StaleConfiguredPath,
        ManagedInstallHealth::BrokenManagedState | ManagedInstallHealth::InvalidManifest => {
            LocalHealth::ManifestMismatch
        }
        ManagedInstallHealth::OutsideManagedRoot => LocalHealth::BrokenProfile,
    };
    let manifest_path = executable.parent()?.join("manifest.json");
    Some(EmulatorLifecycleInstallation {
        emulator_id: entry.emulator_id.clone(),
        exact_binding: ExactBinding::ManagedInstall {
            manifest_path,
            executable_path: executable,
        },
        installation_type: install_type,
        ownership_category: OwnershipCategory::OfficialManaged,
        version: VersionEvidence {
            version: entry.installed_version.clone(),
            raw_output: None,
            source: VersionSource::Profile,
            confidence: VersionConfidence::ProfileEvidence,
        },
        channel: channel(entry.channel.unwrap_or(BuildChannel::Unknown)),
        local_health: health,
        launch_readiness: context.launch_readiness.get(&entry.emulator_id).copied(),
        package: None,
        update_authority: UpdateAuthority::EmuWizManaged,
        update_status: None,
        selected,
        provenance: LifecycleProvenance {
            discovered_by: vec!["managed_emulator_install".into()],
            selected_by: if selected {
                vec!["managed_install_current_pointer".into()]
            } else {
                Vec::new()
            },
            package_manager: None,
            flatpak_scope: None,
            metadata_timestamp_unix: entry
                .manifest
                .as_ref()
                .map(|m| m.installation_timestamp_unix),
        },
    })
}

/// Compose existing inventory, managed installs, update results, profile
/// selection, and adapter readiness. A configured selection is never replaced
/// by a surviving alternative.
pub fn inspect_all_emulator_lifecycles(
    context: &LifecycleContext,
) -> Vec<EmulatorLifecycleProjection> {
    let mut grouped: BTreeMap<EmulatorId, Vec<EmulatorLifecycleInstallation>> = BTreeMap::new();
    for installation in &context.inventory.installations {
        let id = canonical_id(installation.emulator);
        let binding = binding_for(installation);
        let selected = context
            .selected_bindings
            .get(&id)
            .map(|chosen| chosen == &binding)
            .unwrap_or(installation.preferred == Some(true));
        grouped
            .entry(id)
            .or_default()
            .push(installation_record(installation, context, selected));
    }
    for entry in &context.managed.entries {
        let Some(binding_record) = managed_record(entry, context, false) else {
            continue;
        };
        let selected = context
            .selected_bindings
            .get(&entry.emulator_id)
            .map(|chosen| chosen == &binding_record.exact_binding)
            .unwrap_or(false);
        grouped
            .entry(entry.emulator_id.clone())
            .or_default()
            .push(EmulatorLifecycleInstallation {
                selected,
                ..binding_record
            });
    }
    for app in &context.flatpak_installations {
        let Some(id) = flatpak_emulator_id(&app.app_id) else {
            continue;
        };
        let binding = ExactBinding::FlatpakApp {
            app_id: app.app_id.clone(),
        };
        let selected = context
            .selected_bindings
            .get(id)
            .map(|chosen| chosen == &binding)
            .unwrap_or(false);
        if let Some(record) = flatpak_record(app, context, selected) {
            grouped.entry(id.to_string()).or_default().push(record);
        }
    }
    let mut result = Vec::new();
    for (id, mut installations) in grouped {
        installations.sort_by(|a, b| a.exact_binding.cmp(&b.exact_binding));
        let has_selection = context.selected_bindings.contains_key(&id);
        let selected = installations
            .iter()
            .find(|item| item.selected)
            .map(|item| item.exact_binding.clone());
        let stale_selected = has_selection && selected.is_none();
        let state = if stale_selected {
            LifecycleState::Broken
        } else if installations.len() > 1 {
            LifecycleState::MultipleInstallations
        } else if installations.is_empty() {
            LifecycleState::Missing
        } else if installations.iter().any(|i| {
            matches!(
                i.local_health,
                LocalHealth::MissingExecutable
                    | LocalHealth::ChangedExecutable
                    | LocalHealth::ManifestMismatch
                    | LocalHealth::BrokenProfile
            )
        }) {
            LifecycleState::Broken
        } else if installations.iter().any(|i| i.version.version.is_none()) {
            LifecycleState::InstalledUnknownVersion
        } else if installations
            .iter()
            .any(|i| i.update_status == Some(UpdateStatus::UpdateAvailable))
        {
            LifecycleState::InstalledUpdateAvailable
        } else if installations.iter().any(|i| {
            matches!(
                i.ownership_category,
                OwnershipCategory::FlatpakManaged
                    | OwnershipCategory::SystemPackageManaged
                    | OwnershipCategory::PortableUserManaged
            )
        }) {
            LifecycleState::ManagedExternally
        } else {
            LifecycleState::InstalledCurrent
        };
        result.push(EmulatorLifecycleProjection {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            emulator_id: id,
            state,
            selected,
            stale_selected,
            installations,
        });
    }
    result
}

/// Inspect one canonical identity, including a deliberate `Missing` result
/// when no candidate was discovered. This avoids making absence of one
/// emulator indistinguishable from an unrequested scan.
pub fn inspect_emulator_lifecycle(
    context: &LifecycleContext,
    emulator_id: &str,
) -> EmulatorLifecycleProjection {
    inspect_all_emulator_lifecycles(context)
        .into_iter()
        .find(|projection| projection.emulator_id == emulator_id)
        .unwrap_or_else(|| {
            let stale_selected = context.selected_bindings.contains_key(emulator_id);
            EmulatorLifecycleProjection {
                schema_version: LIFECYCLE_SCHEMA_VERSION,
                emulator_id: emulator_id.to_string(),
                state: if stale_selected {
                    LifecycleState::Broken
                } else {
                    LifecycleState::Missing
                },
                selected: context.selected_bindings.get(emulator_id).cloned(),
                stale_selected,
                installations: Vec::new(),
            }
        })
}

/// A deliberately bounded parser for `flatpak list --app` output. The command
/// runner is injectable so tests never depend on Flatpak being installed.
pub fn parse_flatpak_list(output: &str) -> Vec<FlatpakInstallation> {
    let known: BTreeSet<&str> = [
        "org.libretro.RetroArch",
        "net.pcsx2.PCSX2",
        "org.DolphinEmu.dolphin-emu",
        "org.flycast.Flycast",
        "app.xemu.xemu",
        "org.ppsspp.PPSSPP",
        "org.duckstation.DuckStation",
        "net.rpcs3.RPCS3",
        "info.cemu.Cemu",
        "io.github.ryubing.Ryujinx",
        "com.xenia_project.Xenia",
    ]
    .into_iter()
    .collect();
    let mut apps = Vec::new();
    for line in output.lines().take(256) {
        let fields: Vec<_> = line.split('\t').collect();
        let Some(app_id) = fields
            .first()
            .map(|s| s.trim())
            .filter(|id| known.contains(*id))
        else {
            continue;
        };
        apps.push(FlatpakInstallation {
            app_id: (*app_id).to_string(),
            version: fields
                .get(1)
                .filter(|v| !v.is_empty())
                .map(|v| (*v).to_string()),
            branch: fields
                .get(2)
                .filter(|v| !v.is_empty())
                .map(|v| (*v).to_string()),
            installed_ref: fields
                .get(3)
                .filter(|v| !v.is_empty())
                .map(|v| (*v).to_string()),
            scope: fields
                .get(4)
                .filter(|v| !v.is_empty())
                .map(|v| (*v).to_string()),
        });
    }
    apps.sort_by(|a, b| a.app_id.cmp(&b.app_id).then(a.scope.cmp(&b.scope)));
    apps
}

pub fn probe_flatpak(command: &str) -> Result<Vec<FlatpakInstallation>, String> {
    let output = bounded_command(
        command,
        &[
            "list",
            "--app",
            "--columns=application,version,branch,ref,installation",
        ],
    )?;
    Ok(parse_flatpak_list(&output))
}

/// Exact package ownership evidence, never an update operation.
pub fn parse_package_ownership(
    manager: &str,
    output: &str,
    executable: &Path,
) -> Option<PackageEvidence> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let (name, version) = if manager == "dpkg" {
        let name = line.split(':').next()?.trim().to_string();
        (name, None)
    } else {
        (
            line.split_whitespace().next()?.to_string(),
            line.split_whitespace().nth(1).map(str::to_string),
        )
    };
    (!name.is_empty()).then(|| PackageEvidence {
        manager: manager.to_string(),
        package_name: name,
        package_version: version,
        executable_path: executable.to_path_buf(),
    })
}

fn command_on_path(command: &str) -> bool {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .take(64)
        .map(|directory| directory.join(command))
        .any(|path| {
            fs::symlink_metadata(path)
                .map(|m| m.is_file() && !m.file_type().is_symlink())
                .unwrap_or(false)
        })
}

/// Query only package databases that are already installed on the host. The
/// function is deliberately best-effort and never invokes an update command.
pub fn discover_package_ownership(executable: &Path) -> Option<PackageEvidence> {
    let path = executable.to_str()?;
    for (manager, command, args) in [
        ("dpkg", "dpkg-query", vec!["-S", path]),
        ("rpm", "rpm", vec!["-qf", path]),
        ("pacman", "pacman", vec!["-Qo", path]),
    ] {
        if !command_on_path(command) {
            continue;
        }
        if let Ok(output) = bounded_command(command, &args)
            && let Some(mut evidence) = parse_package_ownership(manager, &output, executable)
        {
            evidence.manager = manager.to_string();
            return Some(evidence);
        }
    }
    None
}

/// Bounded local inspection entry point. Remote update refresh is intentionally
/// not part of this call, so offline use remains useful and fast.
pub fn inspect_discovered_emulator_lifecycles() -> Vec<EmulatorLifecycleProjection> {
    inspect_discovered_emulator_lifecycles_with_selections(BTreeMap::new())
}

/// Discovers lifecycle candidates and marks only the exact persisted bindings
/// supplied by the caller as selected. An ambiguous emulator remains
/// ambiguous until the caller provides one exact binding.
pub fn inspect_discovered_emulator_lifecycles_with_selections(
    selected_bindings: BTreeMap<EmulatorId, ExactBinding>,
) -> Vec<EmulatorLifecycleProjection> {
    let inventory = crate::emulator_inventory::discover_installed_emulators();
    let package_evidence = inventory
        .installations
        .iter()
        .filter_map(|installation| {
            discover_package_ownership(&installation.executable_path)
                .map(|evidence| (installation.executable_path.clone(), evidence))
        })
        .collect();
    let raw_version_output = inventory
        .installations
        .iter()
        .filter_map(|installation| {
            crate::emulator_inventory::probe_version_output(&installation.executable_path)
                .map(|output| (installation.executable_path.clone(), output))
        })
        .collect();
    let flatpak_installations = discover_flatpak().unwrap_or_default();
    inspect_all_emulator_lifecycles(&LifecycleContext {
        inventory,
        flatpak_installations,
        package_evidence,
        raw_version_output,
        selected_bindings,
        ..Default::default()
    })
}

fn bounded_command(command: &str, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(command)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if started.elapsed() >= LIFECYCLE_COMMAND_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("bounded lifecycle provider command timed out".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("{command} exited with {}", output.status));
    }
    let mut bytes = output.stdout;
    bytes.truncate(LIFECYCLE_MAX_OUTPUT_BYTES);
    String::from_utf8(bytes).map_err(|_| "provider output was not UTF-8".into())
}

/// Read-only, bounded Flatpak detection. No sandbox internals are inspected.
pub fn discover_flatpak() -> Result<Vec<FlatpakInstallation>, String> {
    probe_flatpak("flatpak")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn executable(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, b"fixture").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn install(
        path: PathBuf,
        kind: InstallationType,
        version: Option<&str>,
    ) -> EmulatorInstallation {
        EmulatorInstallation {
            emulator: InventoryEmulator::Pcsx2,
            installation_root: path.parent().unwrap().into(),
            executable_path: path,
            version: version.map(str::to_string),
            version_confidence: if version.is_some() {
                VersionConfidence::VerifiedCommand
            } else {
                VersionConfidence::Unknown
            },
            version_source: if version.is_some() {
                VersionSource::VersionCommand
            } else {
                VersionSource::Unknown
            },
            channel: BuildChannel::Stable,
            installation_type: kind,
            update_capability: crate::emulator_inventory::UpdateCapability::ManualUnknown,
            preferred: None,
            save_state_risk: crate::emulator_inventory::SaveStateRisk::NotApplicable,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn exact_hash_like_inventory_evidence_stays_current_and_selected() {
        let dir = tempdir().unwrap();
        let path = executable(dir.path(), "pcsx2");
        let mut context = LifecycleContext::from_inventory(EmulatorInventory {
            installations: vec![install(
                path.clone(),
                InstallationType::Managed,
                Some("2.0.0"),
            )],
            ..Default::default()
        });
        context.selected_bindings.insert(
            "PCSX2".into(),
            ExactBinding::ManagedInstall {
                manifest_path: path.parent().unwrap().join("manifest.json"),
                executable_path: path,
            },
        );
        let projection = inspect_all_emulator_lifecycles(&context);
        assert_eq!(projection[0].state, LifecycleState::InstalledCurrent);
        assert!(projection[0].selected.is_some());
    }

    #[test]
    fn multiple_installations_are_retained_without_switching() {
        let dir = tempdir().unwrap();
        let first = executable(dir.path(), "one");
        let second = executable(dir.path(), "two");
        let context = LifecycleContext::from_inventory(EmulatorInventory {
            installations: vec![
                install(first, InstallationType::Manual, Some("1.0")),
                install(second, InstallationType::AppImage, Some("2.0")),
            ],
            ..Default::default()
        });
        let projection = inspect_all_emulator_lifecycles(&context);
        assert_eq!(projection[0].state, LifecycleState::MultipleInstallations);
        assert_eq!(projection[0].installations.len(), 2);
        assert!(projection[0].selected.is_none());
    }

    #[test]
    fn stale_selection_is_broken_and_never_replaced() {
        let dir = tempdir().unwrap();
        let path = executable(dir.path(), "available");
        let mut context = LifecycleContext::from_inventory(EmulatorInventory {
            installations: vec![install(path.clone(), InstallationType::Manual, Some("1.0"))],
            ..Default::default()
        });
        context.selected_bindings.insert(
            "PCSX2".into(),
            ExactBinding::NativeExecutable {
                path: dir.path().join("gone"),
            },
        );
        let projection = inspect_all_emulator_lifecycles(&context);
        assert!(projection[0].stale_selected);
        assert_eq!(projection[0].state, LifecycleState::Broken);
        assert!(projection[0].selected.is_none());
    }

    #[test]
    fn flatpak_parser_is_fixture_only_and_preserves_app_id() {
        let parsed = parse_flatpak_list(
            "net.pcsx2.PCSX2\t2.4.0\tstable\tapp/net.pcsx2.PCSX2/x86_64/stable\tuser\nunknown.App\t1\tstable\tr\tsystem",
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].app_id, "net.pcsx2.PCSX2");
        assert_eq!(parsed[0].scope.as_deref(), Some("user"));
    }

    #[test]
    fn package_parser_is_ownership_only() {
        let evidence =
            parse_package_ownership("dpkg", "pcsx2: /usr/bin/pcsx2", Path::new("/usr/bin/pcsx2"))
                .unwrap();
        assert_eq!(evidence.package_name, "pcsx2");
    }

    #[test]
    fn offline_update_does_not_become_outdated() {
        let dir = tempdir().unwrap();
        let path = executable(dir.path(), "pcsx2");
        let installation = install(path.clone(), InstallationType::Manual, Some("1.0"));
        let update = crate::emulator_update::UpdateResult {
            emulator: InventoryEmulator::Pcsx2,
            executable_path: path,
            installed_version: Some("1.0".into()),
            installed_channel: BuildChannel::Stable,
            available_version: None,
            available_channel: BuildChannel::Unknown,
            status: UpdateStatus::Offline,
            source: crate::emulator_update::UpdateMetadataSource::Unknown,
            provenance: "offline".into(),
            checked_unix_seconds: 0,
            warning: None,
            save_state_warning: false,
        };
        let context = LifecycleContext {
            inventory: EmulatorInventory {
                installations: vec![installation],
                ..Default::default()
            },
            updates: UpdateReport {
                results: vec![update],
            },
            ..Default::default()
        };
        let projection = inspect_all_emulator_lifecycles(&context);
        assert_eq!(
            projection[0].installations[0].update_status,
            Some(UpdateStatus::Offline)
        );
    }

    #[test]
    fn external_appimage_is_not_emuwiz_owned() {
        let dir = tempdir().unwrap();
        let path = executable(dir.path(), "PCSX2.AppImage");
        let context = LifecycleContext::from_inventory(EmulatorInventory {
            installations: vec![install(path, InstallationType::AppImage, Some("2.0"))],
            ..Default::default()
        });
        let item = &inspect_all_emulator_lifecycles(&context)[0].installations[0];
        assert_eq!(
            item.ownership_category,
            OwnershipCategory::PortableUserManaged
        );
        assert_eq!(item.update_authority, UpdateAuthority::UserManaged);
    }

    #[test]
    fn flatpak_is_an_exact_external_binding_and_not_a_path() {
        let mut context = LifecycleContext::default();
        context.flatpak_installations.push(FlatpakInstallation {
            app_id: "net.pcsx2.PCSX2".into(),
            installed_ref: Some("app/net.pcsx2.PCSX2/x86_64/stable".into()),
            version: Some("2.4.0".into()),
            branch: Some("stable".into()),
            scope: Some("user".into()),
        });
        let item = &inspect_emulator_lifecycle(&context, "PCSX2").installations[0];
        assert!(matches!(
            item.exact_binding,
            ExactBinding::FlatpakApp { .. }
        ));
        assert_eq!(item.ownership_category, OwnershipCategory::FlatpakManaged);
        assert_eq!(item.update_authority, UpdateAuthority::Flatpak);
    }

    #[test]
    fn missing_and_stale_are_distinct_from_empty_scan() {
        let context = LifecycleContext::default();
        let missing = inspect_emulator_lifecycle(&context, "RPCS3");
        assert_eq!(missing.state, LifecycleState::Missing);
        assert!(!missing.stale_selected);
    }

    #[test]
    fn missing_selected_install_is_broken_without_fallback() {
        let mut context = LifecycleContext::default();
        context.selected_bindings.insert(
            "PCSX2".into(),
            ExactBinding::NativeExecutable {
                path: PathBuf::from("/gone/pcsx2"),
            },
        );
        let stale = inspect_emulator_lifecycle(&context, "PCSX2");
        assert_eq!(stale.state, LifecycleState::Broken);
        assert!(stale.stale_selected);
    }
}
