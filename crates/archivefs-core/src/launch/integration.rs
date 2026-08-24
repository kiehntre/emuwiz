//! Read-only wiring from already-gathered EmuWiz results to launch planning.
//!
//! [`build_launch_plan_from_results`] is the one integration seam between
//! existing canonical identity, resolved content, local emulator inspection,
//! and [`crate::launch::planning::build_launch_plan`]. It performs no
//! inspection itself: callers hand it profile discoveries and any game
//! inspection they have already completed. It neither mounts content nor
//! reads configuration, and it never builds a command or starts an emulator.
//!
//! Identity remains authoritative upstream. The `identity` and
//! `verified_identity_facts` fields deliberately carry only the result the
//! identity layer has already resolved; this module never reads a filename,
//! extension, emulator metadata, or RetroArch core metadata to decide what a
//! game is.

use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::launch::input_projection::{
    LaunchInputProjection, VerifiedIdentityFact, project_duckstation_launch_input,
    project_pcsx2_launch_input, project_ppsspp_launch_input, project_xenia_launch_input,
};
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContentRef, LaunchPlan, RememberedPreference,
    StandaloneProfileInput, build_launch_plan,
};
use crate::launch::readiness::{
    duckstation_firmware_readiness, pcsx2_firmware_readiness, ppsspp_firmware_readiness,
};
use crate::patch_manager::{
    DuckStationBiosState, DuckStationGameInspection, DuckStationProfile, Pcsx2BiosVerification,
    Pcsx2GameInspection, Pcsx2Profile, PpssppProfile, XeniaProfile,
};

/// One profile from an existing adapter discovery, together with only the
/// already-inspected readiness state launch planning needs.
///
/// The constructors are intentionally shallow projections: they borrow the
/// adapter's own profile and game inspection rather than re-inspecting a
/// configuration directory. PPSSPP has no BIOS requirement, so no game
/// inspection is needed for its firmware readiness.
#[derive(Debug, Clone, Copy)]
pub enum DiscoveredStandaloneProfile<'a> {
    DuckStation {
        profile: &'a DuckStationProfile,
        bios: DuckStationBiosState,
    },
    Pcsx2 {
        profile: &'a Pcsx2Profile,
        bios: Pcsx2BiosVerification,
    },
    Ppsspp {
        profile: &'a PpssppProfile,
    },
    /// Kept in the input so a caller does not silently discard a discovered
    /// Xenia profile. It cannot become a launch candidate until Xenia has a
    /// real per-game request type; [`project_xenia_launch_input`] stays
    /// explicitly unavailable rather than fabricating one here.
    Xenia {
        profile: &'a XeniaProfile,
    },
}

impl<'a> DiscoveredStandaloneProfile<'a> {
    pub fn duckstation(
        profile: &'a DuckStationProfile,
        inspection: &DuckStationGameInspection,
    ) -> Self {
        Self::DuckStation {
            profile,
            bios: inspection.health.bios,
        }
    }

    pub fn pcsx2(profile: &'a Pcsx2Profile, inspection: &Pcsx2GameInspection) -> Self {
        Self::Pcsx2 {
            profile,
            bios: inspection.health.bios,
        }
    }

    pub fn ppsspp(profile: &'a PpssppProfile) -> Self {
        Self::Ppsspp { profile }
    }

    pub fn xenia(profile: &'a XeniaProfile) -> Self {
        Self::Xenia { profile }
    }
}

/// All already-gathered inputs needed to produce one real [`LaunchPlan`].
///
/// `content` is the existing resolved-content result. A container that still
/// needs mounting must keep `resolved_path` empty and `requires_mount` true;
/// this integration layer never changes either value.
#[derive(Debug, Clone, Copy)]
pub struct LaunchPlanResults<'a> {
    pub identity: &'a CanonicalIdentityStatus,
    pub verified_identity_facts: &'a [VerifiedIdentityFact],
    pub content: &'a LaunchContentRef,
    pub standalone_profiles: &'a [DiscoveredStandaloneProfile<'a>],
    pub retroarch: &'a RetroArchEnvironmentReport,
    pub remembered: &'a [RememberedPreference],
}

fn authorized<T>(projection: LaunchInputProjection<T>) -> bool {
    matches!(projection, LaunchInputProjection::Authorized(_))
}

