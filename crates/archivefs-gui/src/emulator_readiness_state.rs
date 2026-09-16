use std::path::PathBuf;

use crate::{
    DolphinLocalProfilesState, DolphinProfilesState, EmulatorSetupFocus, FlycastProfilesState,
    Pcsx2FirmwareEvidenceState, Pcsx2LaunchProfilesState, Pcsx2ProfilesState,
    RememberedEmulatorProfile, RetroArchProfilesState, XeniaProfilesState,
    bios_projection_page, emulator_inventory_page, emulator_setup_overrides,
    emulator_setup_page, pcsx2_page, ready_to_play_page, rpcs3_page,
};

/// UI/session state for emulator setup, readiness, and environment projections.
///
/// Readiness and launch decisions remain owned by the existing core and page
/// adapters. This bundle only consolidates the GUI state, profile probes, and
/// setup overrides that cross those surfaces.
pub(crate) struct EmulatorReadinessState {
    pub(crate) retroarch_profiles: RetroArchProfilesState,
    pub(crate) retroarch_core_directory_override: Option<PathBuf>,
    pub(crate) retroarch_core_folder_rejected_pick: Option<PathBuf>,
    pub(crate) emulator_setup_focus: Option<EmulatorSetupFocus>,
    pub(crate) emulator_setup_page: emulator_setup_page::EmulatorSetupPageState,
    pub(crate) emulator_inventory_page: emulator_inventory_page::EmulatorInventoryPageState,
    pub(crate) bios_projection_page: bios_projection_page::BiosProjectionPageState,
    pub(crate) ready_to_play_page: ready_to_play_page::ReadyToPlayPageState,
    pub(crate) emulator_setup_overrides: emulator_setup_overrides::EmulatorPathOverrides,
    pub(crate) pcsx2_profiles: Pcsx2ProfilesState,
    pub(crate) dolphin_profiles: DolphinProfilesState,
    pub(crate) dolphin_local_profiles: DolphinLocalProfilesState,
    pub(crate) pcsx2_launch_profiles: Pcsx2LaunchProfilesState,
    pub(crate) flycast_profiles: FlycastProfilesState,
    pub(crate) pcsx2_firmware_evidence: Pcsx2FirmwareEvidenceState,
    pub(crate) xenia_profiles: XeniaProfilesState,
    pub(crate) remembered_emulator_profiles: Vec<RememberedEmulatorProfile>,
    pub(crate) rpcs3_status: rpcs3_page::Rpcs3State,
    pub(crate) rpcs3_status_generation: u64,
    pub(crate) pcsx2_status: pcsx2_page::Pcsx2StatusState,
    pub(crate) pcsx2_status_generation: u64,
    pub(crate) pcsx2_status_archive_path: Option<PathBuf>,
}

impl Default for EmulatorReadinessState {
    fn default() -> Self {
        Self {
            retroarch_profiles: RetroArchProfilesState::NotScanned,
            retroarch_core_directory_override: None,
            retroarch_core_folder_rejected_pick: None,
            emulator_setup_focus: None,
            emulator_setup_page: emulator_setup_page::EmulatorSetupPageState::default(),
            emulator_inventory_page: emulator_inventory_page::EmulatorInventoryPageState::default(),
            bios_projection_page: bios_projection_page::BiosProjectionPageState::default(),
            ready_to_play_page: ready_to_play_page::ReadyToPlayPageState::default(),
            emulator_setup_overrides: emulator_setup_overrides::EmulatorPathOverrides::default(),
            pcsx2_profiles: Pcsx2ProfilesState::NotScanned,
            dolphin_profiles: DolphinProfilesState::NotScanned,
            dolphin_local_profiles: DolphinLocalProfilesState::NotScanned,
            pcsx2_launch_profiles: Pcsx2LaunchProfilesState::NotScanned,
            flycast_profiles: FlycastProfilesState::NotScanned,
            pcsx2_firmware_evidence: Pcsx2FirmwareEvidenceState::NotLoaded,
            xenia_profiles: XeniaProfilesState::NotScanned,
            remembered_emulator_profiles: Vec::new(),
            rpcs3_status: rpcs3_page::Rpcs3State::Idle,
            rpcs3_status_generation: 0,
            pcsx2_status: pcsx2_page::Pcsx2StatusState::Idle,
            pcsx2_status_generation: 0,
            pcsx2_status_archive_path: None,
        }
    }
}

impl EmulatorReadinessState {
    pub(crate) fn new() -> Self {
        Self {
            retroarch_core_directory_override: crate::load_retroarch_core_directory_override(),
            emulator_setup_overrides: crate::emulator_setup_overrides::EmulatorPathOverrides::load(),
            remembered_emulator_profiles: crate::load_remembered_emulator_profiles_default()
                .unwrap_or_default(),
            ..Self::default()
        }
    }
}
