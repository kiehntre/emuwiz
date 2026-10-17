//! Where DuckStation cheats get their installation and their history folders.
//!
//! There is no second emulator choice here: the installation + profile come
//! from the same shared resolver Launch uses
//! ([`crate::emulator_setup::profile_resolution`]), narrowed by the same
//! launch authority. Cheats are written only into a pair Launch would start
//! exactly; a refusal, an ambiguous choice or an unusable FORCED pin leaves
//! cheats unavailable, with the resolver's own explanation.

use std::path::PathBuf;

use archivefs_core::emulator_profile_resolver::{ResolutionOutcome, ResolvedEmulatorProfile};

use crate::emulator_setup::profile_resolution::{
    DuckStationLaunchAuthority, DuckStationResolutionInputs, ProfileResolutionPresentation,
    compute_duckstation_resolution, present_resolution, select_for_launch_from_view,
};

/// The shared resolver's answer as Cheats & Mods sees it.
#[derive(Debug, Clone)]
pub(crate) struct DuckStationCheatSelection {
    pub(crate) presentation: ProfileResolutionPresentation,
    pub(crate) authority: DuckStationLaunchAuthority,
    /// Set only when Launch is authorised to start exactly this pair.
    pub(crate) profile: Option<ResolvedEmulatorProfile>,
}

pub(crate) fn select_for_cheats(inputs: &DuckStationResolutionInputs) -> DuckStationCheatSelection {
    let view = compute_duckstation_resolution(inputs);
    let launch = select_for_launch_from_view(inputs, &view);
    let profile = match (&launch.authority, &view.outcome) {
        (
            DuckStationLaunchAuthority::Authorised {
                executable,
                profile_root,
                ..
            },
            ResolutionOutcome::Resolved(resolution),
        ) if resolution.profile.executable == *executable
            && resolution.profile.profile_root == *profile_root =>
        {
            Some(resolution.profile.clone())
        }
        _ => None,
    };
    DuckStationCheatSelection {
        presentation: present_resolution(&view),
        authority: launch.authority,
        profile,
    }
}

/// The shared change-history and backup roots. Cheat installs and their undo
/// must agree on these, so this is the one place that answers. A test that did
/// not install its own temporary machine never reaches the real folders.
pub(crate) fn shared_history_roots() -> Option<(PathBuf, PathBuf)> {
    #[cfg(test)]
    return test_environment::MACHINE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|machine| (machine.history_root.clone(), machine.backup_root.clone()))
    });
    #[cfg(not(test))]
    Some((
        archivefs_core::patch_manager::default_shared_history_root().ok()?,
        archivefs_core::patch_manager::default_shared_backup_root().ok()?,
    ))
}

/// The resolver inputs Launch builds, from the same saved overrides and
/// EmuWiz-managed AppImages. `None` when DuckStation cannot be looked for.
pub(crate) fn duckstation_inputs(
    overrides: &crate::emulator_setup_overrides::EmulatorPathOverrides,
) -> Option<DuckStationResolutionInputs> {
    #[cfg(test)]
    {
        let _ = overrides;
        test_environment::MACHINE
            .with(|slot| slot.borrow().as_ref().and_then(|m| m.duckstation.clone()))
    }
    #[cfg(not(test))]
    {
        let managed =
            archivefs_core::diagnostics::profiles::discover_managed_appimage_installations();
        let executables =
            crate::emulator_setup::managed_appimage_explicit_executables(&managed, "DuckStation");
        DuckStationResolutionInputs::from_environment(overrides, &executables).ok()
    }
}

/// Test-only substitution of the machine. Thread-local, so each test sees
/// only its own temporary folders.
#[cfg(test)]
pub(crate) mod test_environment {
    use std::cell::RefCell;

    use crate::emulator_setup::profile_resolution::DuckStationResolutionInputs;

    #[derive(Clone)]
    pub(crate) struct TestMachine {
        pub(crate) duckstation: Option<DuckStationResolutionInputs>,
        pub(crate) history_root: std::path::PathBuf,
        pub(crate) backup_root: std::path::PathBuf,
    }

    thread_local! {
        pub(super) static MACHINE: RefCell<Option<TestMachine>> = const { RefCell::new(None) };
    }

    /// Restores "no machine" when dropped.
    pub(crate) struct Guard;

    pub(crate) fn install(machine: TestMachine) -> Guard {
        MACHINE.with(|slot| *slot.borrow_mut() = Some(machine));
        Guard
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            MACHINE.with(|slot| *slot.borrow_mut() = None);
        }
    }
}