fn project_standalone_profiles(input: &LaunchPlanResults<'_>) -> Vec<StandaloneProfileInput> {
    input
        .standalone_profiles
        .iter()
        .filter_map(|source| match source {
            DiscoveredStandaloneProfile::DuckStation { profile, bios }
                if authorized(project_duckstation_launch_input(
                    input.verified_identity_facts,
                )) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "duckstation",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: duckstation_firmware_readiness(*bios),
                })
            }
            DiscoveredStandaloneProfile::Pcsx2 { profile, bios }
                if authorized(project_pcsx2_launch_input(input.verified_identity_facts)) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "pcsx2",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: pcsx2_firmware_readiness(*bios),
                })
            }
            DiscoveredStandaloneProfile::Ppsspp { profile }
                if authorized(project_ppsspp_launch_input(input.verified_identity_facts)) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "ppsspp",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: ppsspp_firmware_readiness(),
                })
            }
            DiscoveredStandaloneProfile::Xenia { profile } => {
                let _ = profile;
                let LaunchInputProjection::Unavailable { detail } =
                    project_xenia_launch_input(input.verified_identity_facts)
                else {
                    unreachable!("Xenia has no launch-input request type in this build")
                };
                debug_assert_eq!(
                    detail,
                    "no Xenia launch-input request type exists in this build"
                );
                None
            }
            _ => None,
        })
        .collect()
}

