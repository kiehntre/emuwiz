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
    project_rpcs3_launch_input, project_scummvm_launch_input, project_xemu_launch_input,
    project_xenia_launch_input,
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
use crate::launch::scummvm_command::ScummVmNativeLaunchBinding;
use crate::patch_manager::{
    AmigaEmulatorKind, AmigaGameInspection, AmigaKickstartState, AmigaProfile, CemuProfile,
    DuckStationBiosState, DuckStationGameInspection, DuckStationProfile, FlycastGameInspection,
    FlycastProfile, FlycastSystemFileState, HatariGameInspection, HatariProfile,
    MelonDsFirmwareState, MelonDsProfile, MesenProfile, Pcsx2BiosVerification, Pcsx2GameInspection,
    Pcsx2Profile, PpssppProfile, Rpcs3GameInspection, Rpcs3Profile, SameBoyProfile, XemuProfile,
    XeniaProfile,
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
    /// A discovered native DeSmuME profile. V1 has no external firmware
    /// requirement: upstream defaults to HLE unless a user explicitly opts
    /// into external BIOS/firmware paths, which EmuWiz never supplies.
    Desmume {
        profile: &'a crate::launch::DesmumeProfile,
    },
    Mgba {
        profile: &'a crate::patch_manager::MgbaProfile,
    },
    /// A discovered native SameBoy profile. Its adapter keeps the final
    /// cartridge-header and executable revalidation at execution time; this
    /// projection only makes it a separate Game Boy/Color planner candidate.
    SameBoy {
        profile: &'a SameBoyProfile,
    },
    /// A discovered RMG (Rosalie's Mupen GUI) profile. RMG needs no
    /// BIOS/firmware for N64 cartridge play (see
    /// `crate::patch_manager::rmg_local`'s own module doc comment), exactly
    /// like [`Self::Mgba`] - so this variant carries no firmware state
    /// either.
    Rmg {
        profile: &'a crate::patch_manager::RmgProfile,
    },
    /// A discovered Stella (Atari 2600 emulator) profile. Stella needs no
    /// BIOS/firmware for standard Atari 2600 cartridge play (see
    /// `crate::patch_manager::stella_local`'s own module doc comment),
    /// exactly like [`Self::Rmg`] - so this variant carries no firmware
    /// state either. Stella and RetroArch remain separate candidates on
    /// the same `Atari2600` row - see
    /// [`crate::launch::platform_map::LAUNCH_COMPATIBILITY`]'s Atari2600
    /// entry, which lists `"stella"` as its own standalone adapter
    /// alongside a completely independent `retroarch_core_hints` entry.
    Stella {
        profile: &'a crate::patch_manager::StellaProfile,
    },
    Mesen {
        profile: &'a MesenProfile,
    },
    /// A discovered standalone Snes9x executable/profile. Snes9x (Super
    /// Nintendo / Super Famicom) is a wholly independent candidate from the
    /// RetroArch `snes9x` core for the same `SNES` platform - both are
    /// surfaced, neither is auto-preferred. Needs no firmware/BIOS.
    Snes9x {
        profile: &'a crate::patch_manager::Snes9xProfile,
    },
    /// A discovered VICE C64 executable/profile (`x64sc` or `x64`, kept as
    /// distinct exact profiles - never silently substituted for each
    /// other). Needs no firmware/BIOS: VICE resolves its own C64 system ROM
    /// files through its normal installed search path.
    Vice {
        profile: &'a crate::patch_manager::ViceProfile,
    },
    OpenMsx {
        profile: &'a crate::patch_manager::OpenMsxProfile,
    },
    /// A discovered native ScummVM binding. The verified engine:game ID is
    /// projected from identity facts; the binding itself is revalidated by
    /// ScummVM preflight before execution.
    ScummVm {
        binding: &'a ScummVmNativeLaunchBinding,
        eligible: bool,
    },
    Vita3k {
        profile: &'a crate::patch_manager::Vita3kProfile,
    },
    /// A discovered Azahar executable/profile. Azahar's own Phase 1 scope
    /// (loose `.3dsx` homebrew only - no retail formats, no installed-title
    /// launch, no CIA install, no decryption; see
    /// [`crate::patch_manager::azahar_local`]) is never re-implemented or
    /// widened here: this only makes the adapter *discoverable* as a coarse
    /// candidate. The real `.3dsx`/SMDH/config/key evidence check still
    /// happens exclusively in
    /// [`crate::launch::azahar_command::build_azahar_command_plan`] and
    /// [`crate::launch::azahar_execution::preflight_azahar_launch`] - a
    /// candidate reported `Ready` here is never authorization to launch
    /// anything but a `.3dsx` file that adapter's own preflight has itself
    /// revalidated. [`crate::patch_manager::azahar_local::AzaharProfile`]
    /// has no `profile_id`/`eligible` field of its own (unlike every other
    /// adapter's profile type) - both are synthesized in
    /// [`project_standalone_profiles`] from the executable path, the same
    /// way MAME/FBNeo's own executable-only evidence is projected.
    Azahar {
        profile: &'a crate::patch_manager::AzaharProfile,
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

    pub fn desmume(profile: &'a crate::launch::DesmumeProfile) -> Self {
        Self::Desmume { profile }
    }

    pub fn mgba(profile: &'a crate::patch_manager::MgbaProfile) -> Self {
        Self::Mgba { profile }
    }

    pub fn sameboy(profile: &'a SameBoyProfile) -> Self {
        Self::SameBoy { profile }
    }

    pub fn mesen(profile: &'a MesenProfile) -> Self {
        Self::Mesen { profile }
    }

    pub fn rmg(profile: &'a crate::patch_manager::RmgProfile) -> Self {
        Self::Rmg { profile }
    }

    pub fn snes9x(profile: &'a crate::patch_manager::Snes9xProfile) -> Self {
        Self::Snes9x { profile }
    }

    pub fn stella(profile: &'a crate::patch_manager::StellaProfile) -> Self {
        Self::Stella { profile }
    }

    pub fn vice(profile: &'a crate::patch_manager::ViceProfile) -> Self {
        Self::Vice { profile }
    }

    pub fn openmsx(profile: &'a crate::patch_manager::OpenMsxProfile) -> Self {
        Self::OpenMsx { profile }
    }

    pub fn scummvm(binding: &'a ScummVmNativeLaunchBinding, eligible: bool) -> Self {
        Self::ScummVm { binding, eligible }
    }

    pub fn vita3k(profile: &'a crate::patch_manager::Vita3kProfile) -> Self {
        Self::Vita3k { profile }
    }

    pub fn azahar(profile: &'a crate::patch_manager::AzaharProfile) -> Self {
        Self::Azahar { profile }
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
            DiscoveredStandaloneProfile::Desmume { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "Nintendo DS") =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "desmume",
                    profile_id: profile.profile_id.clone(),
                    profile_path: None,
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
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
            DiscoveredStandaloneProfile::SameBoy { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if crate::launch::sameboy_command::SAMEBOY_SUPPORTED_PLATFORM_IDS.contains(&identity.platform_id.as_str())) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "sameboy",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
    DiscoveredStandaloneProfile::Rmg { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "N64") =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "rmg",
                    profile_id: profile.profile_id.clone(),
                    profile_path: None,
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::Stella { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "Atari2600") =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "stella",
                    profile_id: profile.profile_id.clone(),
                    profile_path: None,
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::Mesen { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if crate::launch::mesen_command::MESEN_SUPPORTED_PLATFORM_IDS.contains(&identity.platform_id.as_str())) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "mesen", profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()), eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::Snes9x { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "SNES") =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "snes9x",
                    profile_id: profile.profile_id.clone(),
                    profile_path: None,
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::Vice { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "Commodore 64") =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "vice",
                    profile_id: profile.profile_id.clone(),
                    profile_path: None,
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::OpenMsx { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if matches!(identity.platform_id.as_str(), "MSX" | "MSX2")) =>
            {
                Some(StandaloneProfileInput {
                    adapter_id: "openmsx",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.executable.path.clone()),
                    eligible: profile.eligible,
                    firmware: FirmwareReadiness::NotRequired,
                })
            }
            DiscoveredStandaloneProfile::ScummVm { binding, eligible }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "ScummVM")
                    && authorized(project_scummvm_launch_input(input.verified_identity_facts))
            => Some(StandaloneProfileInput {
                adapter_id: "scummvm",
                profile_id: binding.executable.display().to_string(),
                profile_path: Some(binding.executable.clone()),
                eligible: *eligible,
                firmware: FirmwareReadiness::NotRequired,
            }),
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
            DiscoveredStandaloneProfile::Azahar { profile }
                if matches!(input.identity, CanonicalIdentityStatus::Resolved(identity)
                    if identity.platform_id == "Nintendo 3DS") =>
            {
                // AzaharProfile carries no profile_id/eligible of its own -
                // synthesized here from the executable path, exactly like
                // the MAME/FBNeo executable-only projection below. Only an
                // existing-but-broken config blocks eligibility, never a
                // config that was simply never written yet - the same rule
                // every profile-having adapter in this file already applies.
                let eligible = !matches!(
                    profile.config_state,
                    crate::patch_manager::AzaharEvidenceState::Unreadable
                        | crate::patch_manager::AzaharEvidenceState::Oversized
                );
                Some(StandaloneProfileInput {
                    adapter_id: "azahar",
                    profile_id: format!("azahar:{}", profile.executable.display()),
                    profile_path: profile.config.clone(),
                    eligible,
                    firmware: FirmwareReadiness::NotRequired,
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

    fn retroarch_with_snes9x_core() -> RetroArchEnvironmentReport {
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
                    file_name: EncodedPath::from_path(&PathBuf::from("snes9x_libretro.so")),
                    full_path: EncodedPath::from_path(&PathBuf::from(
                        "/retroarch/cores/snes9x_libretro.so",
                    )),
                    core_stem: "snes9x".to_string(),
                    info: CoreInfoFinding::Found {
                        display_name: None,
                        display_version: None,
                        system_name: Some("Nintendo - SNES / SFC".to_string()),
                        supported_extensions: Vec::new(),
                        core_name: Some("snes9x".to_string()),
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

    fn rmg_profile(eligible: bool) -> crate::patch_manager::RmgProfile {
        crate::patch_manager::RmgProfile {
            profile_id: "rmg:native".to_string(),
            installation_type: crate::patch_manager::RmgInstallationType::Native,
            eligible,
            blocker: (!eligible).then(|| "no safe RMG executable was discovered".to_string()),
            executable_candidates: Vec::new(),
        }
    }

    fn stella_profile(eligible: bool) -> crate::patch_manager::StellaProfile {
        crate::patch_manager::StellaProfile {
            profile_id: "stella:native".to_string(),
            installation_type: crate::patch_manager::StellaInstallationType::Native,
            eligible,
            blocker: (!eligible).then(|| "no safe Stella executable was discovered".to_string()),
            executable_candidates: Vec::new(),
        }
    }

    fn vice_profile(eligible: bool) -> crate::patch_manager::ViceProfile {
        crate::patch_manager::ViceProfile {
            profile_id: "vice:native:/usr/bin/x64sc".to_string(),
            installation_type: crate::patch_manager::ViceInstallationType::Native,
            eligible,
            blocker: (!eligible).then(|| {
                "VICE requires an exact regular executable named x64sc or x64 with an execute bit"
                    .to_string()
            }),
            executable: eligible.then(|| crate::patch_manager::ViceExecutable {
                path: "/usr/bin/x64sc".into(),
                installation_type: crate::patch_manager::ViceInstallationType::Native,
                kind: crate::patch_manager::ViceC64ExecutableKind::X64sc,
                version: None,
            }),
        }
    }

    fn azahar_profile(
        config_state: crate::patch_manager::AzaharEvidenceState,
    ) -> crate::patch_manager::AzaharProfile {
        crate::patch_manager::AzaharProfile {
            executable: PathBuf::from("/opt/azahar"),
            config: Some(PathBuf::from("/profiles/azahar/qt-config.ini")),
            config_state,
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
    fn verified_scummvm_game_projects_as_a_standalone_candidate() {
        let identity = resolved("ScummVM", "scumm:monkey");
        let binding = ScummVmNativeLaunchBinding {
            executable: PathBuf::from("/usr/games/scummvm"),
        };
        let profiles = [DiscoveredStandaloneProfile::scummvm(&binding, true)];
        let plan = plan(
            &identity,
            &[VerifiedIdentityFact::ScummVmGameId(
                "scumm:monkey".to_string(),
            )],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates.len(), 1);
        assert!(matches!(
            plan.candidates[0].target,
            LaunchTarget::Standalone {
                adapter_id: "scummvm",
                ..
            }
        ));
        assert_eq!(plan.candidates[0].firmware, FirmwareReadiness::NotRequired);
    }

    #[test]
    fn scummvm_without_verified_game_id_is_not_projected() {
        let identity = resolved("ScummVM", "scumm:monkey");
        let binding = ScummVmNativeLaunchBinding {
            executable: PathBuf::from("/usr/games/scummvm"),
        };
        let profiles = [DiscoveredStandaloneProfile::scummvm(&binding, true)];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert!(plan.candidates.iter().all(|candidate| {
            !matches!(
                candidate.target,
                LaunchTarget::Standalone {
                    adapter_id: "scummvm",
                    ..
                }
            )
        }));
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

    // --- Azahar ---

    #[test]
    fn azahar_profile_projects_to_a_nintendo_3ds_candidate() {
        let identity = resolved("Nintendo 3DS", "homebrew-title.3dsx");
        let profile = azahar_profile(crate::patch_manager::AzaharEvidenceState::Present);
        let profiles = [DiscoveredStandaloneProfile::azahar(&profile)];
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
                adapter_id: "azahar",
                ..
            }
        ));
    }

    fn snes9x_profile() -> crate::patch_manager::Snes9xProfile {
        crate::patch_manager::Snes9xProfile {
            profile_id: "snes9x:/usr/bin/snes9x-gtk".to_string(),
            installation_type: crate::patch_manager::Snes9xInstallationType::Native,
            executable: PathBuf::from("/usr/bin/snes9x-gtk"),
            eligible: true,
            blocker: None,
            version: Some("1.63".to_string()),
        }
    }

    fn sameboy_profile(eligible: bool) -> SameBoyProfile {
        let root = PathBuf::from("/profiles/sameboy");
        SameBoyProfile {
            profile_id: "sameboy:/profiles/sameboy".to_string(),
            installation_type: crate::patch_manager::SameBoyInstallationType::Native,
            configuration_path: root.clone(),
            config: crate::patch_manager::SameBoyConfigInspection {
                path: root.join("prefs.bin"),
                exists: false,
                readable: false,
                oversized: false,
            },
            eligible,
            blocker: (!eligible).then(|| "no safe SameBoy executable was discovered".to_string()),
            executable_candidates: Vec::new(),
            boot_rom: crate::patch_manager::SameBoyBootRomEvidence {
                directory: None,
                state: crate::patch_manager::SameBoyBootRomState::NotConfigured,
            },
        }
    }

    fn game_boy_content(extension: &str) -> LaunchContentRef {
        LaunchContentRef {
            kind: Some(LaunchContentKind::Cartridge),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(PathBuf::from(format!("/library/game.{extension}"))),
            requires_mount: false,
            provenance: "direct Game Boy cartridge".to_string(),
        }
    }

    #[test]
    fn sameboy_projects_for_game_boy_and_color_as_a_separate_candidate() {
        let profile = sameboy_profile(true);
        let profiles = [DiscoveredStandaloneProfile::sameboy(&profile)];
        for (platform_id, extension) in [("Game Boy", "gb"), ("Game Boy Color", "gbc")] {
            let launch_plan = plan(
                &resolved(platform_id, "verified-gb-key"),
                &[],
                &game_boy_content(extension),
                &profiles,
                &empty_retroarch(),
            );
            assert!(launch_plan.candidates.iter().any(|candidate| matches!(
                candidate.target,
                LaunchTarget::Standalone {
                    adapter_id: "sameboy",
                    ..
                }
            )));
            assert!(
                launch_plan.candidates.iter().all(|candidate| {
                    candidate.preference != crate::launch::CandidatePreference::Remembered
                }),
                "registration does not select SameBoy automatically"
            );
        }
    }

    #[test]
    fn sameboy_refuses_unrelated_platforms_and_propagates_profile_readiness() {
        let eligible = sameboy_profile(true);
        let profiles = [DiscoveredStandaloneProfile::sameboy(&eligible)];
        let unrelated = plan(
            &resolved("Game Boy Advance", "gba-key"),
            &[],
            &game_boy_content("gb"),
            &profiles,
            &empty_retroarch(),
        );
        assert!(!unrelated.candidates.iter().any(|candidate| matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "sameboy",
                ..
            }
        )));

        let ineligible = sameboy_profile(false);
        let profiles = [DiscoveredStandaloneProfile::sameboy(&ineligible)];
        let blocked = plan(
            &resolved("Game Boy", "gb-key"),
            &[],
            &game_boy_content("gb"),
            &profiles,
            &empty_retroarch(),
        );
        assert!(blocked.candidates.iter().any(|candidate| {
            matches!(
                candidate.target,
                LaunchTarget::Standalone {
                    adapter_id: "sameboy",
                    ..
                }
            ) && candidate.readiness == LaunchReadiness::Blocked
                && candidate
                    .blockers
                    .iter()
                    .any(|blocker| blocker.kind == LaunchBlockerKind::ProfileIneligible)
        }));
    }

    #[test]
    fn snes9x_profile_projects_to_a_snes_candidate_separate_from_retroarch() {
        let identity = resolved("SNES", "verified-snes-key");
        let profile = snes9x_profile();
        let profiles = [DiscoveredStandaloneProfile::snes9x(&profile)];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &retroarch_with_snes9x_core(),
        );
        let standalone = plan.candidates.iter().any(|candidate| {
            matches!(
                candidate.target,
                LaunchTarget::Standalone {
                    adapter_id: "snes9x",
                    ..
                }
            )
        });
        let retroarch = plan
            .candidates
            .iter()
            .any(|candidate| matches!(candidate.target, LaunchTarget::RetroArchCore { .. }));
        assert!(standalone, "standalone Snes9x is projected for SNES");
        assert!(
            retroarch,
            "the RetroArch snes9x core stays its own candidate"
        );
        assert_eq!(plan.candidates.len(), 2);
        assert!(
            plan.candidates.iter().all(|candidate| {
                candidate.preference != crate::launch::CandidatePreference::Remembered
            }),
            "no automatic winner between the two SNES candidates"
        );
    }

    #[test]
    fn non_snes_identity_never_produces_a_snes9x_candidate() {
        let profile = snes9x_profile();
        let profiles = [DiscoveredStandaloneProfile::snes9x(&profile)];
        for identity in [
            resolved("NES", "nes-key"),
            resolved("Game Boy", "gb-key"),
            resolved("N64", "n64-key"),
        ] {
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
                    adapter_id: "snes9x",
                    ..
                }
            )));
        }
    }

    #[test]
    fn nintendo_ds_wiiu_and_switch_never_produce_an_azahar_candidate() {
        let profile = azahar_profile(crate::patch_manager::AzaharEvidenceState::Present);
        let profiles = [DiscoveredStandaloneProfile::azahar(&profile)];
        for identity in [
            resolved("Nintendo DS", "NTR-ABCE"),
            resolved("WiiU", "00050000101010ED"),
            resolved("Switch", "0100000000010000"),
        ] {
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
                    adapter_id: "azahar",
                    ..
                }
            )));
        }
    }

    #[test]
    fn azahar_candidate_uses_the_exact_adapter_id() {
        let identity = resolved("Nintendo 3DS", "homebrew-title.3dsx");
        let profile = azahar_profile(crate::patch_manager::AzaharEvidenceState::Present);
        let profiles = [DiscoveredStandaloneProfile::azahar(&profile)];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        let LaunchTarget::Standalone { adapter_id, .. } = plan.candidates[0].target else {
            panic!("expected a standalone target");
        };
        assert_eq!(adapter_id, "azahar");
    }

    #[test]
    fn azahar_candidate_generation_is_deterministic() {
        let identity = resolved("Nintendo 3DS", "homebrew-title.3dsx");
        let profile = azahar_profile(crate::patch_manager::AzaharEvidenceState::Present);
        let profiles = [DiscoveredStandaloneProfile::azahar(&profile)];
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
    fn missing_azahar_profile_yields_no_installation_candidate_never_a_fallback() {
        let identity = resolved("Nintendo 3DS", "homebrew-title.3dsx");
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
    fn unreadable_azahar_config_blocks_eligibility_but_never_falls_back() {
        let identity = resolved("Nintendo 3DS", "homebrew-title.3dsx");
        let profile = azahar_profile(crate::patch_manager::AzaharEvidenceState::Unreadable);
        let profiles = [DiscoveredStandaloneProfile::azahar(&profile)];
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
                adapter_id: "azahar",
                ..
            }
        ));
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::ProfileIneligible)
        );
    }

    #[test]
    fn azahar_shared_candidate_carries_no_content_form_or_smdh_evidence() {
        // The shared candidate layer must stay coarse: it only ever exposes
        // adapter_id/profile_id/eligibility/firmware. Whether the selected
        // content is actually a launchable .3dsx (vs. a .3ds/.cci/.cxi/.cia
        // this build refuses) is never decided here - only
        // `azahar_command::build_azahar_command_plan` and
        // `azahar_execution::preflight_azahar_launch` inspect the file
        // itself. This test exists to name that boundary: a `Ready`
        // candidate here proves nothing about content-form eligibility.
        let identity = resolved("Nintendo 3DS", "homebrew-title.3dsx");
        let profile = azahar_profile(crate::patch_manager::AzaharEvidenceState::Present);
        let profiles = [DiscoveredStandaloneProfile::azahar(&profile)];
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &empty_retroarch(),
        );
        assert_eq!(plan.candidates[0].firmware, FirmwareReadiness::NotRequired);
    }

    // --- RMG (Nintendo 64) ---

    #[test]
    fn n64_rmg_profile_becomes_a_ready_candidate_without_firmware() {
        let identity = resolved("N64", "z64sha");
        let profile = rmg_profile(true);
        let profiles = [DiscoveredStandaloneProfile::rmg(&profile)];
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
                adapter_id: "rmg",
                ..
            }
        ));
        assert_eq!(plan.candidates[0].firmware, FirmwareReadiness::NotRequired);
        assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
    }

    #[test]
    fn rmg_does_not_match_an_unrelated_platform_and_has_no_fallback() {
        let profile = rmg_profile(true);
        let profiles = [DiscoveredStandaloneProfile::rmg(&profile)];
        for identity in [resolved("PSX", "SLUS-12345"), resolved("Game Boy", "gbsha")] {
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
                    adapter_id: "rmg",
                    ..
                }
            )));
        }
    }

    #[test]
    fn ineligible_rmg_profile_blocks_but_never_falls_back_to_retroarch() {
        let identity = resolved("N64", "z64sha");
        let profile = rmg_profile(false);
        let profiles = [DiscoveredStandaloneProfile::rmg(&profile)];
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
                adapter_id: "rmg",
                ..
            }
        ));
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::ProfileIneligible)
        );
    }

    #[test]
    fn missing_rmg_profile_reports_no_installation_instead_of_substitution() {
        let identity = resolved("N64", "z64sha");
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

    /// RMG and a discovered RetroArch N64 core must coexist as two distinct
    /// candidates for the same platform - never merged, and neither one
    /// automatically preferred over the other. The GUI/user still chooses.
    #[test]
    fn rmg_and_retroarch_coexist_as_separate_n64_candidates() {
        let identity = resolved("N64", "z64sha");
        let profile = rmg_profile(true);
        let profiles = [DiscoveredStandaloneProfile::rmg(&profile)];
        let retroarch_with_n64_core = {
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
                        file_name: EncodedPath::from_path(&PathBuf::from(
                            "mupen64plus_next_libretro.so",
                        )),
                        full_path: EncodedPath::from_path(&PathBuf::from(
                            "/retroarch/cores/mupen64plus_next_libretro.so",
                        )),
                        core_stem: "mupen64plus_next".to_string(),
                        info: CoreInfoFinding::Found {
                            display_name: None,
                            display_version: None,
                            system_name: Some("Nintendo - Nintendo 64".to_string()),
                            supported_extensions: Vec::new(),
                            core_name: Some("mupen64plus_next".to_string()),
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
        };
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &retroarch_with_n64_core,
        );
        assert_eq!(plan.candidates.len(), 2, "{:?}", plan.candidates);
        assert!(plan.candidates.iter().any(|c| matches!(
            c.target,
            LaunchTarget::Standalone {
                adapter_id: "rmg",
                ..
            }
        )));
        assert!(plan.candidates.iter().any(|c| matches!(
            c.target,
            LaunchTarget::RetroArchCore { ref core_stem, .. } if core_stem == "mupen64plus_next"
        )));
        // Neither candidate is discarded, hidden, or silently merged into the
        // other - both remain in `plan.candidates` with their own
        // independent readiness/preference (the reviewed single-hint
        // RetroArch core legitimately reports `SoleEligible` *among RetroArch
        // cores* - see `apply_preference`'s own doc comment - which is not
        // the same thing as RMG being displaced; RMG's own candidate is
        // still present and still `Ready`). Nothing here ever picks one
        // target over the other automatically - that choice is left to the
        // caller/GUI.
        assert!(
            plan.candidates
                .iter()
                .all(|c| c.readiness != LaunchReadiness::Blocked),
            "{:?}",
            plan.candidates
                .iter()
                .map(|c| c.readiness)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn rmg_candidate_generation_is_deterministic() {
        let identity = resolved("N64", "z64sha");
        let profile = rmg_profile(true);
        let profiles = [DiscoveredStandaloneProfile::rmg(&profile)];
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

    // --- Stella (Atari 2600) ---

    #[test]
    fn atari2600_stella_profile_becomes_a_ready_candidate_without_firmware() {
        let identity = resolved("Atari2600", "a26sha");
        let profile = stella_profile(true);
        let profiles = [DiscoveredStandaloneProfile::stella(&profile)];
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
                adapter_id: "stella",
                ..
            }
        ));
        assert_eq!(plan.candidates[0].firmware, FirmwareReadiness::NotRequired);
        assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
    }

    #[test]
    fn stella_does_not_match_an_unrelated_platform_and_has_no_fallback() {
        let profile = stella_profile(true);
        let profiles = [DiscoveredStandaloneProfile::stella(&profile)];
        for identity in [resolved("Atari5200", "a5200sha"), resolved("N64", "z64sha")] {
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
                    adapter_id: "stella",
                    ..
                }
            )));
        }
    }

    #[test]
    fn ineligible_stella_profile_blocks_but_never_falls_back_to_retroarch() {
        let identity = resolved("Atari2600", "a26sha");
        let profile = stella_profile(false);
        let profiles = [DiscoveredStandaloneProfile::stella(&profile)];
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
                adapter_id: "stella",
                ..
            }
        ));
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::ProfileIneligible)
        );
    }

    #[test]
    fn missing_stella_profile_reports_no_installation_instead_of_substitution() {
        let identity = resolved("Atari2600", "a26sha");
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

    /// Stella and a discovered RetroArch Atari 2600 core must coexist as two
    /// distinct candidates for the same platform - never merged, and
    /// neither one automatically preferred over the other. The GUI/user
    /// still chooses.
    #[test]
    fn stella_and_retroarch_coexist_as_separate_atari2600_candidates() {
        let identity = resolved("Atari2600", "a26sha");
        let profile = stella_profile(true);
        let profiles = [DiscoveredStandaloneProfile::stella(&profile)];
        let retroarch_with_atari2600_core = {
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
                        file_name: EncodedPath::from_path(&PathBuf::from("stella_libretro.so")),
                        full_path: EncodedPath::from_path(&PathBuf::from(
                            "/retroarch/cores/stella_libretro.so",
                        )),
                        core_stem: "stella".to_string(),
                        info: CoreInfoFinding::Found {
                            display_name: None,
                            display_version: None,
                            system_name: Some("Atari - 2600".to_string()),
                            supported_extensions: Vec::new(),
                            core_name: Some("stella".to_string()),
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
        };
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &retroarch_with_atari2600_core,
        );
        assert_eq!(plan.candidates.len(), 2, "{:?}", plan.candidates);
        assert!(plan.candidates.iter().any(|c| matches!(
            c.target,
            LaunchTarget::Standalone {
                adapter_id: "stella",
                ..
            }
        )));
        assert!(plan.candidates.iter().any(|c| matches!(
            c.target,
            LaunchTarget::RetroArchCore { ref core_stem, .. } if core_stem == "stella"
        )));
        // Neither candidate is discarded, hidden, or silently merged into the
        // other - both remain in `plan.candidates` with their own
        // independent readiness/preference, exactly like the RMG/RetroArch
        // N64 coexistence test above. No automatic winner is picked here.
        assert!(
            plan.candidates
                .iter()
                .all(|c| c.readiness != LaunchReadiness::Blocked),
            "{:?}",
            plan.candidates
                .iter()
                .map(|c| c.readiness)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn stella_candidate_generation_is_deterministic() {
        let identity = resolved("Atari2600", "a26sha");
        let profile = stella_profile(true);
        let profiles = [DiscoveredStandaloneProfile::stella(&profile)];
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

    // --- VICE (Commodore 64) ---

    #[test]
    fn c64_vice_profile_becomes_a_ready_candidate_without_firmware() {
        let identity = resolved("Commodore 64", "c64sha");
        let profile = vice_profile(true);
        let profiles = [DiscoveredStandaloneProfile::vice(&profile)];
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
                adapter_id: "vice",
                ..
            }
        ));
        assert_eq!(plan.candidates[0].firmware, FirmwareReadiness::NotRequired);
        assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
    }

    #[test]
    fn vice_does_not_match_an_unrelated_platform_and_has_no_fallback() {
        let profile = vice_profile(true);
        let profiles = [DiscoveredStandaloneProfile::vice(&profile)];
        for identity in [
            resolved("Commodore 128", "c128sha"),
            resolved("VIC-20", "vic20sha"),
        ] {
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
                    adapter_id: "vice",
                    ..
                }
            )));
        }
    }

    #[test]
    fn ineligible_vice_profile_blocks_but_never_falls_back_to_retroarch() {
        let identity = resolved("Commodore 64", "c64sha");
        let profile = vice_profile(false);
        let profiles = [DiscoveredStandaloneProfile::vice(&profile)];
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
                adapter_id: "vice",
                ..
            }
        ));
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::ProfileIneligible)
        );
    }

    #[test]
    fn missing_vice_profile_reports_no_installation_instead_of_substitution() {
        let identity = resolved("Commodore 64", "c64sha");
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

    /// VICE and a discovered RetroArch Commodore 64 core must coexist as two
    /// distinct candidates for the same platform - never merged, and
    /// neither one automatically preferred over the other. The GUI/user
    /// still chooses.
    #[test]
    fn vice_and_retroarch_coexist_as_separate_c64_candidates() {
        let identity = resolved("Commodore 64", "c64sha");
        let profile = vice_profile(true);
        let profiles = [DiscoveredStandaloneProfile::vice(&profile)];
        let retroarch_with_c64_core = {
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
                        file_name: EncodedPath::from_path(&PathBuf::from("vice_x64sc_libretro.so")),
                        full_path: EncodedPath::from_path(&PathBuf::from(
                            "/retroarch/cores/vice_x64sc_libretro.so",
                        )),
                        core_stem: "vice_x64sc".to_string(),
                        info: CoreInfoFinding::Found {
                            display_name: None,
                            display_version: None,
                            system_name: Some("Commodore 64".to_string()),
                            supported_extensions: Vec::new(),
                            core_name: Some("vice_x64sc".to_string()),
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
        };
        let plan = plan(
            &identity,
            &[],
            &resolved_content(),
            &profiles,
            &retroarch_with_c64_core,
        );
        assert_eq!(plan.candidates.len(), 2, "{:?}", plan.candidates);
        assert!(plan.candidates.iter().any(|c| matches!(
            c.target,
            LaunchTarget::Standalone {
                adapter_id: "vice",
                ..
            }
        )));
        assert!(plan.candidates.iter().any(|c| matches!(
            c.target,
            LaunchTarget::RetroArchCore { ref core_stem, .. } if core_stem == "vice_x64sc"
        )));
        // Neither candidate is discarded, hidden, or silently merged into the
        // other - both remain in `plan.candidates` with their own
        // independent readiness/preference, exactly like the Stella/RMG
        // coexistence tests above. No automatic winner is picked here.
        assert!(
            plan.candidates
                .iter()
                .all(|c| c.readiness != LaunchReadiness::Blocked),
            "{:?}",
            plan.candidates
                .iter()
                .map(|c| c.readiness)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn vice_candidate_generation_is_deterministic() {
        let identity = resolved("Commodore 64", "c64sha");
        let profile = vice_profile(true);
        let profiles = [DiscoveredStandaloneProfile::vice(&profile)];
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
