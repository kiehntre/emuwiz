//! "This emulator's Flatpak cannot use your NVIDIA card", on Blue's readiness card.
//!
//! A Flatpak emulator draws with the driver package Flatpak mounts for it, not
//! with the system's own driver. After the system's NVIDIA driver is updated,
//! that package no longer matches until Flatpak is updated too, and the
//! emulator silently falls back to software rendering: the game starts but
//! runs slowly with stuttering sound, and Vulkan cannot start at all. Nothing
//! on the card said so; the launch checks all pass.
//!
//! The machine's answer comes from
//! [`archivefs_core::launch::flatpak_graphics`] (two small reads, no process
//! started). This module only decides whether the game's chosen emulator is a
//! Flatpak and words the warning. It never withholds Play: the game does
//! start, and the person may want exactly that.

use super::{GameReadinessSummary, ReadinessPresentationState as State, selected_candidate};
use crate::launch_readiness_page::LaunchReadinessInput;
use archivefs_core::emulator_environment::retroarch::ProfileKind;
use archivefs_core::launch::LaunchTarget;
use archivefs_core::launch::flatpak_graphics::FlatpakGraphics;
use archivefs_core::patch_manager::{Rpcs3InstallationType, XemuInstallationType};

/// The plain name of the Flatpak emulator the selected candidate would start,
/// or `None` when it is not a Flatpak (or not one this module can prove is).
fn flatpak_emulator(input: &LaunchReadinessInput) -> Option<&'static str> {
    let LaunchReadinessInput::Plan {
        plan, xemu, rpcs3, ..
    } = input
    else {
        return None;
    };
    match &selected_candidate(plan)?.target {
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } => match *adapter_id {
            "xemu" => xemu
                .as_ref()?
                .discovery
                .profiles
                .iter()
                .any(|profile| {
                    profile.profile_id == *profile_id
                        && profile.installation_type == XemuInstallationType::FlatpakUser
                })
                .then_some("xemu"),
            "rpcs3" => rpcs3
                .as_ref()?
                .discovery
                .profiles
                .iter()
                .any(|profile| {
                    profile.profile_id == *profile_id
                        && profile.installation_type == Rpcs3InstallationType::FlatpakUser
                })
                .then_some("RPCS3"),
            _ => None,
        },
        LaunchTarget::RetroArchCore { profile, .. } => {
            (profile.profile_kind == ProfileKind::Flatpak).then_some("RetroArch")
        }
    }
}

/// The command that installs the matching driver package. `flatpak update`
/// fetches the package for the driver that is loaded now.
pub(crate) const FIX_COMMAND: &str = "flatpak update";

fn warning(
    emulator: &str,
    host_driver: &str,
    wanted_extension: &str,
    installed_drivers: &[String],
) -> String {
    let has = match installed_drivers {
        [] => "it has none".to_string(),
        drivers => format!("it has {}", drivers.join(", ")),
    };
    let vulkan = if emulator == "RPCS3" {
        " RPCS3's Vulkan renderer cannot start at all like this; its OpenGL renderer starts, but is drawn by the processor."
    } else {
        ""
    };
    format!(
        "{emulator} is installed as a Flatpak and cannot use your NVIDIA graphics card right now: Flatpak has no driver package for NVIDIA driver {host_driver} ({has}). Expect a slow game with stuttering sound.{vulkan} To fix it, run \"{FIX_COMMAND}\" in a terminal. If that asks for an administrator password you do not have, run \"flatpak install --user flathub {wanted_extension}\" instead: the package works from your own Flatpak installation as well as the system one. Then check again."
    )
}

/// Adds the warning to a game that is otherwise ready. A game that is blocked
/// keeps its own reason: that one has to be dealt with first.
pub(crate) fn note_flatpak_graphics(
    summary: &mut GameReadinessSummary,
    input: &LaunchReadinessInput,
    host: &FlatpakGraphics,
) {
    let FlatpakGraphics::Missing {
        host_driver,
        wanted_extension,
        installed_drivers,
    } = host
    else {
        return;
    };
    if !matches!(summary.status, State::Ready | State::ReadyWithWarnings) {
        return;
    }
    let Some(emulator) = flatpak_emulator(input) else {
        return;
    };
    // First, so the card and Setup & Doctor name it before a routine notice
    // such as unverified firmware.
    summary.warnings.insert(
        0,
        warning(emulator, host_driver, wanted_extension, installed_drivers),
    );
    summary.status = State::ReadyWithWarnings;
    summary.details.push(format!(
        "Flatpak graphics driver: NVIDIA {host_driver} is loaded; Flatpak looks for {wanted_extension}, which is not installed."
    ));
}

