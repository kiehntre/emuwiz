//! Shared launch-readiness vocabulary, plus pure projections from each
//! adapter's own existing BIOS/firmware/TOS state enum onto
//! [`FirmwareReadiness`].
//!
//! Every adapter-specific enum this module projects from
//! ([`crate::patch_manager::DuckStationBiosState`],
//! [`crate::patch_manager::Pcsx2BiosVerification`],
//! [`crate::patch_manager::Rpcs3FirmwareStatus`],
//! [`crate::patch_manager::XemuSystemFileState`],
//! [`crate::patch_manager::FlycastSystemFileState`],
//! [`crate::patch_manager::HatariTosHealth`]) is left completely
//! unchanged - these functions only ever read one, never construct or
//! mutate one, and never become the enum an adapter's own inspection code
//! returns. PPSSPP has no BIOS/firmware requirement at all: real PSP
//! discs/PBP images need no separate firmware file the way PS1/PS2/Xbox/
//! Dreamcast/3DS do, so [`ppsspp_firmware_readiness`] is a constant, not a
//! projection, and always answers [`FirmwareReadiness::NotRequired`] -
//! inventing a PSP BIOS requirement here would be exactly the kind of
//! unreviewed assumption this module exists to avoid.

use crate::emulator_environment::retroarch::CoreInfoFinding;
use crate::patch_manager::{
    DuckStationBiosState, FlycastSystemFileState, HatariTosHealth, PceCdFirmwareReadiness,
    Pcsx2BiosVerification, Rpcs3FirmwareStatus, XemuSystemFileState,
};

/// Whether a platform's firmware/BIOS/TOS requirement is currently
/// satisfied, downstream-only - never fed back into canonical identity or
/// evidence. Deliberately coarser than any one adapter's own enum: this is
/// the common vocabulary every adapter's specific state projects onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareReadiness {
    /// A trusted verifier confirmed the exact file. See each adapter's own
    /// `Verified`-equivalent variant doc comment for whether any such
    /// verifier is actually wired up yet - most are not.
    Verified,
    /// A file exists where firmware is expected, but nothing verified its
    /// contents - filename or mere presence alone is never verification.
    PresentUnverified,
    /// Firmware is required and confirmed absent.
    Missing,
    /// Genuinely unknown - unreadable location, not configured, or the
    /// adapter's own state carries no evidence either way. Never treated as
    /// a proven failure and never treated as proven success.
    Unknown,
    /// This platform/target has no firmware requirement at all (PPSSPP is
    /// the only adapter this currently applies to).
    NotRequired,
}

/// Overall verdict for one [`crate::launch::planning::LaunchCandidate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchReadiness {
    /// No blocker and no warning.
    Ready,
    /// No blocker, but at least one [`LaunchWarning`] - still launchable,
    /// worth surfacing.
    ReadyWithWarnings,
    /// At least one [`LaunchBlocker`] - not currently launchable.
    Blocked,
}

