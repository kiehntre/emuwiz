//! Selected-emulator cheat routing and post-install loadability for the
//! Cheats & Mods workflow.
//!
//! Routing decisions and loadability classification live in core
//! (`archivefs_core::patch_manager::{route_cheat_install,
//! assess_cheat_loadability}`); this module only gathers the GUI's already
//! discovered facts (selected emulator, remembered profiles, installed
//! emulators, RetroArch cores, profile directories, cheat switches) and
//! hands them over. It never switches the user's emulator on its own.

use std::path::{Path, PathBuf};

use archivefs_core::patch_manager::{
    CheatEnablementEvidence, CheatIdentityStrength, CheatLoadPathConvention, CheatLoadabilityInput,
    CheatLoadabilityReport, CheatRouteRequest, CheatRouteTarget, PreviewMatchStrength,
    SharedApplyOutcome, assess_cheat_loadability, canonical_cheat_platform,
    observe_emulator_process, read_config_bool, verify_installed_cheat_file,
};

use crate::*;

impl ArchiveFsApp {
    /// Builds the routing request from facts the GUI has already observed.
    /// Installed-standalone probing is limited to the platforms where a
    /// standalone emulator competes with RetroArch for a cheat route.
    pub(crate) fn cheat_route_request(
        &self,
        platform: Option<&str>,
        selected: Option<CheatRouteTarget>,
    ) -> CheatRouteRequest {
        let canonical = platform.and_then(canonical_cheat_platform);
        let configured_defaults = self
            .emulator_readiness
            .remembered_emulator_profiles
            .iter()
            .map(|profile| {
                if profile.adapter.eq_ignore_ascii_case("retroarch") {
                    CheatRouteTarget::retroarch(None)
                } else {
                    CheatRouteTarget::standalone(&profile.adapter)
                }
            })
            .collect();
        let (retroarch_installed, retroarch_cores) =
            match &self.emulator_readiness.retroarch_profiles {
                RetroArchProfilesState::Ready(discovery) => {
                    let cores = canonical
                        .map(|platform_id| retroarch_cores_for_platform(discovery, platform_id))
                        .unwrap_or_default();
                    (
                        Some(discovery.profiles.iter().any(|profile| profile.eligible)),
                        cores,
                    )
                }
                RetroArchProfilesState::NotScanned
                | RetroArchProfilesState::Scanning { .. }
                | RetroArchProfilesState::Error(_) => (None, Vec::new()),
            };
        CheatRouteRequest {
            platform: platform.map(str::to_owned),
            selected,
            configured_defaults,
            installed_standalone: canonical
                .map(observed_competing_standalone_emulators)
                .unwrap_or_default(),
            retroarch_cores,
            retroarch_installed,
        }
    }