/// Builds a launch plan from real, already-gathered EmuWiz results.
///
/// Pure and read-only: this only projects the supplied values and delegates
/// to [`build_launch_plan`]. In particular, unavailable or mismatched
/// adapter input projections do not become candidates, unresolved identity
/// remains fail-closed in the planner, and content that requires a mount is
/// passed through unchanged so the planner blocks it.
pub fn build_launch_plan_from_results(input: &LaunchPlanResults<'_>) -> LaunchPlan {
    let standalone_profiles = project_standalone_profiles(input);
    build_launch_plan(
        input.identity,
        input.content,
        &standalone_profiles,
        input.retroarch,
        input.remembered,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::emulator_environment::retroarch::{
        ConfigFileFinding, ConfigReadOutcome, CoreFinding, CoreInfoFinding, DirectoryProbeFinding,
        Evidence, ProfileKind, ProfileScope, RetroArchPlaylistInventory, RetroArchProfile,
    };
    use crate::emulator_environment::{EncodedPath, FsProbe};
    use crate::launch::planning::{
        LaunchContainerKind, LaunchContentKind, LaunchTarget, ResolvedIdentity,
    };
    use crate::launch::readiness::{LaunchBlockerKind, LaunchReadiness};
    use crate::patch_manager::{
        DuckStationInstallationType, Pcsx2InstallationType, Pcsx2ProfileScope,
        PpssppInstallationType, PpssppProfileScope, XeniaInstallationType, XeniaProfileScope,
    };

    fn resolved(platform_id: &str, game_key: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform_id.to_string(),
            game_key: game_key.to_string(),
        })
    }

    fn resolved_content() -> LaunchContentRef {
        LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::Chd),
            resolved_path: Some(PathBuf::from("/library/game.chd")),
            requires_mount: false,
            provenance: "existing resolved game content".to_string(),
        }
    }

    fn needs_mount_content() -> LaunchContentRef {
        LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::Archive),
            resolved_path: None,
            requires_mount: true,
            provenance: "existing archive mount state is pending".to_string(),
        }
    }

    fn empty_retroarch() -> RetroArchEnvironmentReport {
        RetroArchEnvironmentReport {
            format_version: 1,
            profiles: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn retroarch_with_psx_core() -> RetroArchEnvironmentReport {
        let config_dir = EncodedPath::from_path(&PathBuf::from("/retroarch"));
        RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![RetroArchProfile {
                profile_kind: ProfileKind::Native,
                scope: ProfileScope::User,
                evidence: Evidence {
                    executables: Vec::new(),
                    flatpak_metadata_found: false,
                    config_directory_found: true,
                    config_file_found: true,
                },
                config_directory: DirectoryProbeFinding {
                    path: config_dir.clone(),
                    probe: FsProbe::PresentDirectory,
                },
                config_file: ConfigFileFinding {
                    path: EncodedPath::from_path(&PathBuf::from("/retroarch/retroarch.cfg")),
                    probe: FsProbe::PresentFile,
                    read: ConfigReadOutcome::NotAttempted,
                },
                paths: Vec::new(),
                cores: vec![CoreFinding {
                    file_name: EncodedPath::from_path(&PathBuf::from("mednafen_psx_libretro.so")),
                    full_path: EncodedPath::from_path(&PathBuf::from(
                        "/retroarch/cores/mednafen_psx_libretro.so",
                    )),
                    core_stem: "mednafen_psx".to_string(),
                    info: CoreInfoFinding::Found {
                        display_name: None,
                        display_version: None,
                        system_name: Some("PlayStation".to_string()),
                        supported_extensions: Vec::new(),
                        core_name: Some("mednafen_psx".to_string()),
                        manufacturer: None,
                        categories: None,
                        database: None,
                        firmware: Vec::new(),
                    },
                }],
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

    fn duckstation_profile() -> DuckStationProfile {
        let root = PathBuf::from("/profiles/duckstation");
        DuckStationProfile {
            profile_id: "duckstation-native".to_string(),
            installation_type: DuckStationInstallationType::Native,
            configuration_path: root.clone(),
            eligible: true,
            blocker: None,
            executable_candidates: Vec::new(),
            global_config_path: root.join("settings.ini"),
            game_settings_path: root.join("gamesettings"),
            cheats_path: root.join("cheats"),
            patches_path: root.join("patches"),
            textures_path: root.join("textures"),
            bios_path: root.join("bios"),
            memory_cards_path: root.join("memcards"),
            save_states_path: root.join("savestates"),
        }
    }

    fn pcsx2_profile() -> Pcsx2Profile {
        let root = PathBuf::from("/profiles/pcsx2");
        Pcsx2Profile {
            profile_id: "pcsx2-native".to_string(),
            installation_type: Pcsx2InstallationType::Native,
            scope: Pcsx2ProfileScope::User,
            configuration_path: root,
            provenance: "test profile",
            eligible: true,
            blockers: Vec::new(),
            patch_directories: Vec::new(),
            configuration_identity: None,
            executable_candidates: Vec::new(),
        }
    }

    fn ppsspp_profile() -> PpssppProfile {
        let root = PathBuf::from("/profiles/ppsspp");
        PpssppProfile {
            profile_id: "ppsspp-native".to_string(),
            installation_type: PpssppInstallationType::Native,
            scope: PpssppProfileScope::User,
            configuration_path: root.clone(),
            provenance: "test profile",
            eligible: true,
            blockers: Vec::new(),
            executable_candidates: Vec::new(),
            memstick_path: root.join("PSP"),
            system_path: root.join("PSP/SYSTEM"),
            global_config_path: root.join("PSP/SYSTEM/ppsspp.ini"),
            cheats_path: root.join("PSP/Cheats"),
            textures_path: root.join("PSP/Textures"),
            savedata_path: root.join("PSP/SAVEDATA"),
            game_path: root.join("PSP/GAME"),
            state_path: root.join("PSP/STATE"),
        }
    }

    fn xenia_profile() -> XeniaProfile {
        let root = PathBuf::from("/profiles/xenia");
        XeniaProfile {
            profile_id: "xenia-explicit".to_string(),
            installation_type: XeniaInstallationType::Explicit,
            scope: XeniaProfileScope::Explicit,
            configuration_path: root.clone(),
            provenance: "test profile",
            eligible: true,
            blockers: Vec::new(),
            patches_path: root.join("patches"),
            patches_state: crate::patch_manager::XeniaPatchesDirectoryState::Available,
            patches_warning: None,
            configuration_identity: None,
        }
    }

    fn plan(
        identity: &CanonicalIdentityStatus,
        facts: &[VerifiedIdentityFact],
        content: &LaunchContentRef,
        profiles: &[DiscoveredStandaloneProfile<'_>],
        retroarch: &RetroArchEnvironmentReport,
    ) -> LaunchPlan {
        build_launch_plan_from_results(&LaunchPlanResults {
            identity,
            verified_identity_facts: facts,
            content,
            standalone_profiles: profiles,
            retroarch,
            remembered: &[],
        })
    }

    #[test]
    fn ps1_duckstation_uses_the_existing_bios_projection() {
        let identity = resolved("PSX", "SLUS-12345");
        let profile = duckstation_profile();
        let profiles = [DiscoveredStandaloneProfile::DuckStation {
            profile: &profile,
            bios: DuckStationBiosState::PresentUnverified,
        }];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::Ps1Serial("SLUS-12345".to_string())],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.summary.ready_with_warnings, 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "duckstation",
                ..
            }
        ));
    }

    #[test]
    fn ps1_duckstation_verified_firmware_reaches_strict_ready() {
        let identity = resolved("PSX", "SLUS-12345");
        let profile = duckstation_profile();
        let profiles = [DiscoveredStandaloneProfile::DuckStation {
            profile: &profile,
            bios: DuckStationBiosState::Verified,
        }];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::Ps1Serial("SLUS-12345".to_string())],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.summary.ready, 1);
        assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
    }

    #[test]
    fn ps1_duckstation_missing_firmware_is_blocked() {
        let identity = resolved("PSX", "SLUS-12345");
        let profile = duckstation_profile();
        let profiles = [DiscoveredStandaloneProfile::DuckStation {
            profile: &profile,
            bios: DuckStationBiosState::Missing,
        }];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::Ps1Serial("SLUS-12345".to_string())],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.summary.blocked, 1);
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::RequiredFirmwareMissing)
        );
    }

    #[test]
    fn ps2_pcsx2_missing_firmware_is_blocked() {
        let identity = resolved("PS2", "SLUS-98765");
        let profile = pcsx2_profile();
        let profiles = [DiscoveredStandaloneProfile::Pcsx2 {
            profile: &profile,
            bios: Pcsx2BiosVerification::Missing,
        }];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::Ps2Serial("SLUS-98765".to_string())],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.summary.blocked, 1);
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::RequiredFirmwareMissing)
        );
    }

    #[test]
    fn ps2_pcsx2_unverified_firmware_is_a_warning() {
        let identity = resolved("PS2", "SLUS-98765");
        let profile = pcsx2_profile();
        let profiles = [DiscoveredStandaloneProfile::Pcsx2 {
            profile: &profile,
            bios: Pcsx2BiosVerification::PresentUnverified,
        }];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::Ps2Serial("SLUS-98765".to_string())],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(
            plan.candidates[0].readiness,
            LaunchReadiness::ReadyWithWarnings
        );
    }

    #[test]
    fn psp_ppsspp_is_ready_without_firmware() {
        let identity = resolved("PSP", "ULUS-10000");
        let profile = ppsspp_profile();
        let profiles = [DiscoveredStandaloneProfile::ppsspp(&profile)];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::PspDiscId("ULUS-10000".to_string())],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.summary.ready, 1);
        assert_eq!(
            plan.candidates[0].firmware,
            crate::launch::FirmwareReadiness::NotRequired
        );
    }

    #[test]
    fn installed_retroarch_core_becomes_a_candidate() {
        let identity = resolved("PSX", "SLUS-12345");
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::Ps1Serial("SLUS-12345".to_string())],
            &resolved_content(),
            &[],
            &retroarch_with_psx_core(),
        );
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::RetroArchCore { ref core_stem, .. } if core_stem == "mednafen_psx"
        ));
    }

    #[test]
    fn unknown_identity_produces_no_candidates() {
        let profile = ppsspp_profile();
        let profiles = [DiscoveredStandaloneProfile::ppsspp(&profile)];
        let plan = plan(
            &CanonicalIdentityStatus::Unknown,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert!(plan.candidates.is_empty());
    }

    #[test]
    fn content_requiring_a_mount_remains_blocked() {
        let identity = resolved("PSP", "ULUS-10000");
        let profile = ppsspp_profile();
        let profiles = [DiscoveredStandaloneProfile::ppsspp(&profile)];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::PspDiscId("ULUS-10000".to_string())],
            &needs_mount_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::ContentNotResolved)
        );
    }

    #[test]
    fn xenia_request_unavailability_stays_explicit() {
        let identity = resolved("Xbox360", "4D5307E6");
        let profile = xenia_profile();
        let profiles = [DiscoveredStandaloneProfile::xenia(&profile)];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::XboxTitleId("4D5307E6".to_string())],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert!(matches!(
            project_xenia_launch_input(&[VerifiedIdentityFact::XboxTitleId(
                "4D5307E6".to_string()
            )]),
            LaunchInputProjection::Unavailable {
                detail: "no Xenia launch-input request type exists in this build"
            }
        ));
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::NoInstallationCandidate)
        );
    }
}