/// Why one candidate cannot currently be launched. Structured, never
/// free-text-only - [`LaunchBlocker::detail`] carries the human-readable
/// explanation alongside this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchBlockerKind {
    /// A native Azahar launch was given a non-Nintendo 3DS identity.
    AzaharPlatformMismatch,
    /// Azahar Phase 1 accepts only a loose `.3dsx` homebrew file.
    AzaharContentFormatUnsupported,
    /// The selected Azahar configuration exists but cannot safely be read.
    AzaharProfileUnavailable,
    /// A command-plan request was given a non-RetroArch launch candidate.
    RetroArchCandidateRequired,
    /// A caller supplied a candidate marked blocked without supplying the
    /// candidate's own blocker detail.
    CandidateBlocked,
    /// The canonical game identity this plan was built for is
    /// [`crate::launch::planning::CanonicalIdentityStatus::Unknown`].
    IdentityUnresolved,
    /// The canonical game identity this plan was built for is
    /// [`crate::launch::planning::CanonicalIdentityStatus::Conflict`].
    IdentityConflict,
    /// No discovered standalone profile or RetroArch core is a candidate
    /// for this platform at all.
    NoInstallationCandidate,
    /// [`crate::launch::planning::LaunchContentRef`] carries no resolved,
    /// runnable path.
    ContentNotResolved,
    /// [`FirmwareReadiness::Missing`] for a target that requires firmware.
    RequiredFirmwareMissing,
    /// The discovered profile itself reports `eligible: false`.
    ProfileIneligible,
    /// A RetroArch profile is discovered but no core resolves to this
    /// platform.
    CoreMissing,
    /// More than one distinct RetroArch core resolves to this platform and
    /// nothing (a caller-supplied preferred core stem) disambiguates them -
    /// automatic selection would be unsafe.
    AmbiguousCore,
    /// The RetroArch profile selected by a candidate is absent from the
    /// environment report used to make its command plan.
    RetroArchProfileMissing,
    /// More than one environment profile has the candidate's profile
    /// reference, so selection is not safe.
    AmbiguousRetroArchProfile,
    /// The selected core is present but its current `.info` metadata no
    /// longer resolves to the candidate's canonical platform.
    RetroArchCoreMismatch,
    /// No exact RetroArch executable path is available for the selected
    /// profile in the inspected environment.
    RetroArchExecutableMissing,
    /// More than one exact executable path remains for the selected profile.
    AmbiguousRetroArchExecutable,
    /// An environment-report path was rendered lossily and therefore cannot
    /// be reconstructed as the exact path required for an argv plan.
    RetroArchPathNotExact,
    /// A Dolphin command-plan request was given a non-Dolphin-standalone
    /// launch candidate.
    DolphinCandidateRequired,
    /// The canonical identity this Dolphin plan was built for does not
    /// target `GameCube` - the only platform this native launch slice
    /// supports.
    DolphinPlatformMismatch,
    /// No verified Dolphin Game ID is available for the selected GameCube/Wii
    /// content.
    DolphinGameIdMissing,
    /// The content is not a direct, non-mounted `.iso`/`.gcm` file - no
    /// archive member, no RVZ/CISO/WBFS, no mount-input container.
    DolphinContentFormatUnsupported,
    /// [`crate::patch_manager::resolve_dolphin_native_launch_binding`]
    /// itself refused to produce a launch binding for the candidate's
    /// profile - see the blocker detail for the underlying
    /// [`crate::patch_manager::DolphinLaunchBlockerKind`].
    DolphinBindingUnavailable,
    /// A PCSX2 command-plan request was given a non-PCSX2-standalone launch
    /// candidate.
    Pcsx2CandidateRequired,
    /// The canonical identity this PCSX2 plan was built for does not target
    /// `PS2` - the only platform this native launch slice supports.
    Pcsx2PlatformMismatch,
    /// The content is not a direct, non-mounted `.iso` file - no archive
    /// member, no CHD, no mount-input container.
    Pcsx2ContentFormatUnsupported,
    /// No verified PS2 serial is available for this content - this native
    /// launch slice requires one even though a resolved identity could in
    /// principle rest on an executable CRC alone.
    Pcsx2SerialMissing,
    /// [`crate::patch_manager::resolve_pcsx2_native_launch_binding`] itself
    /// refused to produce a launch binding for the candidate's profile -
    /// see the blocker detail for the underlying
    /// [`crate::patch_manager::Pcsx2LaunchBlockerKind`].
    Pcsx2BindingUnavailable,
    /// A DuckStation command-plan request was given a non-DuckStation-
    /// standalone launch candidate.
    DuckStationCandidateRequired,
    /// The canonical identity this DuckStation plan was built for does not
    /// target `PSX` - the only platform this native launch slice supports.
    DuckStationPlatformMismatch,
    /// The content is not a direct, non-mounted `.iso`/`.chd` file - no
    /// archive member, no mount-input container, no `.cue`/`.bin` or other
    /// format the current archive-kind registry does not yet classify as
    /// direct content.
    DuckStationContentFormatUnsupported,
    /// No verified PS1 serial is available for this content - this native
    /// launch slice always requires one.
    DuckStationSerialMissing,
    /// [`crate::patch_manager::resolve_duckstation_native_launch_binding`]
    /// itself refused to produce a launch binding for the candidate's
    /// profile - see the blocker detail for the underlying
    /// [`crate::patch_manager::DuckStationLaunchBlockerKind`].
    DuckStationBindingUnavailable,
    /// A command-plan request was given a non-RPCS3-standalone launch
    /// candidate.
    Rpcs3CandidateRequired,
    /// The canonical identity this RPCS3 command plan was built for does not
    /// target `PS3`.
    Rpcs3PlatformMismatch,
    /// The content is not a direct, resolved PS3 path supported by the
    /// native RPCS3 command contract.
    Rpcs3ContentFormatUnsupported,
    /// No verified PS3 title ID is available for this content.
    Rpcs3TitleIdMissing,
    /// RPCS3 firmware location/readiness could not be established.
    Rpcs3FirmwareUnavailable,
    /// The RPCS3 profile/executable binding could not be safely resolved.
    Rpcs3BindingUnavailable,
    /// A command-plan request was given a non-PPSSPP-standalone launch
    /// candidate.
    PpssppCandidateRequired,
    /// The canonical identity does not target PSP.
    PpssppPlatformMismatch,
    /// The content is not a direct, non-mounted PSP ISO.
    PpssppContentFormatUnsupported,
    /// No verified PSP disc ID is available for this content.
    PpssppDiscIdMissing,
    /// The PPSSPP profile/executable binding could not be safely resolved.
    PpssppBindingUnavailable,
    /// A command-plan request was given a non-ScummVM launch target.
    ScummVmCandidateRequired,
    /// The canonical identity does not target ScummVM.
    ScummVmPlatformMismatch,
    /// The supplied content is not a safe extracted ScummVM game folder.
    ScummVmContentUnsupported,
    /// No verified ScummVM engine/game ID is available.
    ScummVmGameIdMissing,
    /// The ScummVM executable/profile binding could not be safely resolved.
    ScummVmBindingUnavailable,
    /// A Flycast command-plan request was given a non-Flycast-standalone
    /// launch candidate.
    FlycastCandidateRequired,
    /// The canonical identity this Flycast plan was built for does not
    /// target `Dreamcast` - the only platform this native launch slice
    /// supports.
    FlycastPlatformMismatch,
    /// The content is not a direct, non-mounted `.iso`/`.cue`/`.chd` file -
    /// no archive member, no mount-input container, no GDI/CDI/multi-track
    /// GD-ROM (deliberately out of scope for this native launch slice).
    FlycastContentFormatUnsupported,
    /// No verified Dreamcast product code is available for this content -
    /// this native launch slice always requires one.
    FlycastProductCodeMissing,
    /// [`crate::patch_manager::resolve_flycast_native_launch_binding`]
    /// itself refused to produce a launch binding for the candidate's
    /// profile - see the blocker detail for the underlying
    /// [`crate::patch_manager::FlycastLaunchBlockerKind`].
    FlycastBindingUnavailable,
    /// A melonDS command-plan request was given a non-melonDS candidate.
    MelonDsCandidateRequired,
    /// The canonical identity does not target Nintendo DS.
    MelonDsPlatformMismatch,
    /// The content is not a direct `.nds` file in the Phase 1 adapter.
    MelonDsContentFormatUnsupported,
    /// No verified upstream Nintendo DS identity key is available.
    MelonDsGameKeyMissing,
    /// The melonDS profile/executable binding could not be safely resolved.
    MelonDsBindingUnavailable,
    /// A command-plan request was given a non-mGBA candidate.
    MgbaCandidateRequired,
    /// The canonical identity does not target a supported mGBA platform.
    MgbaPlatformMismatch,
    /// The content is not a direct extension matching the resolved platform.
    MgbaContentFormatUnsupported,
    /// The mGBA profile/executable binding could not be safely resolved.
    MgbaBindingUnavailable,
    /// A Vita3K command-plan request was given a non-Vita3K candidate.
    Vita3kCandidateRequired,
    /// The canonical identity does not target PlayStation Vita.
    Vita3kPlatformMismatch,
    /// The selected content is not an installed Vita title.
    Vita3kContentUnsupported,
    /// No trusted Vita title ID is available.
    Vita3kTitleIdMissing,
    /// The Vita3K profile/executable binding is unavailable.
    Vita3kBindingUnavailable,
    /// The exact Vita title is not installed.
    Vita3kInstalledTitleMissing,
    /// Vita3K firmware evidence is insufficient.
    Vita3kFirmwareUnavailable,
    /// Vita license evidence is insufficient for a title that needs it.
    Vita3kLicenseUnavailable,
    /// A FS-UAE command-plan request was given a non-FS-UAE candidate.
    FsUaeCandidateRequired,
    /// The canonical identity does not target the ordinary Amiga platform.
    FsUaePlatformMismatch,
    /// The selected content is not a directly supported FS-UAE image.
    FsUaeContentFormatUnsupported,
    /// No verified Amiga identity is available.
    FsUaeIdentityMissing,
    /// FS-UAE requires a profile/configuration for this launch.
    FsUaeProfileRequired,
    /// The FS-UAE profile/executable binding could not be resolved.
    FsUaeBindingUnavailable,
    /// FS-UAE firmware/Kickstart evidence is not sufficient.
    FsUaeKickstartUnavailable,
    /// IPF/CAPS content has no explicitly available backend evidence.
    FsUaeIpfBackendUnavailable,
    /// A Xenia command-plan request was given a non-Xenia-standalone launch
    /// candidate.
    XeniaCandidateRequired,
    /// The canonical identity this Xenia plan was built for does not target
    /// `Xbox360` - the only platform this native launch slice supports.
    XeniaPlatformMismatch,
    /// The content is not a direct, non-mounted `.xex` file - no ZIP member
    /// (even though [`crate::game_identity::inspect_zip_xex`] can verify one),
    /// no other archive/mount-input container, and no ISO (Xbox 360 disc
    /// image parsing is not modeled by this build at all).
    XeniaContentFormatUnsupported,
    /// Neither a verified XEX title ID nor a verified XEX media ID is
    /// available for this content - this native launch slice always
    /// requires at least one, exactly matching what
    /// [`crate::launch::evidence_bridge`] itself requires to resolve Xbox
    /// 360 identity at all.
    XeniaTitleIdMissing,
    /// [`crate::patch_manager::resolve_xenia_launch_binding`] itself refused
    /// to produce a launch binding for the candidate's profile - see the
    /// blocker detail for the underlying
    /// [`crate::patch_manager::XeniaLaunchBlockerKind`].
    XeniaBindingUnavailable,
    /// A command-plan request was given a non-xemu-standalone launch
    /// candidate.
    XemuCandidateRequired,
    /// The canonical identity this xemu command plan was built for does not
    /// target `Xbox` - the only platform this native launch slice supports.
    /// Xbox 360/XEX identity must never reach this arm - see
    /// [`crate::launch::xemu_command`]'s own doc comment.
    XemuPlatformMismatch,
    /// The content is not a direct, non-mounted, non-archived `.iso`/`.xiso`
    /// disc image. A loose `.xbe` (even though xemu cannot boot one
    /// directly - see the module doc) and a ZIP-contained `.xbe` (even
    /// though it is genuinely verifiable identity) are both refused here.
    XemuContentFormatUnsupported,
    /// No verified original-Xbox XBE title ID is available for this
    /// content.
    XemuTitleIdMissing,
    /// [`crate::patch_manager::resolve_xemu_native_launch_binding`] itself
    /// refused to produce a launch binding for the candidate's profile -
    /// see the blocker detail for the underlying
    /// [`crate::patch_manager::XemuLaunchBlockerKind`].
    XemuBindingUnavailable,
    /// A DOSBox command-plan request was given a non-DOSBox launch target.
    DosBoxCandidateRequired,
    /// The canonical identity this DOSBox plan was built for does not target
    /// `DOS` - the only platform this native launch slice supports. A bare
    /// Weak MZ signature or an ambiguous/unknown platform never reaches a
    /// resolved `DOS` identity, so it fails closed here.
    DosBoxPlatformMismatch,
    /// The supplied launch input is not a safe absolute DOS game directory.
    DosBoxContentUnsupported,
    /// No `dosbox.conf` was found in the game directory where the DOS layout
    /// rule expects one.
    DosBoxConfigMissing,
    /// A `dosbox.conf` is present but its contents did not parse as a DOSBox
    /// configuration (empty / binary / malformed section / non-UTF-8 / too
    /// large - see [`crate::dosbox_config_evidence`]).
    DosBoxConfigMalformed,
    /// A structurally valid DOSBox config was found, but it carries no
    /// `[autoexec]` section, so there is no reviewed configuration to launch
    /// with. A file name alone is never sufficient.
    DosBoxConfigNoAutoexec,
    /// A compatible DOSBox executable could not be discovered on this
    /// machine (nothing on `PATH` or the standard locations, or the only
    /// match was a symlink / non-regular / non-executable file).
    DosBoxBindingUnavailable,
    /// A DOSBox variant was requested that this build does not support
    /// (only classic DOSBox and DOSBox Staging are modeled; DOSBox-X and
    /// anything else fail closed).
    DosBoxVariantUnsupported,
    /// A native Amiberry command was given a non-Amiga identity.
    AmiberryPlatformMismatch,
    /// The selected Amiberry launch candidate is not a standalone Amiberry target.
    AmiberryCandidateRequired,
    /// Amiberry was not discovered or its selected profile is ineligible.
    AmiberryBindingUnavailable,
    /// Amiberry requires explicit, readable machine configuration for this slice.
    AmiberryProfileRequired,
    /// The selected Amiga media format is outside this native Phase 1 slice.
    AmiberryContentFormatUnsupported,
    /// The selected media is not explicitly represented by the Amiberry profile.
    AmiberryMediaNotConfigured,
    /// Machine model evidence is absent or conflicting.
    AmiberryMachineAmbiguous,
    /// Kickstart evidence is absent, unusable, or only weakly known.
    AmiberryKickstartUnavailable,
    /// IPF requires CAPS/SPS support that was not proven available.
    AmiberryIpfBackendUnavailable,
    /// A MAME launch requires a resolved Arcade platform.
    MamePlatformMismatch,
    /// No unique DAT-backed MAME set identity is available.
    MameSetIdentityUnavailable,
    /// More than one DAT-backed MAME set identity remains possible.
    MameSetIdentityAmbiguous,
    /// The selected MAME set is not storage-complete.
    MameSetIncomplete,
    /// The selected set's dependency resolver reports a blocking state.
    MameDependencyBlocked,
    /// Set/dependency evidence was not trustworthy enough to launch.
    MameSetVerdictUnavailable,
    /// No executable binding was discovered for MAME.
    MameEmulatorUnavailable,
    /// The configured MAME ROM/search-path arrangement is not explicit.
    MameSearchPathUnconfigured,
    /// The requested launch arrangement is outside this native MAME slice.
    MameLaunchArrangementUnsupported,
    /// A native FBNeo launch requires a resolved Arcade platform.
    FbneoPlatformMismatch,
    /// No explicit FBNeo DAT-backed driver identity is available.
    FbneoIdentityUnavailable,
    /// More than one FBNeo driver identity remains possible.
    FbneoIdentityAmbiguous,
    /// The selected FBNeo set is not storage-complete.
    FbneoSetIncomplete,
    /// The selected FBNeo set has a blocking dependency verdict.
    FbneoDependencyBlocked,
    /// FBNeo-specific compatibility evidence is absent or not trustworthy.
    FbneoCompatibilityUnavailable,
    /// No explicit standalone FBNeo executable binding is available.
    FbneoEmulatorUnavailable,
    /// The selected FBNeo archive/content is missing or has drifted.
    FbneoContentUnavailable,
    HatariCandidateRequired,
    HatariPlatformMismatch,
    HatariProfileUnavailable,
    HatariEmulatorUnavailable,
    HatariBindingAmbiguous,
    HatariMachineMismatch,
    HatariTosMissing,
    HatariTosUnavailable,
    HatariContentFormatUnsupported,
    HatariIpfBackendUnavailable,
    HatariDiskDriveInvalid,
    HatariContentUnavailable,
    /// A Cemu command-plan request found no eligible discovered profile, or
    /// [`crate::patch_manager::resolve_cemu_native_launch_binding`] itself
    /// refused to produce a binding - see the blocker detail for the
    /// underlying [`crate::patch_manager::CemuLaunchBlockerKind`].
    CemuBindingUnavailable,
    /// The selected content's shape is not [`crate::patch_manager::CemuContentForm::ExtractedTitle`]
    /// - the only form this build launches - or is not recognised as a Wii
    /// U content shape at all (an arbitrary folder, an unrelated file).
    CemuContentFormatUnsupported,
    /// The selected extracted-title directory does not have the expected
    /// `code`/`content`/`meta` layout, or `code/` does not contain exactly
    /// one `.rpx` file - see the blocker detail for the underlying
    /// [`crate::patch_manager::CemuLayoutErrorKind`].
    CemuLayoutInvalid,
    /// The selected content's own `meta.xml` declares a title ID that does
    /// not classify as [`crate::patch_manager::CemuTitleKind::Base`] - this
    /// build only launches a selected base game, never an update or DLC
    /// title directly (see the module doc comment on
    /// [`crate::launch::cemu_command`]).
    CemuNotABaseTitle,
    /// This content form requires `keys.txt` and none was found configured
    /// for the selected profile.
    CemuKeysRequired,
    /// Cemu's configured MLC path (`<mlc_path>` in `settings.xml`) is
    /// unconfigured, missing, or not a directory.
    CemuMlcUnavailable,
    /// The content, executable, or configuration changed between planning
    /// and the final pre-spawn recheck.
    CemuDriftBeforeSpawn,
    SameBoyPlatformMismatch,
    SameBoyRomHeaderInvalid,
    SameBoyIdentityConflict,
    SameBoyBindingUnavailable,
    SameBoyDriftBeforeSpawn,
}

