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
    /// Uses complete, synchronous standalone-emulator discovery: for building
    /// a workflow and for Review/Apply-time revalidation.
    pub(crate) fn cheat_route_request(
        &self,
        platform: Option<&str>,
        selected: Option<CheatRouteTarget>,
    ) -> CheatRouteRequest {
        let installed = platform
            .and_then(canonical_cheat_platform)
            .map(|platform_id| self.standalone_emulator_discovery.discover_now(platform_id))
            .unwrap_or_default();
        self.cheat_route_request_with(platform, selected, installed)
    }

    /// Per-frame variant: never touches the filesystem. Uses the last
    /// observed standalone-emulator discovery (refreshed in the background
    /// at a bounded interval); `None` until one has been observed.
    pub(crate) fn cached_cheat_route_request(
        &self,
        platform: Option<&str>,
        selected: Option<CheatRouteTarget>,
        context: &egui::Context,
    ) -> Option<CheatRouteRequest> {
        let installed = match platform.and_then(canonical_cheat_platform) {
            Some(platform_id) => self
                .standalone_emulator_discovery
                .cached(platform_id, context)?,
            None => Vec::new(),
        };
        Some(self.cheat_route_request_with(platform, selected, installed))
    }

    fn cheat_route_request_with(
        &self,
        platform: Option<&str>,
        selected: Option<CheatRouteTarget>,
        installed_standalone: Vec<String>,
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
            installed_standalone,
            retroarch_cores,
            retroarch_installed,
        }
    }

    /// Whether `workflow` was built for the emulator/core the current choice
    /// and readiness facts still route to. A workflow with no routed install
    /// target (local-preview adapters) has nothing to go stale here.
    pub(crate) fn cheat_route_target_is_current(&self, workflow: &CheatWorkflowState) -> bool {
        let Some(built_for) = workflow.routing.decision.applicable_target() else {
            return true;
        };
        let live = archivefs_core::patch_manager::route_cheat_install(
            &self.cheat_route_request(
                workflow.platform.as_deref(),
                self.cheat_emulator_selections
                    .get(&workflow.archive_path)
                    .cloned(),
            ),
        );
        live.applicable_target() == Some(built_for)
    }

    /// Per-frame form of [`Self::cheat_route_target_is_current`] using cached
    /// discovery only. `None` means not yet known (no observation cached):
    /// callers must not treat that as a change.
    pub(crate) fn cheat_route_target_is_current_cached(
        &self,
        workflow: &CheatWorkflowState,
        context: &egui::Context,
    ) -> Option<bool> {
        let Some(built_for) = workflow.routing.decision.applicable_target() else {
            return Some(true);
        };
        let request = self.cached_cheat_route_request(
            workflow.platform.as_deref(),
            self.cheat_emulator_selections
                .get(&workflow.archive_path)
                .cloned(),
            context,
        )?;
        let live = archivefs_core::patch_manager::route_cheat_install(&request);
        Some(live.applicable_target() == Some(built_for))
    }

    /// Classic and Blue both call this every frame on the Cheats & Mods page.
    /// When the workflow's emulator/core target is no longer what the
    /// current choice and readiness facts route to, the workflow (with any
    /// preview, review or confirmation built for the old target) is
    /// discarded and rebuilt for the current target, so the user can preview
    /// again without reselecting the emulator. Never interrupts an apply in
    /// flight or a busy adapter task. Returns whether it rebuilt.
    pub(crate) fn refresh_cheat_workflow_after_target_change(
        &mut self,
        context: &egui::Context,
    ) -> bool {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return false;
        };
        if matches!(
            workflow.transaction,
            CheatTransactionState::Applying { .. } | CheatTransactionState::NativeApplying { .. }
        ) || matches!(workflow.duckstation.undo, NativeUndoState::Applying { .. })
            // A RetroArch rescan briefly drops the core list; wait for it
            // rather than rebuilding twice.
            || matches!(
                self.emulator_readiness.retroarch_profiles,
                RetroArchProfilesState::Scanning { .. }
            )
            || workflow.melonds.busy()
            || workflow.mgba.busy()
            || workflow.flycast.busy()
            || workflow.rpcs3.busy()
            || workflow.ppsspp.busy()
            || self.cheat_route_target_is_current_cached(workflow, context) != Some(false)
        {
            return false;
        }
        let archive = workflow.archive_path.clone();
        if let Some(cancellation) = workflow.gamecube_gamehacking_cancellation.as_ref() {
            cancellation.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.cheat_workflow = None;
        self.open_cheats_mods_workspace(context, archive);
        if let Some(workflow) = self.cheat_workflow.as_mut() {
            workflow.transaction_notice = Some(CHEAT_TARGET_REFRESHED.to_string());
        }
        true
    }

    /// Re-resolves the cheat route immediately before Review/Apply and
    /// refuses unless it still names the target (emulator and RetroArch core)
    /// this workflow and its preview were built for. Arcade additionally
    /// requires a routed target with an identified RetroArch core. Returns
    /// `false` (and cancels any pending review) when refused.
    pub(crate) fn revalidate_cheat_target(&mut self) -> bool {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return false;
        };
        let arcade = workflow
            .platform
            .as_deref()
            .and_then(canonical_cheat_platform)
            == Some(archivefs_core::patch_manager::ARCADE_PLATFORM_ID);
        let built_for = workflow.routing.decision.applicable_target();
        let refusal = if !self.cheat_route_target_is_current(workflow) {
            Some(if arcade {
                ARCADE_TARGET_CHANGED
            } else {
                CHEAT_TARGET_CHANGED
            })
        } else if arcade && built_for.is_none() {
            Some(ARCADE_TARGET_CHANGED)
        } else if arcade && matches!(built_for, Some(CheatRouteTarget::RetroArch { core: None })) {
            // The destination depends on the core; an unidentified core
            // cannot be confirmed.
            Some(ARCADE_CORE_REQUIRED)
        } else {
            None
        };
        let Some(refusal) = refusal else {
            return true;
        };
        if let Some(workflow) = self.cheat_workflow.as_mut() {
            // Only a review that has not started writing is cancelled; an
            // apply already in flight keeps its result receiver.
            if matches!(
                workflow.transaction,
                CheatTransactionState::Idle
                    | CheatTransactionState::Review { .. }
                    | CheatTransactionState::NativeReview { .. }
            ) {
                workflow.transaction = CheatTransactionState::Idle;
            }
            workflow.transaction_notice = Some(refusal.to_string());
        }
        false
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
            workflow.melonds.busy()
                || workflow.mgba.busy()
                || workflow.rpcs3.busy()
                || workflow.ppsspp.busy()
                || matches!(workflow.transaction, CheatTransactionState::Applying { .. })
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