/// The machine's current answer, read at most once every 30 seconds: one small
/// file and two folder listings on the local disk. Under test it is whatever
/// the test set, never the real machine.
#[cfg(not(test))]
pub(crate) fn host_flatpak_graphics() -> FlatpakGraphics {
    use archivefs_core::launch::flatpak_graphics::{
        FlatpakGraphicsRoots, inspect_flatpak_graphics,
    };
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const MAX_AGE: Duration = Duration::from_secs(30);
    static CACHE: Mutex<Option<(Instant, FlatpakGraphics)>> = Mutex::new(None);

    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((read_at, answer)) = cache.as_ref()
        && read_at.elapsed() < MAX_AGE
    {
        return answer.clone();
    }
    let answer = match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => {
            inspect_flatpak_graphics(&FlatpakGraphicsRoots::for_home(std::path::Path::new(&home)))
        }
        _ => FlatpakGraphics::Unknown {
            detail: "no home directory is set".to_string(),
        },
    };
    *cache = Some((Instant::now(), answer.clone()));
    answer
}

#[cfg(test)]
thread_local! {
    static TEST_HOST: std::cell::RefCell<FlatpakGraphics> =
        const { std::cell::RefCell::new(FlatpakGraphics::NotNvidia) };
}

#[cfg(test)]
pub(crate) fn host_flatpak_graphics() -> FlatpakGraphics {
    TEST_HOST.with(|host| host.borrow().clone())
}