    /// Records (or clears) the user's explicit emulator choice for the
    /// current game and rebuilds the workflow so every adapter-specific
    /// state is created fresh for the newly routed emulator.
    pub(crate) fn choose_cheat_emulator(
        &mut self,
        context: &egui::Context,
        target: Option<CheatRouteTarget>,
    ) {
        let Some(archive) = self
            .cheat_workflow
            .as_ref()
            .map(|workflow| workflow.archive_path.clone())
        else {
            return;
        };
        if self.cheat_workflow.as_ref().is_some_and(|workflow| {
            matches!(workflow.transaction, CheatTransactionState::Applying { .. })
        }) {
            return;
        }
        match target {
            Some(target) => {
                self.cheat_emulator_selections
                    .insert(archive.clone(), target);
            }
            None => {
                self.cheat_emulator_selections.remove(&archive);
            }
        }
        if let Some(cancellation) = self
            .cheat_workflow
            .as_ref()
            .and_then(|workflow| workflow.gamecube_gamehacking_cancellation.as_ref())
        {
            cancellation.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.cheat_workflow = None;
        self.open_cheats_mods_workspace(context, archive);
    }
}

/// Installed RetroArch cores whose own `.info` metadata declares this
/// platform, sorted and de-duplicated.
pub(crate) fn retroarch_cores_for_platform(
    discovery: &archivefs_core::patch_manager::RetroArchCheatSetupDiscovery,
    platform_id: &str,
) -> Vec<String> {
    let mut cores: Vec<String> = discovery
        .environment
        .profiles
        .iter()
        .flat_map(|profile| profile.cores.iter())
        .filter(|core| {
            archivefs_core::launch::platform_map::retroarch_platform_matches(
                &core.info,
                platform_id,
            )
        })
        .map(|core| core.core_stem.clone())
        .collect();
    cores.sort();
    cores.dedup();
    cores
}

/// Standalone emulators that compete with RetroArch for a platform and that
/// EmuWiz can detect without a background scan. Bounded, read-only
/// discovery, the same the launch-readiness builder already runs.
fn observed_competing_standalone_emulators(platform_id: &str) -> Vec<String> {
    let installed = match platform_id {
        "PSX" => {
            archivefs_core::patch_manager::DuckStationProfileDiscoveryRoots::from_environment()
                .ok()
                .is_some_and(|roots| {
                    archivefs_core::patch_manager::discover_duckstation_profiles(&roots)
                        .profiles
                        .iter()
                        .any(|profile| profile.eligible)
                })
                .then_some("duckstation")
        }
        "PSP" => archivefs_core::patch_manager::PpssppProfileDiscoveryRoots::from_environment()
            .ok()
            .is_some_and(|roots| {
                archivefs_core::patch_manager::discover_ppsspp_profiles(&roots)
                    .profiles
                    .iter()
                    .any(|profile| profile.eligible)
            })
            .then_some("ppsspp"),
        "Dreamcast" => {
            archivefs_core::patch_manager::FlycastProfileDiscoveryRoots::from_environment()
                .ok()
                .is_some_and(|roots| {
                    archivefs_core::patch_manager::discover_flycast_profiles(&roots)
                        .profiles
                        .iter()
                        .any(|profile| profile.eligible)
                })
                .then_some("flycast")
        }
        _ => None,
    };
    installed.map(str::to_owned).into_iter().collect()
}

fn activation_evidence(
    readiness: CheatActivationReadiness,
    setting: &'static str,
) -> CheatEnablementEvidence {
    CheatEnablementEvidence {
        enabled: match readiness {
            CheatActivationReadiness::Enabled => Some(true),
            CheatActivationReadiness::Disabled => Some(false),
            CheatActivationReadiness::Unknown => None,
        },
        setting,
    }
}

fn suffix_convention(
    directory: PathBuf,
    suffix: &str,
    convention: &'static str,
) -> CheatLoadPathConvention {
    CheatLoadPathConvention {
        directory,
        file_name: None,
        required_suffix: Some(suffix.to_string()),
        convention,
    }
}

/// Where the routed emulator reads this game's cheat file, and its cheat
/// switch, from the selected profile. `None` fields mean "not known", never
/// a guessed default.
fn expected_load_path(
    workflow: &CheatWorkflowState,
    readiness: &EmulatorReadinessState,
    target: &CheatRouteTarget,
) -> (
    Option<CheatLoadPathConvention>,
    Option<CheatEnablementEvidence>,
) {
    match workflow.adapter {
        CheatEmulatorAdapter::Pcsx2 => (
            workflow
                .selected_pcsx2_profile_id
                .as_deref()
                .and_then(|id| resolved_pcsx2_cheats_directory(&readiness.pcsx2_profiles, id))
                .map(|directory| {
                    suffix_convention(directory, ".pnach", "pcsx2_profile_cheats_directory")
                }),
            Some(activation_evidence(
                workflow.pcsx2_activation,
                "[EmuCore] EnableCheats",
            )),
        ),
        CheatEmulatorAdapter::Dolphin => {
            let directory = match &readiness.dolphin_profiles {
                DolphinProfilesState::Ready(discovery) => workflow
                    .selected_dolphin_profile_id
                    .as_deref()
                    .and_then(|id| {
                        discovery
                            .profiles
                            .iter()
                            .find(|profile| profile.profile_id == id)
                    })
                    .map(|profile| profile.game_settings_path.clone()),
                _ => None,
            };
            (
                directory.map(|directory| {
                    suffix_convention(directory, ".ini", "dolphin_user_game_settings")
                }),
                Some(activation_evidence(
                    workflow.dolphin_activation,
                    "[Core] EnableCheats",
                )),
            )
        }
        CheatEmulatorAdapter::Xenia => {
            let profile = match &readiness.xenia_profiles {
                XeniaProfilesState::Ready(discovery) => workflow
                    .selected_xenia_profile_id
                    .as_deref()
                    .and_then(|id| {
                        discovery
                            .profiles
                            .iter()
                            .find(|profile| profile.profile_id == id)
                    }),
                XeniaProfilesState::NotScanned => None,
            };
            (
                profile.map(|profile| {
                    suffix_convention(
                        profile.patches_path.clone(),
                        ".patch.toml",
                        "xenia_patches_directory",
                    )
                }),
                profile.map(|profile| CheatEnablementEvidence {
                    enabled: read_config_bool(
                        &profile.configuration_path.join("xenia-canary.config.toml"),
                        None,
                        "apply_patches",
                    ),
                    setting: "apply_patches",
                }),
            )
        }
        CheatEmulatorAdapter::RetroArch => {
            let profile = match &readiness.retroarch_profiles {
                RetroArchProfilesState::Ready(discovery) => {
                    workflow.selected_profile_id.as_deref().and_then(|id| {
                        discovery
                            .profiles
                            .iter()
                            .find(|profile| profile.profile_id == id)
                    })
                }
                _ => None,
            };
            let convention = match (profile, target.retroarch_core()) {
                (Some(profile), Some(core)) => profile
                    .cheat_destination_root
                    .as_ref()
                    .filter(|root| !root.lossy)
                    .zip(workflow.archive_path.file_stem())
                    .map(|(root, stem)| CheatLoadPathConvention {
                        // RetroArch's per-game auto-load file:
                        // <cheat_database_path>/<core>/<content>.cht
                        directory: PathBuf::from(&root.display).join(core),
                        file_name: Some(format!("{}.cht", stem.to_string_lossy())),
                        required_suffix: None,
                        convention: "retroarch_per_core_game_specific_cheat",
                    }),
                _ => None,
            };
            let enablement = profile
                .filter(|profile| !profile.configuration_path.lossy)
                .map(|profile| CheatEnablementEvidence {
                    enabled: read_config_bool(
                        Path::new(&profile.configuration_path.display),
                        None,
                        "apply_cheats_after_load",
                    ),
                    setting: "apply_cheats_after_load",
                });
            (convention, enablement)
        }
        CheatEmulatorAdapter::Unsupported => (None, None),
    }
}

/// Identity strength the installed cheat was matched with. PCSX2, Dolphin
/// and Xenia installs are bound to a verified CRC / Game ID / Title ID;
/// RetroArch and BSFree records match by title and platform unless the
/// preview proved an exact match.
fn install_identity_strength(workflow: &CheatWorkflowState) -> CheatIdentityStrength {
    let CheatStepResource::Ready(response) = &workflow.preview else {
        return CheatIdentityStrength::Unknown;
    };
    if response.bsfree_gamecube_generated.is_some() || response.bsfree_wii_generated.is_some() {
        return CheatIdentityStrength::TitleOnly;
    }
    match workflow.adapter {
        CheatEmulatorAdapter::Pcsx2
        | CheatEmulatorAdapter::Dolphin
        | CheatEmulatorAdapter::Xenia => CheatIdentityStrength::Exact,
        CheatEmulatorAdapter::RetroArch => {
            let exact = response.materialized.as_ref().is_some_and(|materialized| {
                !materialized.sources.is_empty()
                    && materialized
                        .sources
                        .iter()
                        .all(|source| source.match_strength == PreviewMatchStrength::VerifiedExact)
            });
            if exact {
                CheatIdentityStrength::Exact
            } else {
                CheatIdentityStrength::TitleOnly
            }
        }
        CheatEmulatorAdapter::Unsupported => CheatIdentityStrength::Unknown,
    }
}

/// Re-reads the installed destination and classifies whether the selected
/// emulator is expected to load it. `None` for dry runs, results with no
/// written entry, or workflows with no routed emulator.
pub(crate) fn cheat_install_loadability(
    workflow: &CheatWorkflowState,
    readiness: &EmulatorReadinessState,
    result: &SharedApplyResult,
    process_probe: impl Fn(
        &CheatRouteTarget,
    ) -> archivefs_core::patch_manager::EmulatorProcessObservation,
) -> Option<CheatLoadabilityReport> {
    let route = workflow.routing.decision.route()?.clone();
    let entry = result.journal.entries.iter().find(|entry| {
        matches!(
            entry.outcome,
            SharedApplyOutcome::InstalledNew
                | SharedApplyOutcome::ReplacedExisting
                | SharedApplyOutcome::AlreadyInstalled
        )
    })?;
    let installed_path = entry
        .plan_entry
        .destination_root
        .to_path_buf()
        .ok()
        .zip(
            entry
                .plan_entry
                .destination_relative_path
                .to_path_buf()
                .ok(),
        )
        .map(|(root, relative)| root.join(relative));
    let file_check = match &installed_path {
        Some(path) => verify_installed_cheat_file(path, entry.final_destination_digest.as_deref()),
        None => archivefs_core::patch_manager::CheatInstalledFileCheck::Unreadable {
            detail: "the journal path could not be decoded".to_string(),
        },
    };
    let (expected_path, enablement) = expected_load_path(workflow, readiness, &route.target);
    let process = process_probe(&route.target);
    Some(assess_cheat_loadability(&CheatLoadabilityInput {
        identity: install_identity_strength(workflow),
        route,
        installed_path,
        file_check,
        expected_path,
        enablement,
        process,
    }))
}

/// Production process probe (Linux `/proc`, read-only).
pub(crate) fn live_emulator_process_probe(
    target: &CheatRouteTarget,
) -> archivefs_core::patch_manager::EmulatorProcessObservation {
    observe_emulator_process(target)
}
