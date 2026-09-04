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

use std::path::Path;

use crate::amiga_cd_evidence::{
    AMIGA_CD32_PLATFORM_ID, AMIGA_CDTV_PLATFORM_ID, AmigaCdMachineReadiness,
};
use crate::dat::model::DatEcosystem;
use crate::dat::set::SetResolution;
use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::launch::fbneo_command::{FbneoIdentityEvidence, FbneoSetEvidence};
use crate::launch::input_projection::{
    LaunchInputProjection, VerifiedIdentityFact, project_amiga_whdload_launch_input,
    project_duckstation_launch_input, project_flycast_launch_input, project_hatari_launch_input,
    project_melonds_launch_input, project_pcsx2_launch_input, project_ppsspp_launch_input,
    project_rpcs3_launch_input, project_xemu_launch_input, project_xenia_launch_input,
};
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContentRef, LaunchPlan, RememberedPreference,
    StandaloneProfileInput, build_launch_plan,
};
use crate::launch::readiness::{
    FirmwareReadiness, duckstation_firmware_readiness, flycast_firmware_readiness,
    hatari_firmware_readiness, pcsx2_firmware_readiness, ppsspp_firmware_readiness,
    rpcs3_firmware_readiness,
};
use crate::patch_manager::{
    AmigaEmulatorKind, AmigaGameInspection, AmigaKickstartState, AmigaProfile, CemuProfile,
    DuckStationBiosState, DuckStationGameInspection, DuckStationProfile, FlycastGameInspection,
    FlycastProfile, FlycastSystemFileState, HatariGameInspection, HatariProfile,
    MelonDsFirmwareState, MelonDsProfile, Pcsx2BiosVerification, Pcsx2GameInspection, Pcsx2Profile,
    PpssppProfile, Rpcs3GameInspection, Rpcs3Profile, XemuProfile, XeniaProfile,
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
    /// Xenia has no per-game request/inspection type and no firmware/BIOS
    /// concept in this build (see [`project_xenia_launch_input`]'s own doc
    /// comment) - the verified XEX title/media ID a caller already obtained
    /// directly from [`crate::game_identity::GameIdentityReport`] is carried
    /// here instead of through `verified_identity_facts`, since no
    /// `VerifiedIdentityFact` variant names Xbox 360 at all.
    Xenia {
        profile: &'a XeniaProfile,
        verified_xex_title_id: Option<&'a str>,
        verified_xex_media_id: Option<&'a str>,
    },
    Flycast {
        profile: &'a FlycastProfile,
        bios: FlycastSystemFileState,
    },
    MelonDs {
        profile: &'a MelonDsProfile,
    },
    Mgba {
        profile: &'a crate::patch_manager::MgbaProfile,
    },
    Vita3k {
        profile: &'a crate::patch_manager::Vita3kProfile,
    },
    FsUae {
        profile: &'a AmigaProfile,
        inspection: &'a AmigaGameInspection,
    },
    Hatari {
        profile: &'a HatariProfile,
        inspection: &'a HatariGameInspection,
    },
    Rpcs3 {
        profile: &'a Rpcs3Profile,
        inspection: &'a Rpcs3GameInspection,
    },
    /// xemu's own four-way system-file health (MCPX/flash BIOS/EEPROM/HDD)
    /// does not reduce to one shared [`FirmwareReadiness`] value, so it is
    /// deliberately not carried here at all - it is checked directly, from a
    /// freshly re-inspected [`crate::patch_manager::XemuHealth`], inside
    /// [`crate::launch::xemu_command::build_xemu_command_plan`]. Projecting
    /// [`FirmwareReadiness::NotRequired`] below only tells the *generic*
    /// planner not to raise its own, single-value firmware blocker for xemu
    /// candidates - it never claims xemu itself needs no firmware.
    Xemu {
        profile: &'a XemuProfile,
    },
    /// A discovered Cemu profile - see [`crate::patch_manager::cemu_local`].
    /// Cemu's own MLC/keys/layout evidence does not reduce to one shared
    /// [`FirmwareReadiness`] value any more than xemu's does (see the
    /// [`Self::Xemu`] doc comment above for the same reasoning): this only
    /// tells the generic planner not to raise its own firmware blocker,
    /// while [`crate::launch::cemu_command::build_cemu_command_plan`] is
    /// still where the real MLC/keys/layout check happens.
    Cemu {
        profile: &'a CemuProfile,
    },
    /// A discovered Amiberry profile. Amiberry and FS-UAE are distinct
    /// adapters sharing the same underlying [`AmigaProfile`]/
    /// [`AmigaGameInspection`] discovery evidence
    /// (see [`crate::patch_manager::amiga_whdload_local`]'s
    /// [`AmigaEmulatorKind`]) - never merged into one candidate. Kept as its
    /// own variant, not folded into [`Self::FsUae`], so a caller can never
    /// accidentally hand an Amiberry profile to code that expects FS-UAE's
    /// own launch handoff or vice versa.
    Amiberry {
        profile: &'a AmigaProfile,
        inspection: &'a AmigaGameInspection,
    },
    AmiberryCd {
        profile: &'a AmigaProfile,
        readiness: &'a AmigaCdMachineReadiness,
    },
    /// A discovered MAME executable, together with the trusted MAME/DAT set
    /// resolutions a caller already computed. `None`/empty means "no trusted
    /// MAME identity for this content" - never inferred from the platform
    /// alone (see [`crate::launch::mame_command`]).
    Mame {
        executable: Option<&'a Path>,
        set_resolutions: &'a [SetResolution],
    },
    /// A discovered FBNeo executable, together with the trusted FBNeo-
    /// specific set evidence a caller already computed. A MAME-only
    /// identity (see [`FbneoIdentityEvidence::MameOnly`]) is deliberately
    /// not enough - only [`FbneoIdentityEvidence::VerifiedDat`] against the
    /// FBNeo ecosystem authorizes a candidate (see
    /// [`crate::launch::fbneo_command`]).
    Fbneo {
        executable: Option<&'a Path>,
        set: Option<&'a FbneoSetEvidence>,
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

    pub fn xenia(
        profile: &'a XeniaProfile,
        verified_xex_title_id: Option<&'a str>,
        verified_xex_media_id: Option<&'a str>,
    ) -> Self {
        Self::Xenia {
            profile,
            verified_xex_title_id,
            verified_xex_media_id,
        }
    }

    pub fn flycast(profile: &'a FlycastProfile, inspection: &FlycastGameInspection) -> Self {
        Self::Flycast {
            profile,
            bios: inspection.health.system.dreamcast_bios,
        }
    }

    pub fn melonds(profile: &'a MelonDsProfile) -> Self {
        Self::MelonDs { profile }
    }

    pub fn mgba(profile: &'a crate::patch_manager::MgbaProfile) -> Self {
        Self::Mgba { profile }
    }

    pub fn vita3k(profile: &'a crate::patch_manager::Vita3kProfile) -> Self {
        Self::Vita3k { profile }
    }

    pub fn fsuae(profile: &'a AmigaProfile, inspection: &'a AmigaGameInspection) -> Self {
        Self::FsUae {
            profile,
            inspection,
        }
    }

    pub fn hatari(profile: &'a HatariProfile, inspection: &'a HatariGameInspection) -> Self {
        Self::Hatari {
            profile,
            inspection,
        }
    }

    pub fn rpcs3(profile: &'a Rpcs3Profile, inspection: &'a Rpcs3GameInspection) -> Self {
        Self::Rpcs3 {
            profile,
            inspection,
        }
    }

    pub fn xemu(profile: &'a XemuProfile) -> Self {
        Self::Xemu { profile }
    }

    pub fn cemu(profile: &'a CemuProfile) -> Self {
        Self::Cemu { profile }
    }

    pub fn amiberry(profile: &'a AmigaProfile, inspection: &'a AmigaGameInspection) -> Self {
        Self::Amiberry {
            profile,
            inspection,
        }
    }

    pub fn amiberry_cd(profile: &'a AmigaProfile, readiness: &'a AmigaCdMachineReadiness) -> Self {
        Self::AmiberryCd { profile, readiness }
    }

    pub fn mame(executable: Option<&'a Path>, set_resolutions: &'a [SetResolution]) -> Self {
        Self::Mame {
            executable,
            set_resolutions,
        }
    }

    pub fn fbneo(executable: Option<&'a Path>, set: Option<&'a FbneoSetEvidence>) -> Self {
        Self::Fbneo { executable, set }
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
            DiscoveredStandaloneProfile::Flycast { profile, bios }
                if authorized(project_flycast_launch_input(input.verified_identity_facts)) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "flycast",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: flycast_firmware_readiness(*bios),
                })
            }
            DiscoveredStandaloneProfile::MelonDs { profile }
                if authorized(project_melonds_launch_input(input.verified_identity_facts)) =>
            {
                let firmware = match profile.firmware.mode {
                    crate::patch_manager::MelonDsFirmwareMode::DirectBoot => {
                        FirmwareReadiness::NotRequired
                    }
                    crate::patch_manager::MelonDsFirmwareMode::ExternalFirmwareBoot => {
                        if [
                            profile.firmware.bios7,
                            profile.firmware.bios9,
                            profile.firmware.firmware,
                        ]
                        .contains(&MelonDsFirmwareState::Missing)
                        {
                            FirmwareReadiness::Missing
                        } else {
                            FirmwareReadiness::PresentUnverified
                        }
                    }
                    crate::patch_manager::MelonDsFirmwareMode::Unknown => {
                        FirmwareReadiness::Unknown
                    }
                };
                Some(StandaloneProfileInput {
                    adapter_id: "melonds",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware,
                })
            }
            DiscoveredStandaloneProfile::Mgba { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if matches!(identity.platform_id.as_str(), "Game Boy" | "Game Boy Color" | "Game Boy Advance")) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "mgba",
                    profile_id: profile.profile_id.clone(),
                    profile_path: profile.config_path.clone(),
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::Vita3k { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "PlayStation Vita") =>
            {
                let firmware = match profile.firmware {
                    crate::patch_manager::Vita3kFirmwareState::PresentUnverified => {
                        FirmwareReadiness::PresentUnverified
                    }
                    crate::patch_manager::Vita3kFirmwareState::Missing => {
                        FirmwareReadiness::Missing
                    }
                    crate::patch_manager::Vita3kFirmwareState::Unknown => {
                        FirmwareReadiness::Unknown
                    }
                };
                Some(StandaloneProfileInput {
                    adapter_id: "vita3k",
                    profile_id: profile.profile_id.clone(),
                    profile_path: profile.config_path.clone(),
                    eligible: profile.eligible,
                    firmware,
                })
            }
            DiscoveredStandaloneProfile::FsUae {
                profile,
                inspection,
            } if profile.emulator == crate::patch_manager::AmigaEmulatorKind::FsUae
                && authorized(project_amiga_whdload_launch_input(
                    input.verified_identity_facts,
                )) =>
            {
                let firmware = match inspection.health.kickstart.state {
                    AmigaKickstartState::PresentUnverified => FirmwareReadiness::PresentUnverified,
                    AmigaKickstartState::Missing | AmigaKickstartState::NotConfigured => {
                        FirmwareReadiness::Missing
                    }
                    AmigaKickstartState::Unreadable | AmigaKickstartState::Unknown => {
                        FirmwareReadiness::Unknown
                    }
                };
                Some(StandaloneProfileInput {
                    adapter_id: "fsuae",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_root.clone()),
                    eligible: profile.eligible,
                    firmware,
                })
            }
            DiscoveredStandaloneProfile::Hatari {
                profile,
                inspection,
            } if authorized(project_hatari_launch_input(input.verified_identity_facts)) => {
                Some(StandaloneProfileInput {
                    adapter_id: "hatari",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.config_path.clone()),
                    eligible: profile.eligible,
                    firmware: hatari_firmware_readiness(inspection.health.tos.health),
                })
            }
            DiscoveredStandaloneProfile::Rpcs3 {
                profile,
                inspection,
            } if authorized(project_rpcs3_launch_input(input.verified_identity_facts)) => {
                Some(StandaloneProfileInput {
                    adapter_id: "rpcs3",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: rpcs3_firmware_readiness(&inspection.health.firmware),
                })
            }
            DiscoveredStandaloneProfile::Xemu { profile }
                if authorized(project_xemu_launch_input(input.verified_identity_facts)) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "xemu",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::Xenia {
                profile,
                verified_xex_title_id,
                verified_xex_media_id,
            } if authorized(project_xenia_launch_input(
                *verified_xex_title_id,
                *verified_xex_media_id,
            )) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "xenia",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::Cemu { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "WiiU") =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "cemu",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::Amiberry {
                profile,
                inspection,
            } if profile.emulator == AmigaEmulatorKind::Amiberry
                && authorized(project_amiga_whdload_launch_input(
                    input.verified_identity_facts,
                )) =>
            {
                let firmware = match inspection.health.kickstart.state {
                    AmigaKickstartState::PresentUnverified => FirmwareReadiness::PresentUnverified,
                    AmigaKickstartState::Missing | AmigaKickstartState::NotConfigured => {
                        FirmwareReadiness::Missing
                    }
                    AmigaKickstartState::Unreadable | AmigaKickstartState::Unknown => {
                        FirmwareReadiness::Unknown
                    }
                };
                Some(StandaloneProfileInput {
                    adapter_id: "amiberry",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_root.clone()),
                    eligible: profile.eligible,
                    firmware,
                })
            }
            DiscoveredStandaloneProfile::AmiberryCd { profile, readiness }
                if profile.emulator == AmigaEmulatorKind::Amiberry
                    && readiness.readiness != crate::launch::readiness::LaunchReadiness::Blocked
                    && matches!(
                        input.identity,
                        CanonicalIdentityStatus::Resolved(identity)
                            if (identity.platform_id == AMIGA_CD32_PLATFORM_ID
                                && matches!(readiness.machine, crate::amiga_cd_evidence::AmigaCdMachine::Cd32))
                                || (identity.platform_id == AMIGA_CDTV_PLATFORM_ID
                                    && matches!(readiness.machine, crate::amiga_cd_evidence::AmigaCdMachine::Cdtv))
                    ) =>
            {
                let firmware = if readiness.readiness == crate::launch::readiness::LaunchReadiness::Ready {
                    FirmwareReadiness::Verified
                } else {
                    FirmwareReadiness::PresentUnverified
                };
                Some(StandaloneProfileInput {
                    adapter_id: "amiberry",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_root.clone()),
                    eligible: profile.eligible,
                    firmware,
                })
            }
            DiscoveredStandaloneProfile::Mame {
                executable: Some(executable),
                set_resolutions,
            } if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "Arcade")
                && !set_resolutions.is_empty() =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "mame",
                    profile_id: format!("mame:{}", executable.display()),
                    profile_path: Some(executable.to_path_buf()),
                    eligible: true,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::Fbneo {
                executable: Some(executable),
                set,
            } if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "Arcade")
                && set.is_some_and(|set| {
                    matches!(
                        set.identity_evidence,
                        FbneoIdentityEvidence::VerifiedDat {
                            ecosystem: DatEcosystem::FBNeo,
                            ..
                        }
                    )
                }) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "fbneo",
                    profile_id: format!("fbneo:{}", executable.display()),
                    profile_path: Some(executable.to_path_buf()),
                    eligible: true,
                    firmware: FirmwareReadiness::NotRequired,
                })
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
            provenance: "XDG_CONFIG_HOME DuckStation directory",
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

    fn flycast_profile() -> FlycastProfile {
        let root = PathBuf::from("/profiles/flycast");
        FlycastProfile {
            profile_id: "flycast-native".to_string(),
            installation_type: crate::patch_manager::FlycastInstallationType::Native,
            configuration_path: root.join("config"),
            data_path: root.join("data"),
            eligible: true,
            blocker: None,
            executable_candidates: Vec::new(),
            config_path: root.join("config/emu.cfg"),
            system_path: root.join("data/data"),
            game_settings_path: root.join("data/mappings"),
            cheats_path: root.join("data/cheats"),
            textures_path: root.join("data/tex"),
            vmu_path: root.join("data/vmu"),
            save_states_path: root.join("data/states"),
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

    fn vita3k_profile() -> crate::patch_manager::Vita3kProfile {
        let root = PathBuf::from("/profiles/vita3k");
        crate::patch_manager::Vita3kProfile {
            profile_id: "vita3k-native".to_string(),
            installation_type: crate::patch_manager::Vita3kInstallationType::Native,
            configuration_path: root.clone(),
            config_path: Some(root.join("config.yml")),
            vita_fs_path: root.join("ux0"),
            firmware: crate::patch_manager::Vita3kFirmwareState::PresentUnverified,
            eligible: true,
            blocker: None,
            executable_candidates: Vec::new(),
            config: None,
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

    fn cemu_profile(eligible: bool) -> CemuProfile {
        let root = PathBuf::from("/profiles/cemu");
        CemuProfile {
            profile_id: "cemu:/profiles/cemu".to_string(),
            installation_type: crate::patch_manager::CemuInstallationType::Native,
            configuration_path: root.clone(),
            config_path: Some(root.join("settings.xml")),
            eligible,
            blocker: (!eligible).then(|| "no safe Cemu executable was discovered".to_string()),
            executable_candidates: Vec::new(),
            config: None,
            keys: crate::patch_manager::CemuKeysEvidence {
                path: None,
                state: crate::patch_manager::CemuKeysState::NotConfigured,
            },
        }
    }

    fn amiga_profile(emulator: AmigaEmulatorKind, eligible: bool) -> AmigaProfile {
        let root = PathBuf::from("/profiles/amiga");
        AmigaProfile {
            profile_id: format!("{emulator:?}:/profiles/amiga"),
            emulator,
            installation_type: crate::patch_manager::AmigaInstallationType::Native,
            scope: crate::patch_manager::AmigaProfileScope::User,
            configuration_root: root.clone(),
            global_config_path: None,
            profile_paths: Vec::new(),
            executable_candidates: Vec::new(),
            eligible,
            warnings: Vec::new(),
        }
    }

    /// Runs the real, existing WHDLoad inspection over a fixture profile,
    /// rather than hand-constructing `AmigaGameInspection`'s many nested
    /// fields - the same fixture pattern
    /// `patch_manager::amiga_whdload_local`'s own tests already use.
    fn amiga_inspection(profile: &AmigaProfile) -> AmigaGameInspection {
        crate::patch_manager::inspect_amiga_whdload_game(
            profile,
            &crate::patch_manager::AmigaGameRequest::default(),
        )
    }

    fn mame_set_resolution(state: crate::dat::set::SetState) -> SetResolution {
        SetResolution {
            identity: crate::dat::set::SetIdentity {
                source_id: "mame".to_string(),
                game_name: "pacman".to_string(),
            },
            archive_path: PathBuf::from("/library/pacman.zip"),
            state,
            members_required: Vec::new(),
            members_verified: Vec::new(),
            members_bad: Vec::new(),
            members_optional: Vec::new(),
            members_borrowed: Vec::new(),
            disks_required: Vec::new(),
            disks_verified: Vec::new(),
            disks_parent_required: Vec::new(),
            dependencies: crate::dat::dependency::SetDependencyReport {
                state: crate::dat::dependency::DependencyState::NotApplicable,
                requirements: Vec::new(),
            },
        }
    }

    fn fbneo_set_evidence(ecosystem: DatEcosystem) -> FbneoSetEvidence {
        FbneoSetEvidence {
            driver_name: "mslug".to_string(),
            resolution: SetResolution {
                identity: crate::dat::set::SetIdentity {
                    source_id: "fbneo".to_string(),
                    game_name: "mslug".to_string(),
                },
                archive_path: PathBuf::from("/library/mslug.zip"),
                state: crate::dat::set::SetState::Complete,
                members_required: Vec::new(),
                members_verified: Vec::new(),
                members_bad: Vec::new(),
                members_optional: Vec::new(),
                members_borrowed: Vec::new(),
                disks_required: Vec::new(),
                disks_verified: Vec::new(),
                disks_parent_required: Vec::new(),
                dependencies: crate::dat::dependency::SetDependencyReport {
                    state: crate::dat::dependency::DependencyState::NotApplicable,
                    requirements: Vec::new(),
                },
            },
            identity_evidence: match ecosystem {
                DatEcosystem::FBNeo => FbneoIdentityEvidence::VerifiedDat {
                    source_id: "fbneo".to_string(),
                    ecosystem: DatEcosystem::FBNeo,
                },
                _ => FbneoIdentityEvidence::MameOnly {
                    source_id: "mame".to_string(),
                },
            },
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
    fn dreamcast_flycast_uses_the_existing_bios_projection() {
        let identity = resolved("Dreamcast", "T-8109N");
        let profile = flycast_profile();
        let profiles = [DiscoveredStandaloneProfile::Flycast {
            profile: &profile,
            bios: FlycastSystemFileState::PresentUnverified,
        }];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::DreamcastProductCode(
                "T-8109N".to_string(),
            )],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.summary.ready_with_warnings, 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "flycast",
                ..
            }
        ));
    }

    /// Unknown Flycast system state is deliberately not enough for strict
    /// readiness. A real hash-verified boot ROM is the only successful path.
    #[test]
    fn dreamcast_flycast_unconfigured_bios_is_not_strict_ready() {
        let identity = resolved("Dreamcast", "T-8109N");
        let profile = flycast_profile();
        let profiles = [DiscoveredStandaloneProfile::Flycast {
            profile: &profile,
            bios: FlycastSystemFileState::Unknown,
        }];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::DreamcastProductCode(
                "T-8109N".to_string(),
            )],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.summary.ready_with_warnings, 1);
        assert_eq!(
            plan.candidates[0].readiness,
            LaunchReadiness::ReadyWithWarnings
        );
    }

    #[test]
    fn dreamcast_flycast_verified_bios_reaches_strict_ready() {
        let identity = resolved("Dreamcast", "T-8109N");
        let profile = flycast_profile();
        let profiles = [DiscoveredStandaloneProfile::Flycast {
            profile: &profile,
            bios: FlycastSystemFileState::Verified,
        }];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::DreamcastProductCode(
                "T-8109N".to_string(),
            )],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.summary.ready, 1);
        assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
    }

    #[test]
    fn dreamcast_flycast_missing_firmware_is_blocked() {
        let identity = resolved("Dreamcast", "T-8109N");
        let profile = flycast_profile();
        let profiles = [DiscoveredStandaloneProfile::Flycast {
            profile: &profile,
            bios: FlycastSystemFileState::Missing,
        }];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::DreamcastProductCode(
                "T-8109N".to_string(),
            )],
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
    fn multiple_eligible_ppsspp_profiles_remain_undetermined() {
        let identity = resolved("PSP", "ULUS-10000");
        let profile_a = ppsspp_profile();
        let mut profile_b = profile_a.clone();
        profile_b.profile_id = "ppsspp-second".to_string();
        let profiles = [
            DiscoveredStandaloneProfile::ppsspp(&profile_a),
            DiscoveredStandaloneProfile::ppsspp(&profile_b),
        ];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::PspDiscId("ULUS-10000".to_string())],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates.len(), 2);
        assert!(plan.candidates.iter().all(|candidate| {
            candidate.preference == crate::launch::CandidatePreference::Undetermined
                && candidate.warnings.iter().any(|warning| {
                    matches!(
                        warning.kind,
                        crate::launch::LaunchWarningKind::MultipleEligibleProfiles
                    )
                })
        }));
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

    // --- Vita3K ---

    #[test]
    fn vita3k_profile_projects_to_a_distinct_playstation_vita_candidate() {
        let identity = resolved("PlayStation Vita", "PCSA00000");
        let profile = vita3k_profile();
        let profiles = [DiscoveredStandaloneProfile::Vita3k { profile: &profile }];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "vita3k",
                ref profile_id,
                ..
            } if profile_id == "vita3k-native"
        ));
    }

    #[test]
    fn vita3k_does_not_match_psp_or_ps3_and_has_no_fallback() {
        let profile = vita3k_profile();
        let profiles = [DiscoveredStandaloneProfile::Vita3k { profile: &profile }];
        for identity in [resolved("PSP", "ULUS-10000"), resolved("PS3", "BLUS00000")] {
            let plan = plan(
                &identity,
                &[],
                &resolved_content(),
                &profiles,
                &empty_retroarch(),
            );
            assert!(!plan.candidates.iter().any(|candidate| matches!(
                candidate.target,
                LaunchTarget::Standalone {
                    adapter_id: "vita3k",
                    ..
                }
            )));
        }
    }

    #[test]
    fn missing_vita3k_profile_reports_no_installation_instead_of_substitution() {
        let identity = resolved("PlayStation Vita", "PCSA00000");
        let plan = plan(&identity, &[], &resolved_content(), &[], &empty_retroarch());
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "none",
                ..
            }
        ));
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::NoInstallationCandidate)
        );
    }

    #[test]
    fn vita3k_candidate_generation_is_deterministic() {
        let identity = resolved("PlayStation Vita", "PCSA00000");
        let profile = vita3k_profile();
        let profiles = [DiscoveredStandaloneProfile::Vita3k { profile: &profile }];
        let first = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        let second = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(first, second);
    }

    #[test]
    fn xenia_without_a_verified_xex_id_never_becomes_a_candidate() {
        // A verified original-Xbox title ID is a different platform's fact
        // (see `project_xenia_launch_input`'s own doc comment) - it must
        // never substitute for Xenia's own directly-supplied XEX title/media
        // ID, so this profile stays unauthorized despite an unrelated fact
        // being present in `verified_identity_facts`.
        let identity = resolved("Xbox360", "4D5307E6");
        let profile = xenia_profile();
        let profiles = [DiscoveredStandaloneProfile::xenia(&profile, None, None)];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::XboxTitleId("4D5307E6".to_string())],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert!(matches!(
            project_xenia_launch_input(None, None),
            LaunchInputProjection::Unavailable { .. }
        ));
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::NoInstallationCandidate)
        );
    }

    #[test]
    fn xenia_with_a_verified_xex_title_id_becomes_a_ready_candidate() {
        let identity = resolved("Xbox360", "4D5307E6");
        let profile = xenia_profile();
        let profiles = [DiscoveredStandaloneProfile::xenia(
            &profile,
            Some("4D5307E6"),
            None,
        )];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "xenia",
                ..
            }
        ));
        assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
    }

    // --- Cemu ---

    #[test]
    fn wiiu_with_discovered_cemu_profile_becomes_a_candidate() {
        let identity = resolved("WiiU", "00050000101010ED");
        let profile = cemu_profile(true);
        let profiles = [DiscoveredStandaloneProfile::cemu(&profile)];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "cemu",
                ..
            }
        ));
    }

    #[test]
    fn non_wiiu_platform_never_produces_a_cemu_candidate() {
        let identity = resolved("Wii", "GALE01");
        let profile = cemu_profile(true);
        let profiles = [DiscoveredStandaloneProfile::cemu(&profile)];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert!(!plan.candidates.iter().any(|candidate| matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "cemu",
                ..
            }
        )));
    }

    #[test]
    fn cemu_missing_setup_blocker_is_preserved() {
        let identity = resolved("WiiU", "00050000101010ED");
        let profile = cemu_profile(false);
        let profiles = [DiscoveredStandaloneProfile::cemu(&profile)];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates.len(), 1);
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::ProfileIneligible)
        );
        assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Blocked);
    }

    // --- Amiberry ---

    #[test]
    fn amiga_with_discovered_amiberry_profile_becomes_a_candidate() {
        let identity = resolved("Amiga", "amiga-whdload-identity");
        let profile = amiga_profile(AmigaEmulatorKind::Amiberry, true);
        let inspection = amiga_inspection(&profile);
        let profiles = [DiscoveredStandaloneProfile::amiberry(&profile, &inspection)];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::AmigaIdentity(
                "amiga-whdload-identity".to_string(),
            )],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "amiberry",
                ..
            }
        ));
    }

    #[test]
    fn amiberry_missing_kickstart_blocker_is_preserved() {
        let identity = resolved("Amiga", "amiga-whdload-identity");
        let profile = amiga_profile(AmigaEmulatorKind::Amiberry, true);
        let inspection = amiga_inspection(&profile);
        let profiles = [DiscoveredStandaloneProfile::amiberry(&profile, &inspection)];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::AmigaIdentity(
                "amiga-whdload-identity".to_string(),
            )],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        // No Kickstart was configured on the fixture profile, so the shared
        // planner's own firmware condition must still surface it.
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::RequiredFirmwareMissing)
                || plan.candidates[0]
                    .warnings
                    .iter()
                    .any(|warning| warning.kind
                        == crate::launch::readiness::LaunchWarningKind::FirmwarePresentUnverified)
        );
    }

    // --- FS-UAE (already-wired regression: stays a separate candidate) ---

    #[test]
    fn amiberry_and_fsuae_remain_separate_candidates_for_the_same_platform() {
        let identity = resolved("Amiga", "amiga-whdload-identity");
        let amiberry = amiga_profile(AmigaEmulatorKind::Amiberry, true);
        let amiberry_inspection = amiga_inspection(&amiberry);
        let fsuae = amiga_profile(AmigaEmulatorKind::FsUae, true);
        let fsuae_inspection = amiga_inspection(&fsuae);
        let profiles = [
            DiscoveredStandaloneProfile::amiberry(&amiberry, &amiberry_inspection),
            DiscoveredStandaloneProfile::fsuae(&fsuae, &fsuae_inspection),
        ];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::AmigaIdentity(
                "amiga-whdload-identity".to_string(),
            )],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        let adapter_ids: Vec<&str> = plan
            .candidates
            .iter()
            .filter_map(|candidate| match candidate.target {
                LaunchTarget::Standalone { adapter_id, .. } => Some(adapter_id),
                LaunchTarget::RetroArchCore { .. } => None,
            })
            .collect();
        assert!(adapter_ids.contains(&"amiberry"));
        assert!(adapter_ids.contains(&"fsuae"));
        assert_eq!(adapter_ids.len(), 2);
    }

    // --- MAME ---

    #[test]
    fn arcade_with_trusted_mame_identity_becomes_a_candidate() {
        let identity = resolved("Arcade", "pacman");
        let executable = PathBuf::from("/usr/bin/mame");
        let resolutions = [mame_set_resolution(crate::dat::set::SetState::Complete)];
        let profiles = [DiscoveredStandaloneProfile::mame(
            Some(&executable),
            &resolutions,
        )];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "mame",
                ..
            }
        ));
    }

    #[test]
    fn arcade_with_no_trusted_mame_set_resolution_never_fakes_a_candidate() {
        let identity = resolved("Arcade", "pacman");
        let executable = PathBuf::from("/usr/bin/mame");
        let profiles = [DiscoveredStandaloneProfile::mame(Some(&executable), &[])];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert!(!plan.candidates.iter().any(|candidate| matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "mame",
                ..
            }
        )));
    }

    // --- FBNeo ---

    #[test]
    fn arcade_with_trusted_fbneo_identity_becomes_a_candidate() {
        let identity = resolved("Arcade", "mslug");
        let executable = PathBuf::from("/usr/bin/fbneo");
        let evidence = fbneo_set_evidence(DatEcosystem::FBNeo);
        let profiles = [DiscoveredStandaloneProfile::fbneo(
            Some(&executable),
            Some(&evidence),
        )];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "fbneo",
                ..
            }
        ));
    }

    #[test]
    fn mame_only_identity_never_produces_a_fbneo_candidate() {
        let identity = resolved("Arcade", "mslug");
        let executable = PathBuf::from("/usr/bin/fbneo");
        let evidence = fbneo_set_evidence(DatEcosystem::MAMEArcade);
        let profiles = [DiscoveredStandaloneProfile::fbneo(
            Some(&executable),
            Some(&evidence),
        )];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert!(!plan.candidates.iter().any(|candidate| matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "fbneo",
                ..
            }
        )));
    }

    // --- Multi-adapter ---

    #[test]
    fn mame_and_fbneo_remain_separate_candidates_for_the_same_arcade_set() {
        let identity = resolved("Arcade", "mslug");
        let mame_executable = PathBuf::from("/usr/bin/mame");
        let fbneo_executable = PathBuf::from("/usr/bin/fbneo");
        let resolutions = [mame_set_resolution(crate::dat::set::SetState::Complete)];
        let evidence = fbneo_set_evidence(DatEcosystem::FBNeo);
        let profiles = [
            DiscoveredStandaloneProfile::mame(Some(&mame_executable), &resolutions),
            DiscoveredStandaloneProfile::fbneo(Some(&fbneo_executable), Some(&evidence)),
        ];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        let adapter_ids: Vec<&str> = plan
            .candidates
            .iter()
            .filter_map(|candidate| match candidate.target {
                LaunchTarget::Standalone { adapter_id, .. } => Some(adapter_id),
                LaunchTarget::RetroArchCore { .. } => None,
            })
            .collect();
        assert!(adapter_ids.contains(&"mame"));
        assert!(adapter_ids.contains(&"fbneo"));
        assert_eq!(adapter_ids.len(), 2);
    }

    #[test]
    fn cemu_selection_never_silently_substitutes_another_adapter() {
        // No Cemu profile discovered at all: the shared planner must report
        // "nothing installed", never silently promote a RetroArch core or
        // any other adapter as if it were Cemu.
        let identity = resolved("WiiU", "00050000101010ED");
        let plan = plan(&identity, &[], &resolved_content(), &[], &empty_retroarch());
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "none",
                ..
            }
        ));
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::NoInstallationCandidate)
        );
    }

    #[test]
    fn standalone_candidate_ordering_is_deterministic_by_input_order() {
        let identity = resolved("Arcade", "mslug");
        let mame_executable = PathBuf::from("/usr/bin/mame");
        let fbneo_executable = PathBuf::from("/usr/bin/fbneo");
        let resolutions = [mame_set_resolution(crate::dat::set::SetState::Complete)];
        let evidence = fbneo_set_evidence(DatEcosystem::FBNeo);
        let profiles = [
            DiscoveredStandaloneProfile::mame(Some(&mame_executable), &resolutions),
            DiscoveredStandaloneProfile::fbneo(Some(&fbneo_executable), Some(&evidence)),
        ];
        let first = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        let second = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(first, second);
    }
}
