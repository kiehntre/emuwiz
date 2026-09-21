//! Read-only orchestration for the persistent-state inventory.
//!
//! Profile discovery remains the authority for effective emulator paths.  This
//! module only composes those already-discovered paths with the provider-neutral
//! inventory; it never creates a directory, probes a binary, or changes state.

use crate::diagnostics::profiles::DiscoveredProfiles;
use crate::emulator_lifecycle::inspect_discovered_emulator_lifecycles;
use crate::persistent_state_inventory::{
    inventory_persistent_state, PersistentStateInventory, PersistentStateRoot, PersistentStateType,
    StateEmulator, StateInstallation, StatePathOrigin,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnavailableStateRoot {
    pub emulator: StateEmulator,
    pub profile: Option<String>,
    pub path: Option<PathBuf>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguredStateInventory {
    pub inventory: PersistentStateInventory,
    pub unavailable: Vec<UnavailableStateRoot>,
    pub lifecycle_projections: usize,
    pub read_only: bool,
}

/// Build an inventory from the existing, read-only profile discovery surfaces.
/// Missing configured paths are reported as unavailable rather than empty.
pub fn inventory_configured_state() -> ConfiguredStateInventory {
    let profiles = DiscoveredProfiles::from_environment(Vec::new());
    inventory_discovered_profiles(&profiles)
}

/// Compose already-discovered profiles.  This entry point makes deterministic
/// CLI and adapter tests possible without touching process configuration.
pub fn inventory_discovered_profiles(profiles: &DiscoveredProfiles) -> ConfiguredStateInventory {
    let mut roots = Vec::new();
    let mut unavailable = Vec::new();
    let lifecycle_projections = inspect_discovered_emulator_lifecycles().len();

    if let Ok(discovery) = &profiles.duckstation {
        for profile in selected_profiles(&discovery.profiles) {
            let installation = installation(
                &profile.profile_id,
                profile
                    .executable_candidates
                    .first()
                    .map(|x| x.path.clone()),
                profile
                    .executable_candidates
                    .first()
                    .and_then(|x| x.version.clone()),
                profile.provenance,
            );
            root_or_unavailable(
                &mut roots,
                &mut unavailable,
                PersistentStateRoot {
                    emulator: StateEmulator::DuckStation,
                    installation: Some(installation.clone()),
                    path: profile.memory_cards_path.clone(),
                    path_origin: origin(profile.provenance),
                    effective_from_configuration: true,
                    state_type_hint: Some(PersistentStateType::MemoryCard),
                    container_path: None,
                    identity_evidence: Vec::new(),
                },
                &profile.profile_id,
            );
            root_or_unavailable(
                &mut roots,
                &mut unavailable,
                PersistentStateRoot {
                    emulator: StateEmulator::DuckStation,
                    installation: Some(installation),
                    path: profile.save_states_path.clone(),
                    path_origin: origin(profile.provenance),
                    effective_from_configuration: true,
                    state_type_hint: Some(PersistentStateType::SaveState),
                    container_path: None,
                    identity_evidence: Vec::new(),
                },
                &profile.profile_id,
            );
        }
    } else {
        unavailable.push(UnavailableStateRoot {
            emulator: StateEmulator::DuckStation,
            profile: None,
            path: None,
            reason: "profile discovery unavailable".into(),
        });
    }

    if let Ok(discovery) = &profiles.ppsspp {
        for profile in selected_profiles(&discovery.profiles) {
            let installation = installation(
                &profile.profile_id,
                profile.resolved_emulator_executable(),
                profile
                    .executable_candidates
                    .first()
                    .and_then(|x| x.version.clone()),
                profile.provenance,
            );
            root_or_unavailable(
                &mut roots,
                &mut unavailable,
                PersistentStateRoot {
                    emulator: StateEmulator::Ppsspp,
                    installation: Some(installation.clone()),
                    path: profile.savedata_path.clone(),
                    path_origin: origin(profile.provenance),
                    effective_from_configuration: true,
                    state_type_hint: Some(PersistentStateType::NativeSave),
                    container_path: None,
                    identity_evidence: Vec::new(),
                },
                &profile.profile_id,
            );
            root_or_unavailable(
                &mut roots,
                &mut unavailable,
                PersistentStateRoot {
                    emulator: StateEmulator::Ppsspp,
                    installation: Some(installation),
                    path: profile.state_path.clone(),
                    path_origin: origin(profile.provenance),
                    effective_from_configuration: true,
                    state_type_hint: Some(PersistentStateType::SaveState),
                    container_path: None,
                    identity_evidence: Vec::new(),
                },
                &profile.profile_id,
            );
        }
    }

    if let Ok(discovery) = &profiles.pcsx2 {
        for profile in selected_profiles(&discovery.profiles) {
            let installation = installation(
                &profile.profile_id,
                profile
                    .executable_candidates
                    .first()
                    .map(|x| x.path.clone()),
                None,
                profile.provenance,
            );
            let root = &profile.configuration_path;
            root_or_unavailable(
                &mut roots,
                &mut unavailable,
                configured_dir(
                    StateEmulator::Pcsx2,
                    installation.clone(),
                    root.join("memcards"),
                    PersistentStateType::MemoryCard,
                    profile.provenance,
                ),
                &profile.profile_id,
            );
            root_or_unavailable(
                &mut roots,
                &mut unavailable,
                configured_dir(
                    StateEmulator::Pcsx2,
                    installation,
                    root.join("sstates"),
                    PersistentStateType::SaveState,
                    profile.provenance,
                ),
                &profile.profile_id,
            );
        }
    }

    if let Ok(discovery) = &profiles.rpcs3 {
        for profile in selected_profiles(&discovery.profiles) {
            let installation = installation(
                &profile.profile_id,
                profile
                    .executable_candidates
                    .first()
                    .map(|x| x.path.clone()),
                profile
                    .executable_candidates
                    .first()
                    .and_then(|x| x.version.clone()),
                profile.provenance,
            );
            root_or_unavailable(
                &mut roots,
                &mut unavailable,
                configured_dir(
                    StateEmulator::Rpcs3,
                    installation,
                    profile.dev_hdd0_path.clone(),
                    PersistentStateType::NandOrVirtualDisk,
                    profile.provenance,
                ),
                &profile.profile_id,
            );
        }
    }

    if let Ok(discovery) = &profiles.xemu {
        for profile in selected_profiles(&discovery.profiles) {
            let installation = installation(
                &profile.profile_id,
                profile
                    .executable_candidates
                    .first()
                    .map(|x| x.path.clone()),
                profile
                    .executable_candidates
                    .first()
                    .and_then(|x| x.version.clone()),
                profile.provenance,
            );
            root_or_unavailable(
                &mut roots,
                &mut unavailable,
                configured_dir(
                    StateEmulator::Xemu,
                    installation,
                    profile.configuration_path.clone(),
                    PersistentStateType::NandOrVirtualDisk,
                    profile.provenance,
                ),
                &profile.profile_id,
            );
        }
    }

    let inventory = inventory_persistent_state(&roots);
    ConfiguredStateInventory {
        inventory,
        unavailable,
        lifecycle_projections,
        read_only: true,
    }
}

fn configured_dir(
    emulator: StateEmulator,
    installation: StateInstallation,
    path: PathBuf,
    hint: PersistentStateType,
    provenance: &str,
) -> PersistentStateRoot {
    PersistentStateRoot {
        emulator,
        installation: Some(installation),
        path,
        path_origin: origin(provenance),
        effective_from_configuration: true,
        state_type_hint: Some(hint),
        container_path: None,
        identity_evidence: Vec::new(),
    }
}

fn root_or_unavailable(
    roots: &mut Vec<PersistentStateRoot>,
    unavailable: &mut Vec<UnavailableStateRoot>,
    root: PersistentStateRoot,
    profile: &str,
) {
    if root.path.is_absolute() && Path::new(&root.path).exists() {
        roots.push(root);
    } else {
        unavailable.push(UnavailableStateRoot {
            emulator: root.emulator,
            profile: Some(profile.into()),
            path: Some(root.path),
            reason: "effective configured path is unavailable".into(),
        });
    }
}

fn selected_profiles<T>(profiles: &[T]) -> Vec<&T> {
    // The lifecycle contract is explicit selection, never an implicit merge.
    // Until a caller supplies a selected binding, discovery is only safe when
    // it produced one profile.
    if profiles.len() == 1 {
        profiles.iter().collect()
    } else {
        Vec::new()
    }
}

fn origin(provenance: &str) -> StatePathOrigin {
    let p = provenance.to_ascii_lowercase();
    if p.contains("flatpak") {
        StatePathOrigin::Flatpak
    } else if p.contains("portable") || p.contains("appimage") {
        StatePathOrigin::Portable
    } else {
        StatePathOrigin::Native
    }
}

fn installation(
    profile: &str,
    executable: Option<PathBuf>,
    version: Option<String>,
    _provenance: &str,
) -> StateInstallation {
    StateInstallation {
        installation_id: format!("profile:{profile}"),
        executable,
        version,
        profile: Some(profile.into()),
        firmware_context: None,
        selected: true,
    }
}

// The profile modules intentionally keep their resolved profile private to the
// shared `resolved` field for adapters that expose it.  These helpers preserve
// a small, stable orchestration boundary without duplicating lifecycle logic.
trait ProfileResolution {
    fn resolved_emulator_executable(&self) -> Option<PathBuf>;
}

impl ProfileResolution for crate::patch_manager::PpssppProfile {
    fn resolved_emulator_executable(&self) -> Option<PathBuf> {
        self.executable_candidates.first().map(|x| x.path.clone())
    }
}
impl ProfileResolution for crate::patch_manager::Pcsx2Profile {
    fn resolved_emulator_executable(&self) -> Option<PathBuf> {
        self.executable_candidates.first().map(|x| x.path.clone())
    }
}
impl ProfileResolution for crate::patch_manager::Rpcs3Profile {
    fn resolved_emulator_executable(&self) -> Option<PathBuf> {
        self.executable_candidates.first().map(|x| x.path.clone())
    }
}
impl ProfileResolution for crate::patch_manager::XemuProfile {
    fn resolved_emulator_executable(&self) -> Option<PathBuf> {
        self.executable_candidates.first().map(|x| x.path.clone())
    }
}
