//! Read-only orchestration for the persistent-state inventory.
//!
//! Profile discovery remains the authority for effective emulator paths.  This
//! module only composes those already-discovered paths with the provider-neutral
//! inventory; it never creates a directory, probes a binary, or changes state.

use crate::diagnostics::profiles::DiscoveredProfiles;
use crate::emulator_lifecycle::inspect_discovered_emulator_lifecycles;
use crate::persistent_state_inventory::{
    PersistentStateInventory, PersistentStateRoot, PersistentStateType, StateEmulator,
    StateInstallation, StatePathOrigin, inventory_persistent_state,
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
            // A custom `[MemoryCards] Directory` that the shared folder rules
            // refuse is reported, never replaced by a silent scan of the
            // default folder DuckStation is not using.
            if let Err(refusal) = crate::patch_manager::resolve_duckstation_memory_cards_folder(
                &profile.configuration_path,
            ) {
                unavailable.push(UnavailableStateRoot {
                    emulator: StateEmulator::DuckStation,
                    profile: Some(profile.profile_id.clone()),
                    path: None,
                    reason: format!("DuckStation memory-card folder was refused: {refusal:?}"),
                });
            } else {
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
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_identity::IdentityKind;
    use crate::patch_manager::{
        DuckStationProfileDiscovery, DuckStationProfileDiscoveryRoots,
        discover_duckstation_profiles,
    };
    use std::collections::BTreeMap;
    use std::fs;

    fn ps1_card() -> Vec<u8> {
        crate::memory_card_inventory::tests::fixture()
    }

    fn profiles_for(root: &Path) -> DiscoveredProfiles {
        let home = root.parent().unwrap().to_path_buf();
        let discovery = discover_duckstation_profiles(&DuckStationProfileDiscoveryRoots {
            home: home.clone(),
            xdg_config_home: home.clone(),
            xdg_data_home: home.join("data"),
            xdg_config_home_explicit: true,
            explicit_configuration_roots: vec![root.to_path_buf()],
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
            path_override: Some(Vec::new()),
        });
        let profiles: Vec<_> = discovery
            .profiles
            .into_iter()
            .filter(|profile| profile.configuration_path == root && profile.eligible)
            .collect();
        assert_eq!(profiles.len(), 1, "fixture profile must be discovered");
        let not_under_test = || "not under test".to_string();
        DiscoveredProfiles {
            dolphin: Err(not_under_test()),
            preferred_dolphin: None,
            pcsx2: Err(not_under_test()),
            xenia: None,
            ppsspp: Err(not_under_test()),
            duckstation: Ok(DuckStationProfileDiscovery {
                profiles,
                complete: true,
            }),
            xemu: Err(not_under_test()),
            rpcs3: Err(not_under_test()),
        }
    }

    fn duckstation_cards(inventory: &ConfiguredStateInventory) -> Vec<PathBuf> {
        inventory
            .inventory
            .records
            .iter()
            .filter(|r| {
                r.emulator == StateEmulator::DuckStation
                    && r.state_type == PersistentStateType::MemoryCard
            })
            .map(|r| r.path.clone())
            .collect()
    }

    #[test]
    fn custom_duckstation_memory_card_folder_is_inventoried_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("duckstation");
        let custom = temp.path().join("cards").join("ps1");
        fs::create_dir_all(&custom).unwrap();
        fs::create_dir_all(root.join("memcards")).unwrap();
        fs::write(
            root.join("settings.ini"),
            format!(
                "[MemoryCards]\nDirectory = {}\nCard1Type = PerGameTitle\n",
                custom.display()
            ),
        )
        .unwrap();
        // DuckStation's default PerGameTitle naming: the card is named by
        // title, not serial.  Discovery must not depend on the file name.
        let card = custom.join("Crash Bandicoot (USA)_1.mcd");
        let bytes = ps1_card();
        fs::write(&card, &bytes).unwrap();
        // A stale card in the default folder DuckStation is not using.
        fs::write(root.join("memcards/stale_1.mcd"), ps1_card()).unwrap();

        let inventory = inventory_discovered_profiles(&profiles_for(&root));

        assert_eq!(duckstation_cards(&inventory), vec![card.clone()]);
        let save = inventory
            .inventory
            .records
            .iter()
            .find(|r| {
                r.emulator == StateEmulator::DuckStation
                    && r.state_type == PersistentStateType::NativeSave
                    && r.container_path.as_deref() == Some(card.as_path())
            })
            .expect("the save inside the title-named card is listed");
        assert!(save.game_identity.iter().any(|identity| {
            identity.kind == IdentityKind::Ps1Serial
                && identity.value.as_deref() == Some("SLUS-00067")
        }));
        assert_eq!(fs::read(&card).unwrap(), bytes, "card bytes unchanged");
    }

    #[test]
    fn refused_duckstation_memory_card_folder_is_reported_not_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("duckstation");
        fs::create_dir_all(root.join("memcards")).unwrap();
        fs::write(root.join("memcards/shared_card_1.mcd"), ps1_card()).unwrap();
        fs::write(
            root.join("settings.ini"),
            "[MemoryCards]\nDirectory = ../outside\n",
        )
        .unwrap();

        let inventory = inventory_discovered_profiles(&profiles_for(&root));

        assert!(duckstation_cards(&inventory).is_empty());
        assert!(inventory.unavailable.iter().any(|u| {
            u.emulator == StateEmulator::DuckStation
                && u.reason.contains("memory-card folder was refused")
        }));
    }
}
