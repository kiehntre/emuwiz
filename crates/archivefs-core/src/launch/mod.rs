//! Unified Launch Planning - Phase 1.
//!
//! Given a canonically identified game and the emulator/RetroArch-core
//! environment already discovered on this machine, this module produces a
//! read-only [`planning::LaunchPlan`] describing which launches are
//! possible and, for each, whether it is ready, ready with warnings, or
//! blocked - and why.
//!
//! # What this module is not
//!
//! - Every submodule except [`execution`] never starts a process.
//!   [`planning::build_launch_plan`] is a pure function and never spawns
//!   anything; [`execution`] is the one deliberate, narrowly-scoped
//!   exception - see its own module doc comment for exactly what it does
//!   and does not launch (Phase 1: one native RetroArch process, one direct
//!   loose regular content file, nothing else).
//! - It never mutates an emulator's configuration, mounts an archive,
//!   downloads firmware, or writes anything to disk.
//! - It does not replace any adapter's own BIOS/firmware state enum (see
//!   [`readiness`]'s module doc) and it does not retrofit the nine
//!   `patch_manager::*_local` adapters behind one trait.
//! - It does not perform identity fusion. Canonical identity is resolved
//!   entirely upstream (`platform_evidence_fusion`, `platform::identity`,
//!   `game_identity`); this module only ever consumes an already-resolved
//!   [`planning::CanonicalIdentityStatus`] and fails closed whenever it is
//!   not [`planning::CanonicalIdentityStatus::Resolved`].
//!
//! # Layout
//!
//! - [`readiness`] - the shared vocabulary for "is this playable right
//!   now" ([`readiness::FirmwareReadiness`], [`readiness::LaunchReadiness`],
//!   blocker/warning kinds) plus pure projections from each adapter's own
//!   existing BIOS/firmware/TOS state enum - none of which are changed or
//!   replaced.
//! - [`platform_map`] - the small, reviewed canonical-platform ->
//!   standalone-adapter table covering the platforms already supported by
//!   an adapter in this build.
//! - [`planning`] - the content/target/candidate/plan data model and the
//!   pure [`planning::build_launch_plan`] planner.
//! - [`input_projection`] - pure projection from already-verified identity
//!   facts to each Phase 1 adapter's own launch-input request type (or an
//!   explicit unavailable result); never executes anything and never
//!   fuses or promotes identity itself.
//! - [`es_de_export`] - a read-only ES-DE export/launch-entry plan built
//!   from an already-resolved canonical identity and content path; never
//!   writes `es_systems.xml`/`gamelist.xml` and never launches ES-DE.
//! - [`evidence_bridge`] - the narrow, pure conversion from EmuWiz's
//!   existing authoritative identity/content evidence
//!   ([`crate::game_identity::GameIdentityReport`], [`crate::ArchiveRecord`])
//!   into [`planning::CanonicalIdentityStatus`]/[`input_projection::VerifiedIdentityFact`]/
//!   [`planning::LaunchContentRef`]; never resolves identity or mounts
//!   anything itself.
//! - [`execution`] - the first supported slice of real launch execution:
//!   live-revalidates a user-authorized launch request from scratch
//!   (fresh identity re-inspection, fresh RetroArch environment discovery,
//!   a freshly rebuilt plan/command) and, only if every check still holds,
//!   spawns exactly one native RetroArch process via
//!   `std::process::Command` - never a shell. Nothing else in this module
//!   spawns anything; see its own module doc comment for the exact scope.
//!
//! See `docs/PATCH_CHEAT_MANAGER_DESIGN.md` and `ROADMAP.md`'s
//! "Launch-preparation workflows" note for the wider design context this
//! module implements the first slice of.