/// One blocking condition on a [`crate::launch::planning::LaunchCandidate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchBlocker {
    pub kind: LaunchBlockerKind,
    pub detail: String,
}

impl LaunchBlocker {
    pub fn new(kind: LaunchBlockerKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

/// A non-blocking condition worth surfacing on an otherwise-launchable
/// candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchWarningKind {
    /// Optional firmware ([`crate::emulator_environment::retroarch::FirmwareRequirement::optional`])
    /// is missing - the core/adapter itself declared it non-essential.
    OptionalFirmwareMissing,
    /// [`FirmwareReadiness::PresentUnverified`] for a target that requires
    /// firmware.
    FirmwarePresentUnverified,
    /// More than one eligible profile/core exists for this platform and
    /// none is remembered/preferred - still launchable, just ambiguous
    /// which one is "the" one.
    MultipleEligibleProfiles,
    /// The selected Cemu extracted title has no `meta.xml`, or `meta.xml`
    /// could not be read - the layout is still launched from its `.rpx`,
    /// but with no self-reported title identity to show.
    CemuTitleIdentityUnavailable,
    /// A `keys.txt` was found for this Cemu profile, but its contents were
    /// never inspected (and never will be) - see
    /// [`crate::patch_manager::CemuKeysState::PresentUnverified`].
    CemuKeysPresentUnverified,
    SameBoyRomHeaderChecksumInvalid,
    SameBoyCustomBootRomUnavailable,
}

/// One non-blocking condition on a [`crate::launch::planning::LaunchCandidate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchWarning {
    pub kind: LaunchWarningKind,
    pub detail: String,
}