/// Sets what [`host_flatpak_graphics`] answers on this test thread.
#[cfg(test)]
pub(crate) fn set_test_host(host: FlatpakGraphics) {
    TEST_HOST.with(|slot| *slot.borrow_mut() = host);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch_readiness_page::{Rpcs3LaunchContext, XemuLaunchContext};
    use archivefs_core::emulator_environment::retroarch::{ProfileRef, ProfileScope};
    use archivefs_core::launch::{
        CandidatePreference, FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchCandidate,
        LaunchContentRef, LaunchPlan, LaunchPlanSummary, LaunchReadiness,
    };
    use archivefs_core::patch_manager::{
        Rpcs3ProfileDiscoveryRoots, XemuProfileDiscoveryRoots, discover_rpcs3_profiles,
        discover_xemu_profiles,
    };
    use std::path::Path;

    /// The maintainer's machine on 2026-11-20.
    fn mismatch() -> FlatpakGraphics {
        FlatpakGraphics::Missing {
            host_driver: "580.196.07".into(),
            wanted_extension: "org.freedesktop.Platform.GL.nvidia-580-196-07".into(),
            installed_drivers: vec!["580.178.04".into(), "580.173.02".into()],
        }
    }

    fn candidate(target: LaunchTarget, blockers: &[LaunchBlockerKind]) -> LaunchCandidate {
        LaunchCandidate {
            target,
            content: LaunchContentRef {
                kind: None,
                container: None,
                resolved_path: Some("/games/xbox/game.iso".into()),
                requires_mount: false,
                provenance: "synthetic".into(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: blockers
                .iter()
                .map(|kind| LaunchBlocker::new(*kind, "synthetic"))
                .collect(),
            warnings: vec![],
            readiness: if blockers.is_empty() {
                LaunchReadiness::Ready
            } else {
                LaunchReadiness::Blocked
            },
            preference: CandidatePreference::Undetermined,
        }
    }

    fn input(
        candidate: LaunchCandidate,
        xemu: Option<XemuLaunchContext>,
        rpcs3: Option<Rpcs3LaunchContext>,
    ) -> LaunchReadinessInput {
        let ready = usize::from(candidate.readiness == LaunchReadiness::Ready);
        LaunchReadinessInput::Plan {
            plan: LaunchPlan {
                platform_id: Some("Xbox".into()),
                game_key: Some("4D530002".into()),
                summary: LaunchPlanSummary {
                    candidates: 1,
                    ready,
                    ready_with_warnings: 0,
                    blocked: 1 - ready,
                },
                candidates: vec![candidate],
                media_topology: None,
            },
            retroarch: None,
            retroarch_scanned: true,
            standalone_scans_complete: true,
            dolphin: None,
            pcsx2: None,
            duckstation: None,
            ppsspp: None,
            rpcs3,
            xemu,
            xenia: None,
            melonds: None,
            cemu: None,
            flycast: None,
            xroar: None,
            azahar: None,
            vita3k: None,
            vice: None,
            amiga_whdload: None,
            #[cfg(target_os = "linux")]
            shadps4: None,
        }
    }

    fn xemu_roots(home: &Path) -> XemuProfileDiscoveryRoots {
        XemuProfileDiscoveryRoots {
            home: home.to_path_buf(),
            xdg_config_home: home.join(".config"),
            xdg_data_home: home.join(".local/share"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: Default::default(),
            appimage_directory: None,
            path_override: Some(Vec::new()),
        }
    }

    /// A home holding only the xemu Flatpak's own profile, as discovery finds it.
    fn xemu_flatpak(home: &Path) -> (XemuLaunchContext, String) {
        let profile = home.join(".var/app/app.xemu.xemu/data/xemu/xemu");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join("xemu.toml"),
            "[general]\nshow_welcome = false\n",
        )
        .unwrap();
        let roots = xemu_roots(home);
        let discovery = discover_xemu_profiles(&roots);
        let id = discovery
            .profiles
            .iter()
            .find(|profile| profile.installation_type == XemuInstallationType::FlatpakUser)
            .expect("the Flatpak profile is discovered")
            .profile_id
            .clone();
        (
            XemuLaunchContext {
                discovery,
                roots,
                verified_xbox_title_id: Some("4D530002".into()),
            },
            id,
        )
    }

    fn standalone(adapter_id: &'static str, profile_id: &str) -> LaunchTarget {
        LaunchTarget::Standalone {
            adapter_id,
            profile_id: profile_id.into(),
            profile_path: None,
        }
    }

    fn noted(input: &LaunchReadinessInput, host: &FlatpakGraphics) -> GameReadinessSummary {
        let mut summary = super::super::project(input, super::super::ReadinessFreshness::Current);
        note_flatpak_graphics(&mut summary, input, host);
        summary
    }

    #[test]
    fn a_ready_xbox_game_on_the_xemu_flatpak_is_warned_with_the_cause_and_the_fix() {
        let home = tempfile::tempdir().unwrap();
        let (context, profile_id) = xemu_flatpak(home.path());
        let input = input(
            candidate(standalone("xemu", &profile_id), &[]),
            Some(context),
            None,
        );
        let before = super::super::project(&input, super::super::ReadinessFreshness::Current);
        assert_eq!(before.status, State::Ready);

        let summary = noted(&input, &mismatch());
        assert_eq!(summary.status, State::ReadyWithWarnings);
        // Play is still offered: the game does start.
        assert_eq!(summary.primary_action, before.primary_action);
        let warning = &summary.warnings[0];
        assert!(warning.starts_with(
            "xemu is installed as a Flatpak and cannot use your NVIDIA graphics card"
        ));
        assert!(warning.contains("NVIDIA driver 580.196.07 (it has 580.178.04, 580.173.02)"));
        assert!(warning.contains("stuttering sound"));
        assert!(warning.contains("run \"flatpak update\" in a terminal"));
        assert!(warning.contains(
            "run \"flatpak install --user flathub org.freedesktop.Platform.GL.nvidia-580-196-07\" instead"
        ));
        assert!(!warning.contains("RPCS3"));
        assert!(
            summary
                .details
                .iter()
                .any(|line| { line.contains("org.freedesktop.Platform.GL.nvidia-580-196-07") })
        );
    }

    #[test]
    fn a_matching_driver_or_a_machine_without_nvidia_adds_nothing() {
        let home = tempfile::tempdir().unwrap();
        let (context, profile_id) = xemu_flatpak(home.path());
        let input = input(
            candidate(standalone("xemu", &profile_id), &[]),
            Some(context),
            None,
        );
        let plain = super::super::project(&input, super::super::ReadinessFreshness::Current);
        for host in [
            FlatpakGraphics::NotNvidia,
            FlatpakGraphics::Matching {
                host_driver: "580.196.07".into(),
            },
            FlatpakGraphics::Unknown {
                detail: "unreadable".into(),
            },
        ] {
            assert_eq!(noted(&input, &host), plain, "{host:?}");
        }
    }

    #[test]
    fn an_emulator_that_is_not_a_flatpak_is_never_warned() {
        let home = tempfile::tempdir().unwrap();
        let (context, _) = xemu_flatpak(home.path());
        // Same machine, but the candidate is another xemu profile, and a
        // standalone emulator this module has no installation evidence for.
        for target in [
            standalone("xemu", "xemu-native-user"),
            standalone("pcsx2", "default"),
        ] {
            let input = input(candidate(target, &[]), Some(context.clone()), None);
            let summary = noted(&input, &mismatch());
            assert_eq!(summary.status, State::Ready);
            assert!(summary.warnings.is_empty());
        }
    }

    #[test]
    fn a_blocked_game_keeps_its_own_reason() {
        let home = tempfile::tempdir().unwrap();
        let (context, profile_id) = xemu_flatpak(home.path());
        let input = input(
            candidate(
                standalone("xemu", &profile_id),
                &[LaunchBlockerKind::XemuContentFormatUnsupported],
            ),
            Some(context),
            None,
        );
        let plain = super::super::project(&input, super::super::ReadinessFreshness::Current);
        assert_eq!(noted(&input, &mismatch()), plain);
        assert_eq!(
            plain.findings[0].title,
            "xemu can only start an Xbox disc image (.iso)"
        );
    }

    #[test]
    fn the_rpcs3_flatpak_warning_explains_vulkan_and_opengl() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".var/app/net.rpcs3.RPCS3/config/rpcs3");
        std::fs::create_dir_all(config.join("dev_hdd0")).unwrap();
        std::fs::write(config.join("config.yml"), "Video:\n  Renderer: Vulkan\n").unwrap();
        let roots = Rpcs3ProfileDiscoveryRoots {
            home: home.path().to_path_buf(),
            xdg_config_home: home.path().join(".config"),
            xdg_data_home: home.path().join(".local/share"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: Default::default(),
            appimage_directory: None,
            selected_executable: None,
            path_override: Some(Vec::new()),
        };
        let discovery = discover_rpcs3_profiles(&roots);
        let profile_id = discovery
            .profiles
            .iter()
            .find(|profile| profile.installation_type == Rpcs3InstallationType::FlatpakUser)
            .expect("the Flatpak profile is discovered")
            .profile_id
            .clone();
        let context = Rpcs3LaunchContext {
            discovery,
            roots,
            verified_ps3_title_id: Some("BLES00000".into()),
        };
        let input = input(
            candidate(standalone("rpcs3", &profile_id), &[]),
            None,
            Some(context),
        );
        let summary = noted(&input, &mismatch());
        assert_eq!(summary.status, State::ReadyWithWarnings);
        assert!(summary.warnings[0].starts_with("RPCS3 is installed as a Flatpak"));
        assert!(summary.warnings[0].contains("Vulkan renderer cannot start at all like this"));
        assert!(
            summary.warnings[0].contains("OpenGL renderer starts, but is drawn by the processor")
        );
    }

    #[test]
    fn a_retroarch_flatpak_core_is_warned_and_a_native_one_is_not() {
        let core = |profile_kind| LaunchTarget::RetroArchCore {
            profile: ProfileRef {
                profile_kind,
                scope: ProfileScope::User,
            },
            core_stem: "puae".into(),
            platform_id: "Amiga",
        };
        let flatpak = input(candidate(core(ProfileKind::Flatpak), &[]), None, None);
        let summary = noted(&flatpak, &mismatch());
        assert!(summary.warnings[0].starts_with("RetroArch is installed as a Flatpak"));
        let native = input(candidate(core(ProfileKind::Native), &[]), None, None);
        assert!(noted(&native, &mismatch()).warnings.is_empty());
    }

    #[test]
    fn with_no_driver_package_at_all_the_warning_says_so() {
        let text = warning(
            "xemu",
            "580.196.07",
            "org.freedesktop.Platform.GL.nvidia-580-196-07",
            &[],
        );
        assert!(text.contains("NVIDIA driver 580.196.07 (it has none)"));
    }

    /// Read-only and run by hand: prints what Blue says on this machine.
    /// `EMUWIZ_PROBE_HOME="$HOME" scripts/cargo-iso test -p archivefs-gui --lib \
    ///  flatpak_graphics::tests::this_machine -- --ignored --nocapture`
    #[test]
    #[ignore = "reads this machine's driver version and Flatpak folders; run by hand"]
    fn this_machine() {
        use archivefs_core::launch::flatpak_graphics::{
            FlatpakGraphicsRoots, inspect_flatpak_graphics,
        };
        let home = std::env::var_os("EMUWIZ_PROBE_HOME")
            .expect("set EMUWIZ_PROBE_HOME to the home folder to inspect");
        let host = inspect_flatpak_graphics(&FlatpakGraphicsRoots::for_home(Path::new(&home)));
        println!("{host:#?}");
        if let FlatpakGraphics::Missing {
            host_driver,
            wanted_extension,
            installed_drivers,
        } = &host
        {
            for emulator in ["xemu", "RPCS3"] {
                println!(
                    "\n{}",
                    warning(emulator, host_driver, wanted_extension, installed_drivers)
                );
            }
        }
    }

    #[test]
    fn tests_never_read_the_real_machine() {
        assert_eq!(host_flatpak_graphics(), FlatpakGraphics::NotNvidia);
        set_test_host(mismatch());
        assert_eq!(host_flatpak_graphics(), mismatch());
        set_test_host(FlatpakGraphics::NotNvidia);
    }
}
