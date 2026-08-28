//! Read-only RetroArch command planning.
//!
//! This module turns one already-authorized RetroArch
//! [`LaunchCandidate`] and the already-inspected
//! [`RetroArchEnvironmentReport`] into argv-shaped data.  It never
//! re-discovers an installation, checks the live filesystem, mounts content,
//! writes a configuration file, constructs shell text, or starts a process.
//!
//! The environment report's public paths are intentionally display-safe.  A
//! lossy path cannot be converted back to the exact original path, so this
//! planner refuses it rather than guessing.  Valid UTF-8 paths (including
//! spaces, quotes, shell-looking characters, and Unicode) are carried as one
//! `OsString` argument each.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::emulator_environment::EncodedPath;
use crate::emulator_environment::retroarch::{
    AppImageIdentificationConfidence, CoreFinding, ExecutableState, ProfileRef,
    RetroArchEnvironmentReport, RetroArchProfile,
};
use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::platform_map::retroarch_platform_matches;
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};

/// The executable invocation data for a launch that has passed every
/// fail-closed check.  This is data only: no type in this module implements
/// process spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: RetroArchCommandSelection,
}

/// The inspected environment facts that selected the command's profile and
/// exact core file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchCommandSelection {
    pub profile: ProfileRef,
    pub core_stem: String,
    pub platform_id: String,
    pub core_library: PathBuf,
    pub content_path: PathBuf,
}

/// A successful command, or the structured reasons a command was withheld.
/// `command` is `None` whenever `blockers` is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchCommandPlan {
    pub command: Option<RetroArchCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

impl RetroArchCommandPlan {
    fn blocked(blockers: Vec<LaunchBlocker>) -> Self {
        debug_assert!(!blockers.is_empty());
        Self {
            command: None,
            blockers,
        }
    }
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

fn exact_path(
    encoded: &EncodedPath,
    role: &'static str,
    blockers: &mut Vec<LaunchBlocker>,
) -> Option<PathBuf> {
    if encoded.lossy {
        blockers.push(blocker(
            LaunchBlockerKind::RetroArchPathNotExact,
            format!("the inspected {role} path is lossy, so its exact filesystem path cannot be reconstructed"),
        ));
        None
    } else {
        Some(PathBuf::from(&encoded.display))
    }
}

fn selected_profile<'a>(
    environment: &'a RetroArchEnvironmentReport,
    profile_ref: ProfileRef,
    blockers: &mut Vec<LaunchBlocker>,
) -> Option<&'a RetroArchProfile> {
    let matching: Vec<&RetroArchProfile> = environment
        .profiles
        .iter()
        .filter(|profile| {
            profile.profile_kind == profile_ref.profile_kind && profile.scope == profile_ref.scope
        })
        .collect();
    match matching.as_slice() {
        [profile] => Some(*profile),
        [] => {
            blockers.push(blocker(
                LaunchBlockerKind::RetroArchProfileMissing,
                "the candidate's RetroArch profile is no longer present in the inspected environment",
            ));
            None
        }
        _ => {
            blockers.push(blocker(
                LaunchBlockerKind::AmbiguousRetroArchProfile,
                "more than one inspected RetroArch profile matches the candidate's profile reference",
            ));
            None
        }
    }
}

fn selected_core<'a>(
    profile: &'a RetroArchProfile,
    stem: &str,
    platform_id: &str,
    blockers: &mut Vec<LaunchBlocker>,
) -> Option<&'a CoreFinding> {
    let matching: Vec<&CoreFinding> = profile
        .cores
        .iter()
        .filter(|core| core.core_stem == stem)
        .collect();
    let core = match matching.as_slice() {
        [core] => *core,
        [] => {
            blockers.push(blocker(
                LaunchBlockerKind::CoreMissing,
                format!(
                    "the selected RetroArch core `{stem}` is no longer installed for this profile"
                ),
            ));
            return None;
        }
        _ => {
            blockers.push(blocker(
                LaunchBlockerKind::AmbiguousCore,
                format!(
                    "more than one installed core has the selected RetroArch core stem `{stem}`"
                ),
            ));
            return None;
        }
    };

    if !retroarch_platform_matches(&core.info, platform_id) {
        blockers.push(blocker(
            LaunchBlockerKind::RetroArchCoreMismatch,
            format!("the selected RetroArch core `{stem}` no longer resolves to the candidate platform `{platform_id}`"),
        ));
        return None;
    }
    Some(core)
}