impl LaunchWarning {
    pub fn new(kind: LaunchWarningKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

/// Projects [`DuckStationBiosState`] onto [`FirmwareReadiness`]. `Verified`
/// stays `Verified` (produced only by
/// `patch_manager::duckstation_firmware::resolve_duckstation_bios`'s real
/// Redump-backed hash verification, never by this projection); `Unknown`
/// is honest uncertainty, not a proven absence, so it never becomes
/// `Missing`.
pub fn duckstation_firmware_readiness(state: DuckStationBiosState) -> FirmwareReadiness {
    match state {
        DuckStationBiosState::Verified => FirmwareReadiness::Verified,
        DuckStationBiosState::PresentUnverified => FirmwareReadiness::PresentUnverified,
        DuckStationBiosState::Missing => FirmwareReadiness::Missing,
        DuckStationBiosState::Unknown => FirmwareReadiness::Unknown,
    }
}

/// Projects [`Pcsx2BiosVerification`] onto [`FirmwareReadiness`]. `Verified`
/// stays `Verified` (PCSX2's own enum reserves that variant for a real
/// future hash verifier - see its doc comment); `Unreadable` is honest
/// uncertainty, not a proven absence, so it becomes `Unknown` rather than
/// `Missing`.
pub fn pcsx2_firmware_readiness(state: Pcsx2BiosVerification) -> FirmwareReadiness {
    match state {
        Pcsx2BiosVerification::Verified => FirmwareReadiness::Verified,
        Pcsx2BiosVerification::PresentUnverified => FirmwareReadiness::PresentUnverified,
        Pcsx2BiosVerification::Missing => FirmwareReadiness::Missing,
        Pcsx2BiosVerification::Unreadable => FirmwareReadiness::Unknown,
    }
}

/// Projects [`PceCdFirmwareReadiness`] onto [`FirmwareReadiness`].
///
/// `VerifiedSufficient` -> `Verified` (produced only by
/// `patch_manager::pcengine_cd_firmware`'s real MAME + Mednafen dual-sourced
/// hash match, never by this projection). `VerifiedButRequirementUnknown`
/// and `EmulatorRequirementUnknown` -> `Unknown`: honest uncertainty, never
/// a proven pass or a proven failure, because this build has no per-title
/// PC Engine CD compatibility table. `NoVerifiedFirmware` -> `Missing` only
/// when the selected emulator is known to load an external System Card.
pub fn pcengine_cd_firmware_readiness(status: PceCdFirmwareReadiness) -> FirmwareReadiness {
    match status {
        PceCdFirmwareReadiness::EmulatorProvidesFirmware => FirmwareReadiness::NotRequired,
        PceCdFirmwareReadiness::VerifiedSufficient { .. } => FirmwareReadiness::Verified,
        PceCdFirmwareReadiness::CandidatePresentHashUnknown => FirmwareReadiness::PresentUnverified,
        PceCdFirmwareReadiness::NoVerifiedFirmware => FirmwareReadiness::Missing,
        PceCdFirmwareReadiness::VerifiedButRequirementUnknown { .. }
        | PceCdFirmwareReadiness::EmulatorRequirementUnknown => FirmwareReadiness::Unknown,
    }
}

/// Projects [`Rpcs3FirmwareStatus`] onto [`FirmwareReadiness`]. RPCS3's own
/// inspector is a presence check only (a small set of files RPCS3 itself
/// writes once firmware is installed) - it never verifies firmware
/// authenticity, so `Present(_)` (even with a version string attached) can
/// only ever mean `PresentUnverified`, never `Verified`.
pub fn rpcs3_firmware_readiness(status: &Rpcs3FirmwareStatus) -> FirmwareReadiness {
    match status {
        Rpcs3FirmwareStatus::Present(_) => FirmwareReadiness::PresentUnverified,
        Rpcs3FirmwareStatus::Missing => FirmwareReadiness::Missing,
        Rpcs3FirmwareStatus::Unknown => FirmwareReadiness::Unknown,
    }
}

/// Projects [`XemuSystemFileState`] onto [`FirmwareReadiness`]. xemu never
/// verifies its MCPX boot ROM/flash BIOS contents - `Present` only proves a
/// file exists at the configured path, so it maps to `PresentUnverified`,
/// never `Verified`.
pub fn xemu_firmware_readiness(state: XemuSystemFileState) -> FirmwareReadiness {
    match state {
        XemuSystemFileState::Present => FirmwareReadiness::PresentUnverified,
        XemuSystemFileState::Missing => FirmwareReadiness::Missing,
        XemuSystemFileState::Unreadable
        | XemuSystemFileState::NotConfigured
        | XemuSystemFileState::Unknown => FirmwareReadiness::Unknown,
    }
}

/// Projects [`FlycastSystemFileState`] onto [`FirmwareReadiness`]. Unknown or
/// unreadable Flycast system files are deliberately treated like
/// `PresentUnverified`: neither state may be more launchable than a file whose
/// contents are known not to match the trusted firmware record. Only a real
/// hash match reaches strict `Verified`.
pub fn flycast_firmware_readiness(state: FlycastSystemFileState) -> FirmwareReadiness {
    match state {
        FlycastSystemFileState::Verified => FirmwareReadiness::Verified,
        FlycastSystemFileState::PresentUnverified => FirmwareReadiness::PresentUnverified,
        FlycastSystemFileState::Missing => FirmwareReadiness::Missing,
        FlycastSystemFileState::Unreadable
        | FlycastSystemFileState::NotConfigured
        | FlycastSystemFileState::Unknown => FirmwareReadiness::PresentUnverified,
    }
}

/// Projects [`HatariTosHealth`] onto [`FirmwareReadiness`]. Hatari's own
/// `Verified` variant means what it says - a real TOS ROM hash match - and
/// stays `Verified` through this projection, matching Phase 1's explicit
/// requirement that Hatari's verified TOS state is never downgraded.
pub fn hatari_firmware_readiness(state: HatariTosHealth) -> FirmwareReadiness {
    match state {
        HatariTosHealth::Verified => FirmwareReadiness::Verified,
        HatariTosHealth::PresentUnverified => FirmwareReadiness::PresentUnverified,
        HatariTosHealth::Missing => FirmwareReadiness::Missing,
        HatariTosHealth::NotConfigured | HatariTosHealth::Unreadable => FirmwareReadiness::Unknown,
    }
}

/// PPSSPP has no BIOS/firmware requirement - a real PSP UMD/PBP image needs
/// no separate firmware file the way PS1/PS2/Xbox/Dreamcast do. This is a
/// constant, not a projection from any PPSSPP adapter state, because no
/// such state exists to project from - inventing one here would itself be
/// exactly the mistake this function's existence prevents.
pub fn ppsspp_firmware_readiness() -> FirmwareReadiness {
    FirmwareReadiness::NotRequired
}

/// Xenia has no BIOS/firmware requirement - it emulates the Xbox 360 kernel
/// directly and needs no separate console firmware dump the way PS1/PS2/
/// (original) Xbox/Dreamcast do. A constant, not a projection from any
/// Xenia adapter state, for exactly the same reason [`ppsspp_firmware_readiness`]
/// is one: no such state exists to project from.
pub fn xenia_firmware_readiness() -> FirmwareReadiness {
    FirmwareReadiness::NotRequired
}

/// Whether a RetroArch core's own `.info` metadata declares a required
/// (non-optional) `firmwareN_*` entry. This never checks whether the
/// declared file actually exists on disk (that would be filesystem I/O the
/// pure planner must not perform) - it only reads metadata the core's
/// `.info` file already declared, exactly as
/// [`crate::emulator_environment::retroarch`] already parsed it. A core
/// that declares no firmware requirement, or whose `.info` could not be
/// read at all, answers `NotRequired`/`Unknown` respectively rather than a
/// guess.
pub fn retroarch_core_firmware_readiness(info: &CoreInfoFinding) -> FirmwareReadiness {
    match info {
        CoreInfoFinding::Found { firmware, .. } => {
            if firmware.iter().any(|entry| !entry.optional) {
                FirmwareReadiness::Unknown
            } else {
                FirmwareReadiness::NotRequired
            }
        }
        _ => FirmwareReadiness::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckstation_projection_matches_every_state() {
        assert_eq!(
            duckstation_firmware_readiness(DuckStationBiosState::Verified),
            FirmwareReadiness::Verified
        );
        assert_eq!(
            duckstation_firmware_readiness(DuckStationBiosState::PresentUnverified),
            FirmwareReadiness::PresentUnverified
        );
        assert_eq!(
            duckstation_firmware_readiness(DuckStationBiosState::Missing),
            FirmwareReadiness::Missing
        );
        assert_eq!(
            duckstation_firmware_readiness(DuckStationBiosState::Unknown),
            FirmwareReadiness::Unknown
        );
    }

    #[test]
    fn pcsx2_verified_stays_verified() {
        assert_eq!(
            pcsx2_firmware_readiness(Pcsx2BiosVerification::Verified),
            FirmwareReadiness::Verified
        );
        assert_eq!(
            pcsx2_firmware_readiness(Pcsx2BiosVerification::Unreadable),
            FirmwareReadiness::Unknown
        );
    }

    #[test]
    fn rpcs3_present_is_never_verified() {
        assert_eq!(
            rpcs3_firmware_readiness(&Rpcs3FirmwareStatus::Present(Some("4.91".to_string()))),
            FirmwareReadiness::PresentUnverified
        );
        assert_eq!(
            rpcs3_firmware_readiness(&Rpcs3FirmwareStatus::Missing),
            FirmwareReadiness::Missing
        );
        assert_eq!(
            rpcs3_firmware_readiness(&Rpcs3FirmwareStatus::Unknown),
            FirmwareReadiness::Unknown
        );
    }

    #[test]
    fn xemu_projection_maps_uncertain_states_to_unknown() {
        assert_eq!(
            xemu_firmware_readiness(XemuSystemFileState::Present),
            FirmwareReadiness::PresentUnverified
        );
        assert_eq!(
            xemu_firmware_readiness(XemuSystemFileState::NotConfigured),
            FirmwareReadiness::Unknown
        );
        assert_eq!(
            xemu_firmware_readiness(XemuSystemFileState::Unreadable),
            FirmwareReadiness::Unknown
        );
    }

    #[test]
    fn flycast_verified_and_uncertain_states_are_distinct() {
        assert_eq!(
            flycast_firmware_readiness(FlycastSystemFileState::Verified),
            FirmwareReadiness::Verified
        );
        assert_eq!(
            flycast_firmware_readiness(FlycastSystemFileState::PresentUnverified),
            FirmwareReadiness::PresentUnverified
        );
        assert_eq!(
            flycast_firmware_readiness(FlycastSystemFileState::Unknown),
            FirmwareReadiness::PresentUnverified
        );
    }

    #[test]
    fn hatari_verified_tos_stays_verified() {
        assert_eq!(
            hatari_firmware_readiness(HatariTosHealth::Verified),
            FirmwareReadiness::Verified
        );
        assert_eq!(
            hatari_firmware_readiness(HatariTosHealth::PresentUnverified),
            FirmwareReadiness::PresentUnverified
        );
        assert_eq!(
            hatari_firmware_readiness(HatariTosHealth::NotConfigured),
            FirmwareReadiness::Unknown
        );
    }

    #[test]
    fn ppsspp_never_requires_firmware() {
        assert_eq!(ppsspp_firmware_readiness(), FirmwareReadiness::NotRequired);
    }
}