pub mod amiberry_command;
pub mod dolphin_command;
pub mod dolphin_execution;
pub mod dosbox_command;
pub mod dosbox_execution;
pub mod duckstation_command;
pub mod duckstation_execution;
pub mod es_de_export;
pub mod es_de_publish;
pub mod evidence_bridge;
pub mod execution;
pub mod fbneo_command;
pub mod fbneo_execution;
pub mod flycast_command;
pub mod flycast_execution;
pub mod fsuae_command;
pub mod fsuae_execution;
pub mod input_projection;
pub mod integration;
pub mod mame_command;
pub mod mame_execution;
pub mod melonds_command;
pub mod melonds_execution;
pub mod mgba_command;
pub mod mgba_execution;
pub mod pcsx2_command;
pub mod pcsx2_execution;
pub mod planning;
pub mod platform_map;
pub mod ppsspp_command;
pub mod ppsspp_execution;
pub mod process_spawn;
pub mod readiness;
pub mod retroarch_command;
pub mod rpcs3_command;
pub mod rpcs3_execution;
pub mod scummvm_command;
pub mod scummvm_execution;
pub mod xemu_command;
pub mod xemu_execution;
pub mod xenia_command;
pub mod xenia_execution;

#[cfg(test)]
mod plan_to_spawn_tests;
#[cfg(test)]
mod tests;