fn executable_paths(profile: &RetroArchProfile) -> Vec<&EncodedPath> {
    match profile.profile_kind {
        // The native executable finder has already verified these PATH
        // entries as regular executable files.  Do not supplement them with
        // AppImages: a native executable and an AppImage are distinct launch
        // targets, and silently choosing one would be unsafe.
        crate::emulator_environment::retroarch::ProfileKind::Native => {
            profile.evidence.executables.iter().collect()
        }
        crate::emulator_environment::retroarch::ProfileKind::AppImage => profile
            .app_images
            .iter()
            .filter(|candidate| {
                candidate.confidence == AppImageIdentificationConfidence::Exact
                    && candidate.executable == Some(ExecutableState::Executable)
            })
            .map(|candidate| &candidate.path)
            .collect(),
        // The report proves an installed Flatpak application, but it does
        // not contain an exact executable path.  Planning `flatpak run ...`
        // would instead be a launcher command and would invent an executable
        // absent from the inspected report, so it is intentionally blocked.
        crate::emulator_environment::retroarch::ProfileKind::Flatpak => Vec::new(),
    }
}

/// Builds a safe RetroArch argv plan from only an already-authorized launch
/// candidate and an already-completed RetroArch environment report.
///
/// `identity` is passed separately so this boundary can still fail closed if
/// a caller retained a stale candidate after identity became unknown or
/// conflicting.  The normal argument order is exactly `-L`, core library,
/// content path, as separate `OsString` values.
pub fn build_retroarch_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    environment: &RetroArchEnvironmentReport,
) -> RetroArchCommandPlan {
    let mut blockers = Vec::new();

    match identity {
        CanonicalIdentityStatus::Resolved(_) => {}
        CanonicalIdentityStatus::Unknown => blockers.push(blocker(
            LaunchBlockerKind::IdentityUnresolved,
            "canonical game identity could not be resolved",
        )),
        CanonicalIdentityStatus::Conflicting => blockers.push(blocker(
            LaunchBlockerKind::IdentityConflict,
            "canonical game identity evidence conflicts and was not resolved to one answer",
        )),
    }

    let LaunchTarget::RetroArchCore {
        profile: profile_ref,
        core_stem,
        platform_id,
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::RetroArchCandidateRequired,
            "the supplied launch candidate does not target a RetroArch core",
        ));
        return RetroArchCommandPlan::blocked(blockers);
    };

    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied RetroArch launch candidate is marked blocked",
            ));
        }
    }
    if candidate.content.requires_mount {
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "content requires a mount that has not been performed, so no command can be produced",
        ));
    }
    let content_path = match &candidate.content.resolved_path {
        Some(path) if !candidate.content.requires_mount => Some(path.clone()),
        _ => {
            blockers.push(blocker(
                LaunchBlockerKind::ContentNotResolved,
                "no resolved runnable game/content path is available",
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return RetroArchCommandPlan::blocked(blockers);
    }

    let Some(profile) = selected_profile(environment, *profile_ref, &mut blockers) else {
        return RetroArchCommandPlan::blocked(blockers);
    };
    let Some(core) = selected_core(profile, core_stem, platform_id, &mut blockers) else {
        return RetroArchCommandPlan::blocked(blockers);
    };

    let executables = executable_paths(profile);
    let executable = match executables.as_slice() {
        [] => {
            blockers.push(blocker(
                LaunchBlockerKind::RetroArchExecutableMissing,
                "no exact executable RetroArch path is available for the selected profile",
            ));
            None
        }
        [executable] => exact_path(executable, "RetroArch executable", &mut blockers),
        _ => {
            blockers.push(blocker(
                LaunchBlockerKind::AmbiguousRetroArchExecutable,
                "more than one exact RetroArch executable is available for the selected profile",
            ));
            None
        }
    };
    let core_library = exact_path(&core.full_path, "RetroArch core library", &mut blockers);

    if !blockers.is_empty() {
        return RetroArchCommandPlan::blocked(blockers);
    }

    let executable = executable.expect("a single executable is required when no blockers exist");
    let core_library =
        core_library.expect("a lossless core path is required when no blockers exist");
    let content_path =
        content_path.expect("a resolved content path is required when no blockers exist");
    let arguments = vec![
        OsString::from("-L"),
        core_library.clone().into_os_string(),
        content_path.clone().into_os_string(),
    ];
    RetroArchCommandPlan {
        command: Some(RetroArchCommand {
            executable,
            arguments,
            working_directory: None,
            selection: RetroArchCommandSelection {
                profile: *profile_ref,
                core_stem: core_stem.clone(),
                platform_id: (*platform_id).to_string(),
                core_library,
                content_path,
            },
        }),
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::emulator_environment::retroarch::{
        ConfigFileFinding, ConfigReadOutcome, CoreInfoFinding, DirectoryProbeFinding, Evidence,
        ProfileKind, ProfileScope, RetroArchPlaylistInventory, RetroArchProfile,
    };
    use crate::emulator_environment::{EncodedPath, FsProbe};
    use crate::launch::planning::{
        CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
        ResolvedIdentity,
    };
    use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};

    fn resolved() -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "PSX".to_string(),
            game_key: "SLUS-00001".to_string(),
        })
    }

    fn profile_ref() -> ProfileRef {
        ProfileRef {
            profile_kind: ProfileKind::Native,
            scope: ProfileScope::User,
        }
    }

    fn core(stem: &str, system_name: Option<&str>) -> CoreFinding {
        core_with_database(stem, system_name, None)
    }

    fn core_with_database(
        stem: &str,
        system_name: Option<&str>,
        database: Option<&str>,
    ) -> CoreFinding {
        CoreFinding {
            file_name: EncodedPath::from_path(&PathBuf::from(format!("{stem}_libretro.so"))),
            full_path: EncodedPath::from_path(&PathBuf::from(format!(
                "/retroarch/cores/{stem}_libretro.so"
            ))),
            core_stem: stem.to_string(),
            info: CoreInfoFinding::Found {
                display_name: None,
                display_version: None,
                system_name: system_name.map(str::to_string),
                supported_extensions: Vec::new(),
                core_name: Some(stem.to_string()),
                manufacturer: None,
                categories: None,
                database: database.map(str::to_string),
                firmware: Vec::new(),
            },
        }
    }

    fn report(executables: Vec<&str>, cores: Vec<CoreFinding>) -> RetroArchEnvironmentReport {
        RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![RetroArchProfile {
                profile_kind: ProfileKind::Native,
                scope: ProfileScope::User,
                evidence: Evidence {
                    executables: executables
                        .into_iter()
                        .map(|path| EncodedPath::from_path(&PathBuf::from(path)))
                        .collect(),
                    flatpak_metadata_found: false,
                    config_directory_found: true,
                    config_file_found: true,
                },
                config_directory: DirectoryProbeFinding {
                    path: EncodedPath::from_path(&PathBuf::from("/retroarch")),
                    probe: FsProbe::PresentDirectory,
                },
                config_file: ConfigFileFinding {
                    path: EncodedPath::from_path(&PathBuf::from("/retroarch/retroarch.cfg")),
                    probe: FsProbe::PresentFile,
                    read: ConfigReadOutcome::NotAttempted,
                },
                paths: Vec::new(),
                cores,
                playlists: RetroArchPlaylistInventory {
                    directory: None,
                    playlists: Vec::new(),
                    diagnostics: Vec::new(),
                    complete: true,
                },
                app_images: Vec::new(),
                diagnostics: Vec::new(),
            }],
            diagnostics: Vec::new(),
        }
    }

    fn candidate(path: Option<PathBuf>) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::RetroArchCore {
                profile: profile_ref(),
                core_stem: "mednafen_psx".to_string(),
                platform_id: "PSX",
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::OpticalDisc),
                container: Some(LaunchContainerKind::Chd),
                resolved_path: path,
                requires_mount: false,
                provenance: "already resolved content".to_string(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: Vec::new(),
            warnings: Vec::new(),
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::SoleEligible,
        }
    }

    fn has_blocker(plan: &RetroArchCommandPlan, kind: LaunchBlockerKind) -> bool {
        plan.blockers.iter().any(|blocker| blocker.kind == kind)
    }

    #[test]
    fn genesis_plus_gx_metadata_plans_a_sega_cd_command_without_changing_content_path() {
        let mut selected = candidate(Some(PathBuf::from("/games/actual-title.cue")));
        selected.target = LaunchTarget::RetroArchCore {
            profile: profile_ref(),
            core_stem: "genesis_plus_gx".to_string(),
            platform_id: "Sega CD",
        };
        let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Sega CD".to_string(),
            game_key: "GM T-12345-00".to_string(),
        });
        let plan = build_retroarch_command_plan(
            &identity,
            &selected,
            &report(
                vec!["/usr/bin/retroarch"],
                vec![core_with_database(
                    "genesis_plus_gx",
                    Some("Sega - MS/GG/MD/CD"),
                    Some(
                        "Sega - Game Gear|Sega - Master System - Mark III|Sega - Mega-CD - Sega CD|Sega - Mega Drive - Genesis",
                    ),
                )],
            ),
        );
        let command = plan.command.expect("reviewed Sega CD core should plan");
        assert!(plan.blockers.is_empty());
        assert_eq!(command.selection.platform_id, "Sega CD");
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("-L"),
                OsString::from("/retroarch/cores/genesis_plus_gx_libretro.so"),
                OsString::from("/games/actual-title.cue"),
            ]
        );
    }

    #[test]
    fn valid_installed_retroarch_core_and_game_produce_ordered_argv() {
        let plan = build_retroarch_command_plan(
            &resolved(),
            &candidate(Some(PathBuf::from("/games/Final Fantasy VII.chd"))),
            &report(
                vec!["/usr/bin/retroarch"],
                vec![core("mednafen_psx", Some("PlayStation"))],
            ),
        );
        let command = plan
            .command
            .expect("a fully inspected native installation plans");
        assert!(plan.blockers.is_empty());
        assert_eq!(command.executable, PathBuf::from("/usr/bin/retroarch"));
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("-L"),
                OsString::from("/retroarch/cores/mednafen_psx_libretro.so"),
                OsString::from("/games/Final Fantasy VII.chd"),
            ]
        );
        assert_eq!(command.working_directory, None);
        assert_eq!(command.selection.profile, profile_ref());
    }

    #[test]
    fn missing_executable_blocks_without_a_command() {
        let plan = build_retroarch_command_plan(
            &resolved(),
            &candidate(Some(PathBuf::from("/games/game.chd"))),
            &report(Vec::new(), vec![core("mednafen_psx", Some("PlayStation"))]),
        );
        assert!(plan.command.is_none());
        assert!(has_blocker(
            &plan,
            LaunchBlockerKind::RetroArchExecutableMissing
        ));
    }

    #[test]
    fn missing_selected_core_blocks_without_a_command() {
        let plan = build_retroarch_command_plan(
            &resolved(),
            &candidate(Some(PathBuf::from("/games/game.chd"))),
            &report(vec!["/usr/bin/retroarch"], Vec::new()),
        );
        assert!(plan.command.is_none());
        assert!(has_blocker(&plan, LaunchBlockerKind::CoreMissing));
    }

    #[test]
    fn blocked_launch_candidate_is_not_reauthorized() {
        let mut candidate = candidate(Some(PathBuf::from("/games/game.chd")));
        candidate.readiness = LaunchReadiness::Blocked;
        candidate.blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::RequiredFirmwareMissing,
            "firmware is missing",
        ));
        let plan = build_retroarch_command_plan(
            &resolved(),
            &candidate,
            &report(
                vec!["/usr/bin/retroarch"],
                vec![core("mednafen_psx", Some("PlayStation"))],
            ),
        );
        assert!(plan.command.is_none());
        assert!(has_blocker(
            &plan,
            LaunchBlockerKind::RequiredFirmwareMissing
        ));
    }

    #[test]
    fn unresolved_and_mount_required_content_both_fail_closed() {
        let mut candidate = candidate(None);
        candidate.content.requires_mount = true;
        let plan = build_retroarch_command_plan(
            &resolved(),
            &candidate,
            &report(
                vec!["/usr/bin/retroarch"],
                vec![core("mednafen_psx", Some("PlayStation"))],
            ),
        );
        assert!(plan.command.is_none());
        assert!(has_blocker(&plan, LaunchBlockerKind::ContentNotResolved));
        assert!(
            plan.blockers
                .iter()
                .any(|blocker| blocker.detail.contains("mount"))
        );
    }

    #[test]
    fn stale_profile_or_core_metadata_mismatch_blocks() {
        let stale_profile = build_retroarch_command_plan(
            &resolved(),
            &candidate(Some(PathBuf::from("/games/game.chd"))),
            &RetroArchEnvironmentReport {
                format_version: 1,
                profiles: Vec::new(),
                diagnostics: Vec::new(),
            },
        );
        assert!(stale_profile.command.is_none());
        assert!(has_blocker(
            &stale_profile,
            LaunchBlockerKind::RetroArchProfileMissing
        ));

        let plan = build_retroarch_command_plan(
            &resolved(),
            &candidate(Some(PathBuf::from("/games/game.chd"))),
            &report(
                vec!["/usr/bin/retroarch"],
                vec![core("mednafen_psx", Some("Sega - Mega Drive - Genesis"))],
            ),
        );
        assert!(plan.command.is_none());
        assert!(has_blocker(&plan, LaunchBlockerKind::RetroArchCoreMismatch));
    }

    #[test]
    fn ambiguous_executable_or_core_blocks() {
        let ambiguous_executable = build_retroarch_command_plan(
            &resolved(),
            &candidate(Some(PathBuf::from("/games/game.chd"))),
            &report(
                vec!["/usr/bin/retroarch", "/opt/retroarch/retroarch"],
                vec![core("mednafen_psx", Some("PlayStation"))],
            ),
        );
        assert!(has_blocker(
            &ambiguous_executable,
            LaunchBlockerKind::AmbiguousRetroArchExecutable
        ));

        let ambiguous_core = build_retroarch_command_plan(
            &resolved(),
            &candidate(Some(PathBuf::from("/games/game.chd"))),
            &report(
                vec!["/usr/bin/retroarch"],
                vec![
                    core("mednafen_psx", Some("PlayStation")),
                    core("mednafen_psx", Some("PlayStation")),
                ],
            ),
        );
        assert!(has_blocker(
            &ambiguous_core,
            LaunchBlockerKind::AmbiguousCore
        ));
    }

    #[test]
    fn shell_looking_and_unicode_paths_remain_individual_arguments() {
        let game = PathBuf::from("/games/odd $name; \"quoted\" 日本語.chd");
        let mut environment = report(
            vec!["/opt/Retro Arch/retroarch; $safe"],
            vec![core("mednafen_psx", Some("PlayStation"))],
        );
        environment.profiles[0].cores[0].full_path = EncodedPath::from_path(&PathBuf::from(
            "/cores/odd core; $value \"日本語\"_libretro.so",
        ));
        let plan =
            build_retroarch_command_plan(&resolved(), &candidate(Some(game.clone())), &environment);
        let command = plan
            .command
            .expect("special characters are path data, not shell syntax");
        assert_eq!(
            command.executable,
            PathBuf::from("/opt/Retro Arch/retroarch; $safe")
        );
        assert_eq!(command.arguments.len(), 3);
        assert_eq!(command.arguments[0], OsString::from("-L"));
        assert_eq!(
            command.arguments[1],
            OsString::from("/cores/odd core; $value \"日本語\"_libretro.so")
        );
        assert_eq!(command.arguments[2], game.into_os_string());
    }

    #[test]
    fn non_retroarch_or_unresolved_identity_is_blocked() {
        let mut non_retroarch = candidate(Some(PathBuf::from("/games/game.chd")));
        non_retroarch.target = LaunchTarget::Standalone {
            adapter_id: "duckstation",
            profile_id: "native".to_string(),
            profile_path: None,
        };
        let wrong_target = build_retroarch_command_plan(
            &resolved(),
            &non_retroarch,
            &report(
                vec!["/usr/bin/retroarch"],
                vec![core("mednafen_psx", Some("PlayStation"))],
            ),
        );
        assert!(has_blocker(
            &wrong_target,
            LaunchBlockerKind::RetroArchCandidateRequired
        ));

        let unresolved = build_retroarch_command_plan(
            &CanonicalIdentityStatus::Unknown,
            &candidate(Some(PathBuf::from("/games/game.chd"))),
            &report(
                vec!["/usr/bin/retroarch"],
                vec![core("mednafen_psx", Some("PlayStation"))],
            ),
        );
        assert!(has_blocker(
            &unresolved,
            LaunchBlockerKind::IdentityUnresolved
        ));

        let conflicting = build_retroarch_command_plan(
            &CanonicalIdentityStatus::Conflicting,
            &candidate(Some(PathBuf::from("/games/game.chd"))),
            &report(
                vec!["/usr/bin/retroarch"],
                vec![core("mednafen_psx", Some("PlayStation"))],
            ),
        );
        assert!(has_blocker(
            &conflicting,
            LaunchBlockerKind::IdentityConflict
        ));
    }
}
