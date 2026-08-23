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
//! - It never starts a process. Nothing here reaches `std::process::Command`
//!   for an emulator - [`planning::build_launch_plan`] is a pure function
//!   and never spawns anything.
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
//!
//! See `docs/PATCH_CHEAT_MANAGER_DESIGN.md` and `ROADMAP.md`'s
//! "Launch-preparation workflows" note for the wider design context this
//! module implements the first slice of.

pub mod planning;
pub mod platform_map;
pub mod readiness;

#[cfg(test)]
mod tests;

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