/// Shown when an Arcade game's RetroArch core has not been identified.
pub(crate) const ARCADE_CORE_REQUIRED: &str = "EmuWiz has not identified which RetroArch arcade core runs this game, so it cannot confirm where cheats would go. Choose the core (for example RetroArch's fbneo core), or install or rescan RetroArch cores, before installing. Nothing was installed.";

/// How long a cached standalone-emulator discovery is used by per-frame
/// checks before a background refresh is started.
const STANDALONE_DISCOVERY_REFRESH: std::time::Duration = std::time::Duration::from_secs(5);

type StandaloneDiscoverFn = dyn Fn(&str) -> Vec<String> + Send + Sync;

#[derive(Default)]
struct StandaloneDiscoveryEntry {
    observed: Option<(std::time::Instant, Vec<String>)>,
    refreshing: bool,
}

/// Which competing standalone emulators (DuckStation, PPSSPP, Flycast) were
/// observed per platform. Discovery reads emulator configuration folders, so
/// per-frame checks only read this cache; a stale entry is refreshed on a
/// background thread so a slow or unavailable folder never blocks a frame.
/// Building a workflow and Review/Apply always discover synchronously.
#[derive(Clone)]
pub(crate) struct StandaloneEmulatorDiscovery {
    entries: std::sync::Arc<
        std::sync::Mutex<std::collections::BTreeMap<String, StandaloneDiscoveryEntry>>,
    >,
    discover: std::sync::Arc<StandaloneDiscoverFn>,
    refresh_interval: std::time::Duration,
}

