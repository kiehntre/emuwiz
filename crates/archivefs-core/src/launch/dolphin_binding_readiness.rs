//! Launch readiness for discovered Dolphin profiles.
//!
//! A Dolphin profile is a launch option only if a launch binding (one exact
//! Dolphin program and user-directory mode) can be proven for it right now.
//! A profile whose `Dolphin.ini` exists but has no usable program is shown as
//! blocked with the binding's own reason. It is never shown as Ready and it
//! never joins a tie with options that can actually start.
//!
//! This only narrows readiness. Launch preflight still resolves the binding
//! again, from scratch, immediately before starting Dolphin.

use super::planning::{LaunchPlan, LaunchTarget, StandaloneProfileInput};
use super::readiness::{FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{
    DolphinLaunchBlocker, DolphinLaunchBlockerKind, DolphinLocalDiscoveryRoots,
    DolphinLocalProfileDiscovery, resolve_dolphin_native_launch_binding,
};

/// Planner inputs for every discovered Dolphin profile, plus the binding
/// refusal for each profile that is otherwise eligible but cannot start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinLaunchProfiles {
    pub inputs: Vec<StandaloneProfileInput>,
    pub unavailable: Vec<(String, DolphinLaunchBlocker)>,
}

pub fn dolphin_launch_profiles(
    discovery: &DolphinLocalProfileDiscovery,
    roots: &DolphinLocalDiscoveryRoots,
) -> DolphinLaunchProfiles {
    let mut unavailable = Vec::new();
    let inputs = discovery
        .profiles
        .iter()
        .map(|profile| {
            let mut eligible = profile.eligible;
            if eligible && let Err(blocker) = resolve_dolphin_native_launch_binding(profile, roots)
            {
                eligible = false;
                unavailable.push((profile.profile_id.clone(), blocker));
            }
            StandaloneProfileInput {
                adapter_id: "dolphin",
                profile_id: profile.profile_id.clone(),
                profile_path: Some(profile.configuration_root.clone()),
                eligible,
                firmware: FirmwareReadiness::NotRequired,
            }
        })
        .collect();
    DolphinLaunchProfiles {
        inputs,
        unavailable,
    }
}

/// Replaces the planner's generic "profile is not eligible" blocker with the
/// binding's real reason, for exactly the profiles in `profiles.unavailable`.
pub fn apply(plan: &mut LaunchPlan, profiles: &DolphinLaunchProfiles) {
    for candidate in &mut plan.candidates {
        let LaunchTarget::Standalone {
            adapter_id: "dolphin",
            profile_id,
            ..
        } = &candidate.target
        else {
            continue;
        };
        let Some((_, refusal)) = profiles.unavailable.iter().find(|(id, _)| id == profile_id)
        else {
            continue;
        };
        let reason = LaunchBlocker::new(
            LaunchBlockerKind::DolphinBindingUnavailable,
            message(refusal),
        );
        candidate
            .blockers
            .retain(|blocker| blocker.kind != LaunchBlockerKind::ProfileIneligible);
        if !candidate.blockers.contains(&reason) {
            candidate.blockers.insert(0, reason);
        }
        candidate.readiness = LaunchReadiness::Blocked;
    }
    plan.summary = super::planning::summarize(&plan.candidates);
}

fn message(refusal: &DolphinLaunchBlocker) -> String {
    use DolphinLaunchBlockerKind as K;
    let summary = match refusal.kind {
        K::ExecutableMissing => "No Dolphin program was found for this Dolphin settings folder.",
        K::AmbiguousExecutable => {
            "More than one Dolphin program matches this settings folder, so none is used."
        }
        K::ExecutableUnsafe | K::ExecutableNotExecutable => {
            "The Dolphin program for this settings folder is not a safe, executable file."
        }
        K::UnsupportedInstallationType => {
            "EmuWiz cannot start Dolphin from this kind of installation."
        }
        K::ProfileRootMismatch
        | K::DefaultResolutionMismatch
        | K::ExplicitRootInvalid
        | K::EnvironmentOverridePresent
        | K::PortableOrLegacyLayoutConflict => {
            "Dolphin would not use this settings folder, so EmuWiz cannot start it from here."
        }
    };
    format!("{summary} Detail: {}", refusal.detail)
}

#[cfg(test)]
mod tests;