pub use dolphin_command::{
    DOLPHIN_SUPPORTED_PLATFORM_ID, DolphinCommand, DolphinCommandPlan, DolphinCommandSelection,
    build_dolphin_command_plan,
};
pub use dolphin_execution::{
    DolphinLaunchCommandFacts, DolphinLaunchExecutionError, DolphinLaunchExitReport,
    DolphinLaunchPreflightError, DolphinLaunchPreflightErrorKind, DolphinLaunchRequest,
    DolphinLaunchSpawnError, LaunchedDolphinProcess, preflight_and_launch_dolphin,
    preflight_dolphin_launch, spawn_dolphin,
};
pub use dosbox_command::{
    DOSBOX_CONFIG_FILE_NAME, DOSBOX_CONFIG_FLAG, DOSBOX_SUPPORTED_PLATFORM_ID,
    DosBoxBindingRefusal, DosBoxCommand, DosBoxCommandPlan, DosBoxCommandSelection,
    DosBoxConfigStatus, DosBoxNativeLaunchBinding, DosBoxVariant, build_dosbox_command_plan,
    discover_dosbox_config, dosbox_config_status_from_inspection, dosbox_variant_from_id,
    resolve_dosbox_native_launch_binding, resolve_dosbox_native_launch_binding_at,
    resolve_dosbox_native_launch_binding_from_id,
};
pub use dosbox_execution::{
    DosBoxLaunchExecutionError, DosBoxLaunchExitReport, DosBoxLaunchPreflightError,
    DosBoxLaunchPreflightErrorKind, DosBoxLaunchRequest, DosBoxLaunchSpawnError,
    LaunchedDosBoxProcess, preflight_and_launch_dosbox, preflight_dosbox_launch, spawn_dosbox,
};
pub use duckstation_command::{
    DUCKSTATION_SUPPORTED_PLATFORM_ID, DuckStationCommand, DuckStationCommandPlan,
    DuckStationCommandSelection, build_duckstation_command_plan,
};
pub use duckstation_execution::{
    DuckStationLaunchCommandFacts, DuckStationLaunchExecutionError, DuckStationLaunchExitReport,
    DuckStationLaunchPreflightError, DuckStationLaunchPreflightErrorKind, DuckStationLaunchRequest,
    DuckStationLaunchSpawnError, LaunchedDuckStationProcess, preflight_and_launch_duckstation,
    preflight_duckstation_launch, spawn_duckstation,
};
pub use es_de_export::{
    ES_DE_SYSTEM_MAP, EsDeEntryBlocker, EsDeEntryBlockerKind, EsDeEntryPlan, EsDeEntryStatus,
    EsDeExportOutcome, EsDeSystemMapping, NoEntryReason, build_es_de_entry_plan,
    es_de_system_for_platform,
};
pub use evidence_bridge::{
    canonical_identity_from_game_report, launch_content_ref_from_archive_record,
};
pub use execution::{
    LAUNCH_STDERR_CAPTURE_LIMIT, LaunchCommandFacts, LaunchContentIdentity, LaunchExecutionError,
    LaunchExitReport, LaunchPreflightError, LaunchPreflightErrorKind, LaunchSpawnError,
    LaunchedRetroArchProcess, RetroArchLaunchRequest, preflight_and_launch_retroarch,
    preflight_retroarch_launch, spawn_retroarch,
};
pub use fbneo_command::{
    FBNEO_SUPPORTED_PLATFORM_ID, FbneoCommand, FbneoCommandPlan, FbneoIdentityEvidence,
    FbneoSetEvidence, build_fbneo_command_plan,
};
pub use fbneo_execution::{
    FbneoLaunchExecutionError, FbneoLaunchPreflightError, FbneoLaunchRequest,
    preflight_and_launch_fbneo, preflight_fbneo_launch, spawn_fbneo,
};
pub use flycast_command::{
    FLYCAST_SUPPORTED_PLATFORM_ID, FlycastCommand, FlycastCommandPlan, FlycastCommandSelection,
    build_flycast_command_plan,
};
pub use flycast_execution::{
    FlycastLaunchCommandFacts, FlycastLaunchExecutionError, FlycastLaunchExitReport,
    FlycastLaunchPreflightError, FlycastLaunchPreflightErrorKind, FlycastLaunchRequest,
    FlycastLaunchSpawnError, LaunchedFlycastProcess, preflight_and_launch_flycast,
    preflight_flycast_launch, spawn_flycast,
};
pub use fsuae_command::{
    FSUAE_SUPPORTED_PLATFORM_ID, FsUaeCommand, FsUaeCommandPlan, FsUaeCommandSelection,
    FsUaeLaunchBlocker, FsUaeLaunchBlockerKind, FsUaeNativeLaunchBinding, build_fsuae_command_plan,
    resolve_fsuae_native_launch_binding,
};
pub use fsuae_execution::{
    FsUaeLaunchPreflightError, FsUaeLaunchPreflightErrorKind, FsUaeLaunchRequest,
    preflight_fsuae_launch, spawn_fsuae,
};
pub use input_projection::{
    LaunchInputProjection, SegaCdGameRequest, VerifiedIdentityFact,
    project_amiga_whdload_launch_input, project_dolphin_gamecube_launch_input,
    project_dolphin_wii_launch_input, project_duckstation_launch_input,
    project_flycast_launch_input, project_hatari_launch_input, project_melonds_launch_input,
    project_pcsx2_launch_input, project_ppsspp_launch_input, project_rpcs3_launch_input,
    project_sega_cd_launch_input, project_xemu_launch_input, project_xenia_launch_input,
};
pub use integration::{
    DiscoveredStandaloneProfile, LaunchPlanResults, build_launch_plan_from_results,
};
pub use mame_command::{
    MAME_SUPPORTED_PLATFORM_IDS, MameCommand, MameCommandPlan, build_mame_command_plan,
};
pub use mame_execution::{
    MameLaunchExecutionError, MameLaunchPreflightError, MameLaunchRequest,
    preflight_and_launch_mame, preflight_mame_launch, spawn_mame,
};
pub use melonds_command::{
    MELONDS_SUPPORTED_PLATFORM_ID, MelonDsCommand, MelonDsCommandPlan, MelonDsCommandSelection,
    build_melonds_command_plan,
};
pub use melonds_execution::{
    MelonDsLaunchPreflightError, MelonDsLaunchPreflightErrorKind, MelonDsLaunchRequest,
    preflight_melonds_launch, spawn_melonds,
};
pub use mgba_command::{
    MGBA_SUPPORTED_PLATFORM_IDS, MgbaCommand, MgbaCommandPlan, MgbaCommandSelection,
    build_mgba_command_plan,
};
pub use mgba_execution::{
    MgbaLaunchPreflightError, MgbaLaunchPreflightErrorKind, MgbaLaunchRequest,
    preflight_mgba_launch, spawn_mgba,
};
pub use pcsx2_command::{
    PCSX2_SUPPORTED_PLATFORM_ID, Pcsx2Command, Pcsx2CommandPlan, Pcsx2CommandSelection,
    build_pcsx2_command_plan,
};
pub use pcsx2_execution::{
    LaunchedPcsx2Process, Pcsx2LaunchCommandFacts, Pcsx2LaunchExecutionError,
    Pcsx2LaunchExitReport, Pcsx2LaunchPreflightError, Pcsx2LaunchPreflightErrorKind,
    Pcsx2LaunchRequest, Pcsx2LaunchSpawnError, preflight_and_launch_pcsx2, preflight_pcsx2_launch,
    spawn_pcsx2,
};
pub use planning::{
    CandidatePreference, CanonicalIdentityStatus, LaunchCandidate, LaunchContainerKind,
    LaunchContentKind, LaunchContentRef, LaunchPlan, LaunchPlanSummary, LaunchTarget,
    RememberedPreference, ResolvedIdentity, StandaloneProfileInput, build_launch_plan,
};
pub use platform_map::{
    LAUNCH_COMPATIBILITY, LaunchCompatibility, MappingConfidence, extension_narrows_candidate,
    launch_compatibility_for_platform, platforms_for_standalone_adapter,
    retroarch_platform_candidate,
};
pub use ppsspp_command::{
    PPSSPP_SUPPORTED_PLATFORM_ID, PpssppCommand, PpssppCommandPlan, PpssppCommandSelection,
    build_ppsspp_command_plan,
};
pub use ppsspp_execution::{
    LaunchedPpssppProcess, PpssppLaunchCommandFacts, PpssppLaunchExecutionError,
    PpssppLaunchExitReport, PpssppLaunchPreflightError, PpssppLaunchPreflightErrorKind,
    PpssppLaunchRequest, PpssppLaunchSpawnError, preflight_and_launch_ppsspp,
    preflight_ppsspp_launch, spawn_ppsspp,
};
pub use readiness::{
    FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchReadiness, LaunchWarning,
    LaunchWarningKind, duckstation_firmware_readiness, flycast_firmware_readiness,
    hatari_firmware_readiness, pcengine_cd_firmware_readiness, pcsx2_firmware_readiness,
    ppsspp_firmware_readiness, retroarch_core_firmware_readiness, rpcs3_firmware_readiness,
    xemu_firmware_readiness, xenia_firmware_readiness,
};
pub use retroarch_command::{
    RetroArchCommand, RetroArchCommandPlan, RetroArchCommandSelection, build_retroarch_command_plan,
};
pub use rpcs3_command::{
    RPCS3_SUPPORTED_PLATFORM_ID, Rpcs3Command, Rpcs3CommandPlan, Rpcs3CommandSelection,
    build_rpcs3_command_plan,
};
pub use rpcs3_execution::{
    LaunchedRpcs3Process, Rpcs3LaunchCommandFacts, Rpcs3LaunchExecutionError,
    Rpcs3LaunchExitReport, Rpcs3LaunchPreflightError, Rpcs3LaunchPreflightErrorKind,
    Rpcs3LaunchRequest, Rpcs3LaunchSpawnError, preflight_and_launch_rpcs3, preflight_rpcs3_launch,
    spawn_rpcs3,
};
pub use scummvm_command::{
    SCUMMVM_SUPPORTED_PLATFORM_ID, ScummVmCommand, ScummVmCommandPlan, ScummVmCommandSelection,
    ScummVmNativeLaunchBinding, build_scummvm_command_plan, resolve_scummvm_native_launch_binding,
    resolve_scummvm_native_launch_binding_at,
};
pub use scummvm_execution::{
    LaunchedScummVmProcess, ScummVmLaunchExecutionError, ScummVmLaunchPreflightError,
    ScummVmLaunchPreflightErrorKind, ScummVmLaunchRequest, ScummVmLaunchSpawnError,
    preflight_and_launch_scummvm, preflight_scummvm_launch, spawn_scummvm,
};
pub use xemu_command::{
    XEMU_SUPPORTED_PLATFORM_ID, XemuCommand, XemuCommandPlan, XemuCommandSelection,
    build_xemu_command_plan,
};
pub use xemu_execution::{
    LaunchedXemuProcess, XemuLaunchCommandFacts, XemuLaunchExecutionError, XemuLaunchExitReport,
    XemuLaunchPreflightError, XemuLaunchPreflightErrorKind, XemuLaunchRequest,
    XemuLaunchSpawnError, preflight_and_launch_xemu, preflight_xemu_launch, spawn_xemu,
};
pub use xenia_command::{
    XENIA_SUPPORTED_PLATFORM_ID, XeniaCommand, XeniaCommandPlan, XeniaCommandSelection,
    build_xenia_command_plan,
};
pub use xenia_execution::{
    LaunchedXeniaProcess, XeniaLaunchCommandFacts, XeniaLaunchExecutionError,
    XeniaLaunchExitReport, XeniaLaunchPreflightError, XeniaLaunchPreflightErrorKind,
    XeniaLaunchRequest, XeniaLaunchSpawnError, preflight_and_launch_xenia, preflight_xenia_launch,
    spawn_xenia,
};