impl Default for StandaloneEmulatorDiscovery {
    fn default() -> Self {
        Self::with_discover(
            std::sync::Arc::new(observed_competing_standalone_emulators),
            STANDALONE_DISCOVERY_REFRESH,
        )
    }
}

impl StandaloneEmulatorDiscovery {
    pub(crate) fn with_discover(
        discover: std::sync::Arc<StandaloneDiscoverFn>,
        refresh_interval: std::time::Duration,
    ) -> Self {
        Self {
            entries: Default::default(),
            discover,
            refresh_interval,
        }
    }

    /// Complete synchronous discovery; also refreshes the cache.
    pub(crate) fn discover_now(&self, platform_id: &str) -> Vec<String> {
        let observed = (self.discover)(platform_id);
        if let Ok(mut entries) = self.entries.lock() {
            entries.entry(platform_id.to_string()).or_default().observed =
                Some((std::time::Instant::now(), observed.clone()));
        }
        observed
    }

    /// Last observed value without touching the filesystem. Starts one
    /// background refresh when the entry is missing or older than the
    /// refresh interval.
    pub(crate) fn cached(&self, platform_id: &str, context: &egui::Context) -> Option<Vec<String>> {
        let mut entries = self.entries.lock().ok()?;
        let entry = entries.entry(platform_id.to_string()).or_default();
        let stale = entry
            .observed
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= self.refresh_interval);
        if stale && !entry.refreshing {
            entry.refreshing = true;
            let this = self.clone();
            let platform_id = platform_id.to_string();
            let context = context.clone();
            std::thread::spawn(move || {
                let observed = (this.discover)(&platform_id);
                if let Ok(mut entries) = this.entries.lock() {
                    let entry = entries.entry(platform_id).or_default();
                    entry.observed = Some((std::time::Instant::now(), observed));
                    entry.refreshing = false;
                }
                context.request_repaint();
            });
        }
        entry
            .observed
            .as_ref()
            .map(|(_, observed)| observed.clone())
    }
}

/// The workflow's notice when it explains a changed or unconfirmable cheat
/// target; rendered as a banner on the shared Cheats & Mods page (Classic
/// and Blue).
pub(crate) fn cheat_target_notice(workflow: &CheatWorkflowState) -> Option<&str> {
    workflow.transaction_notice.as_deref().filter(|notice| {
        [
            CHEAT_TARGET_REFRESHED,
            CHEAT_TARGET_CHANGED,
            ARCADE_TARGET_CHANGED,
            ARCADE_CORE_REQUIRED,
        ]
        .contains(notice)
    })
}

/// Shown after the page was rebuilt for a changed emulator or core.
pub(crate) const CHEAT_TARGET_REFRESHED: &str = "The emulator or RetroArch core for this game changed, so this page was refreshed for the new target and any unconfirmed preview was discarded. Choose cheats again to preview the new destination, then confirm.";

/// Shown when the emulator or RetroArch core changed after the preview.
pub(crate) const CHEAT_TARGET_CHANGED: &str = "The emulator or RetroArch core for this game changed after the preview, so nothing was installed. Review the new preview before installing.";

/// Shown when an Arcade game's cheat target is no longer the confirmed one.
pub(crate) const ARCADE_TARGET_CHANGED: &str = "The emulator for this Arcade game is no longer the one you confirmed, so nothing was installed. Choose the emulator again and review the preview before installing.";

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
        // DuckStation's loadability is judged from its native receipt.
        CheatEmulatorAdapter::DuckStation | CheatEmulatorAdapter::Unsupported => (None, None),
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
        | CheatEmulatorAdapter::Xenia
        | CheatEmulatorAdapter::DuckStation => CheatIdentityStrength::Exact,
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
