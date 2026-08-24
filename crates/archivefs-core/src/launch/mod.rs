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

pub mod dolphin_command;
pub mod dolphin_execution;
pub mod es_de_export;
pub mod evidence_bridge;
pub mod execution;
pub mod input_projection;
pub mod integration;
pub mod planning;
pub mod platform_map;
pub mod process_spawn;
pub mod readiness;
pub mod retroarch_command;

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
pub use input_projection::{
    LaunchInputProjection, VerifiedIdentityFact, project_amiga_whdload_launch_input,
    project_dolphin_gamecube_launch_input, project_dolphin_wii_launch_input,
    project_duckstation_launch_input, project_flycast_launch_input, project_hatari_launch_input,
    project_pcsx2_launch_input, project_ppsspp_launch_input, project_rpcs3_launch_input,
    project_xemu_launch_input, project_xenia_launch_input,
};
pub use integration::{
    DiscoveredStandaloneProfile, LaunchPlanResults, build_launch_plan_from_results,
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
pub use readiness::{
    FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchReadiness, LaunchWarning,
    LaunchWarningKind, duckstation_firmware_readiness, flycast_firmware_readiness,
    hatari_firmware_readiness, pcsx2_firmware_readiness, ppsspp_firmware_readiness,
    retroarch_core_firmware_readiness, rpcs3_firmware_readiness, xemu_firmware_readiness,
};
pub use retroarch_command::{
    RetroArchCommand, RetroArchCommandPlan, RetroArchCommandSelection, build_retroarch_command_plan,
};
