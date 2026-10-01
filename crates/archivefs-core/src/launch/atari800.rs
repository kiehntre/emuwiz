//! Explicit, disposable Linux Atari800 floppy sessions. No identity inference,
//! live-config import, native GUI registration, or persistent-save routing.
//!
//! Classification: VERIFIED LAUNCHABLE (Atari800 5.0.0 smoke-tested). See
//! docs/research/NATIVE_ADAPTERS_FIVE_PLATFORM_RESCUE.md for the evidence:
//! the emulator reads the explicit `-config`, reads its ROMs from scratch, and
//! opens ONLY the scratch disk copy read-write; HOME stays empty.

#[cfg(test)]
mod tests;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use crate::identity_source::model::LocalEvidenceStrength;

use super::native_support::{
    ExecutableBinding, ExecutableDiscovery, ExecutableRefusal, member, read_prefix, seed_is_exact,
};
use super::planning::CanonicalIdentityStatus;
use super::process_spawn::PreparedProcessCommand;
use super::readiness::{FirmwareReadiness, LaunchReadiness};
use super::safe_launch_sandbox::{
    ConfigIsolation, LaunchMediaSafety, LaunchMediaSafetyDeclaration, MediaKind, MediaRole,
    PersistentStatePolicy, PreparedSandbox, ProfileSeed, SafeLaunchSandboxError, SandboxManager,
    SandboxPlan, SandboxedProcess, SourceProvenance, plan_sandbox,
};

pub const ADAPTER_ID: &str = "atari800";
pub const DISPLAY_NAME: &str = "Atari800";
pub const CLASSIFICATION: super::native_support::NativeAdapterClassification =
    super::native_support::NativeAdapterClassification::VerifiedLaunchable;
pub const PLATFORM_ID: &str = "Atari 8-bit";
pub const DISPOSABLE_SESSION_WARNING: &str = "Temporary Atari800 session: disk changes and new saves are discarded. Existing saves are not imported. Persistent-save sessions are not supported.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atari800Machine {
    Atari400800,
    Atari800Xl,
    Atari130Xe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atari800Video {
    Pal,
    Ntsc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atari800PreparationState {
    NeedsScratchPreparation,
    Prepared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atari800RomKind {
    Os400800,
    OsXlXe,
    Basic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atari800Rom {
    pub path: PathBuf,
    /// Explicit user/profile binding, not inferred from the filename or size.
    pub kind: Atari800RomKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atari800Profile {
    pub id: String,
    pub executable: PathBuf,
    pub machine: Atari800Machine,
    pub video: Atari800Video,
    pub os: Atari800Rom,
    /// None means BASIC explicitly disabled, not automatically discovered.
    pub basic: Option<Atari800Rom>,
    /// Must contain EXACTLY isolated_config_seed(machine, basic.is_some()).
    /// A normal user Atari800 config is deliberately not an eligible seed.
    pub isolated_seed: PathBuf,
    /// V1 checks role/size and binds hashes, but has no firmware trust catalogue.
    /// A file is never called Verified simply because its hash was computed.
    pub accept_present_unverified_roms: bool,
    /// Explicit consent to the warning above, never defaulted on by a planner.
    pub disposable_session_acknowledged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atari800MediaFormat {
    Atr,
    Xfd,
    Atx,
    Cassette,
    Program,
    Cartridge,
    HardDisk,
    ReferenceManifest,
    Unknown,
}

impl Atari800MediaFormat {
    pub fn safety(self) -> LaunchMediaSafetyDeclaration {
        LaunchMediaSafetyDeclaration {
            safety: match self {
                Self::Atr | Self::Xfd => LaunchMediaSafety::ScratchCopyWithIsolatedConfig,
                _ => LaunchMediaSafety::UnsafeUnsupported,
            },
            config_isolation: ConfigIsolation::Combined { flag: "-config" },
            persistent_state: PersistentStatePolicy::DisposableSession,
        }
    }
    fn suffix(self) -> Result<&'static str, Atari800Error> {
        match self {
            Self::Atr => Ok("atr"),
            Self::Xfd => Ok("xfd"),
            Self::Atx => Err(Atari800Error::UnsupportedMedia(
                "ATX copy is byte-preserving, but V1 lacks a bounded ATX validation/attachment contract",
            )),
            _ => Err(Atari800Error::UnsupportedMedia(
                "V1 accepts only small, independently verified ATR/XFD floppies; no HDD, manifest, state, cartridge, program or tape launch",
            )),
        }
    }
}

/// Downstream-only identity input, following the shared launch input contract.
/// The caller supplies an independently verified structural/DAT platform plus
/// the SHA-256 of the SAME inspected file. A hash computed from an arbitrary
/// .xfd plus its extension is NOT verified platform evidence. XFD currently
/// requires external verified evidence; this adapter never resolves identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atari800Media {
    pub path: PathBuf,
    pub format: Atari800MediaFormat,
    pub identity: CanonicalIdentityStatus,
    pub platform_evidence: LocalEvidenceStrength,
    pub verified_source_sha256: [u8; 32],
}

#[derive(Debug)]
pub enum Atari800Error {
    ExecutableMissing,
    ExecutableUnsafe,
    IdentityNotVerified,
    UnsupportedMedia(&'static str),
    FirmwareMissing,
    FirmwareIncompatible,
    FirmwareNeedsAcknowledgement,
    IsolatedProfileRequired,
    DisposableSessionRequired,
    ProfileChanged,
    Sandbox(SafeLaunchSandboxError),
    Io(std::io::Error),
}

impl std::fmt::Display for Atari800Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::ExecutableMissing => {
                "Atari800 executable is missing; select a native installation"
            }
            Self::ExecutableUnsafe => {
                "Atari800 requires an unchanged regular executable with execute permission"
            }
            Self::IdentityNotVerified => {
                "A fresh, resolved Atari 8-bit platform and matching verified source hash are required; extension/geometry alone is insufficient"
            }
            Self::UnsupportedMedia(reason) => reason,
            Self::FirmwareMissing => "Required Atari800 system ROM is missing",
            Self::FirmwareIncompatible => {
                "ROM role or size is incompatible with the explicit Atari machine/BASIC profile"
            }
            Self::FirmwareNeedsAcknowledgement => {
                "System ROMs are present but unverified; explicit acknowledgement is required"
            }
            Self::IsolatedProfileRequired => {
                "An exact EmuWiz isolated Atari800 seed is required; live/user config imports are refused"
            }
            Self::DisposableSessionRequired => DISPOSABLE_SESSION_WARNING,
            Self::ProfileChanged => {
                "Atari800 profile or selected identity changed since planning; make a new plan"
            }
            Self::Sandbox(error) => return write!(f, "Atari800 safe preparation refused: {error}"),
            Self::Io(error) => return write!(f, "Atari800 input unavailable: {error}"),
        };
        f.write_str(message)
    }
}
impl std::error::Error for Atari800Error {}
impl From<SafeLaunchSandboxError> for Atari800Error {
    fn from(error: SafeLaunchSandboxError) -> Self {
        Self::Sandbox(error)
    }
}
impl From<std::io::Error> for Atari800Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Produce seed CONTENT only. Caller explicitly stores it as a dedicated
/// profile asset; planning never writes a seed or changes user settings.
/// The seed must exist: upstream otherwise falls back to /etc/atari800.cfg.
pub fn isolated_config_seed(_machine: Atari800Machine, basic: bool) -> String {
    // Keep the seed free of path-bearing emulator settings. Atari800 resolves
    // some legacy directory values relative to CFG_data_dir (the config file),
    // while the typed command below supplies exact scratch ROM/media paths.
    format!(
        "EmuWiz Atari800 disposable profile V1\n\
         HD_READ_ONLY=1\n\
         ENABLE_H_PATCH=0\nENABLE_P_PATCH=0\nENABLE_R_PATCH=0\n\
         ENABLE_SIO_PATCH=1\nDISABLE_BASIC={}\n",
        if basic { 0 } else { 1 }
    )
}

/// Bounded discovery only; never invokes an executable. Multiple paths are
/// returned without choosing a winner or machine model. A `PATH` symlink is
/// reported with its eligible target, not followed (see `native_support`).
pub fn discover_executables(explicit: &[PathBuf], path_env: Option<&OsStr>) -> ExecutableDiscovery {
    super::native_support::discover_executables(explicit, path_env, &[ADAPTER_ID])
}

/// Upstream's read-only version option is `-v`. Output may be supplied by an
/// existing discovery worker; this planner does not execute a version probe.
pub fn parse_version(output: &str) -> Option<String> {
    if output.len() > 4096 {
        return None;
    }
    let value = output
        .trim()
        .strip_prefix("Atari 800 Emulator, Version ")?
        .split_whitespace()
        .next()?;
    (value.len() <= 32
        && value.split('.').count() >= 2
        && value
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))
    .then(|| value.to_owned())
}

fn executable_binding(path: &Path) -> Result<ExecutableBinding, Atari800Error> {
    ExecutableBinding::capture(path).map_err(|refusal| match refusal {
        ExecutableRefusal::Missing => Atari800Error::ExecutableMissing,
        ExecutableRefusal::Unsafe => Atari800Error::ExecutableUnsafe,
    })
}

fn prefix(source: &SourceProvenance, count: usize) -> Result<Vec<u8>, Atari800Error> {
    read_prefix(source, count).map_err(|error| {
        if error.kind() == std::io::ErrorKind::Other {
            Atari800Error::IdentityNotVerified
        } else {
            Atari800Error::Io(error)
        }
    })
}

fn validate_media(media: &Atari800Media, source: &SourceProvenance) -> Result<(), Atari800Error> {
    if media.platform_evidence != LocalEvidenceStrength::Verified
        || !matches!(&media.identity, CanonicalIdentityStatus::Resolved(id)
            if id.platform_id == PLATFORM_ID && !id.game_key.trim().is_empty())
        || media.verified_source_sha256 != source.sha256
    {
        return Err(Atari800Error::IdentityNotVerified);
    }
    let h = prefix(source, 16)?;
    let len = source.original_identity.size;
    // These are attachment guards for upstream AFILE/SIO, NOT a new platform
    // identity parser. Conservative conventional floppy sizes only (<=184336).
    let valid = match media.format {
        Atari800MediaFormat::Atr if h.len() == 16 && h[..2] == [0x96, 0x02] => {
            let paragraphs = u32::from_le_bytes([h[2], h[3], h[6], h[7]]) as u64;
            let sector = u16::from_le_bytes([h[4], h[5]]);
            paragraphs * 16 + 16 == len
                && match sector {
                    128 => matches!(len - 16, 92160 | 133120),
                    256 => matches!(len - 16, 183936 | 184320),
                    _ => false,
                }
        }
        Atari800MediaFormat::Xfd if h.len() == 16 => {
            matches!(len, 92160 | 133120 | 183936 | 184320) && !non_xfd_dispatch(&h, len)
        }
        _ => false,
    };
    if !valid {
        return Err(Atari800Error::UnsupportedMedia(
            "Not a supported small ATR/XFD floppy envelope, or Atari800 would dispatch these bytes as another file type",
        ));
    }
    Ok(())
}

// AFILE_DetectFileType tests these BEFORE its raw XFD size fallback. Refuse,
// in particular, a state/manifest disguised as .xfd (could reopen source paths).
fn non_xfd_dispatch(h: &[u8], len: u64) -> bool {
    (h[0] == 0 && h[1] == 0 && (h[2] != 0 || h[3] != 0))
        || h[..2] == [0x1f, 0x8b]
        || (h[0].is_ascii_digit() && (h[1].is_ascii_digit() || h[1] == b' '))
        || [b"ATAR", b"AT8X", b"CART", b"FUJI"]
            .iter()
            .any(|magic| h.starts_with(*magic))
        || h[..2] == [0x96, 0x02]
        || matches!(h[0], 0xf9 | 0xfa)
        || (h[..2] == [0xff, 0xff] && (h[2] != 0xff || h[3] != 0xff))
        || ((len - 16) % 140 == 0
            && u16::from_be_bytes([h[0], h[1]]) as u64 == (len - 16) / 140
            && h[2] == b'P')
}

fn check_rom(rom: &Atari800Rom, kind: Atari800RomKind, size: u64) -> Result<(), Atari800Error> {
    let meta = fs::symlink_metadata(&rom.path).map_err(|_| Atari800Error::FirmwareMissing)?;
    if rom.kind != kind || !meta.is_file() || meta.len() != size {
        return Err(Atari800Error::FirmwareIncompatible);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Atari800Plan {
    profile: Atari800Profile,
    media: Atari800Media,
    sandbox: SandboxPlan,
    /// Identity plus SHA-256 of the emulator binary, rechecked before spawn.
    executable_binding: ExecutableBinding,
}

impl Atari800Plan {
    pub fn preparation_state(&self) -> Atari800PreparationState {
        Atari800PreparationState::NeedsScratchPreparation
    }
    pub fn sandbox_plan(&self) -> &SandboxPlan {
        &self.sandbox
    }
    pub fn media(&self) -> &Atari800Media {
        &self.media
    }
    pub fn profile(&self) -> &Atari800Profile {
        &self.profile
    }
    pub fn firmware_readiness(&self) -> FirmwareReadiness {
        FirmwareReadiness::PresentUnverified
    }
    pub fn readiness(&self) -> LaunchReadiness {
        LaunchReadiness::ReadyWithWarnings
    }
    pub fn warnings(&self) -> [&'static str; 2] {
        [
            DISPOSABLE_SESSION_WARNING,
            "System ROM roles and sizes match; contents remain present/unverified",
        ]
    }

    fn fresh(&self, profile: &Atari800Profile, media: &Atari800Media) -> Result<(), Atari800Error> {
        if profile != &self.profile || media != &self.media {
            return Err(Atari800Error::ProfileChanged);
        }
        if executable_binding(&profile.executable)? != self.executable_binding {
            return Err(Atari800Error::ProfileChanged);
        }
        self.sandbox.revalidate_sources()?;
        let (kind, size) = match profile.machine {
            Atari800Machine::Atari400800 => (Atari800RomKind::Os400800, 10240),
            _ => (Atari800RomKind::OsXlXe, 16384),
        };
        check_rom(&profile.os, kind, size)?;
        if let Some(rom) = &profile.basic {
            check_rom(rom, Atari800RomKind::Basic, 8192)?;
        }
        // Independent identity binding and AFILE dispatch must still agree.
        validate_media(media, self.sandbox.sources().next().expect("primary"))?;
        self.sandbox.revalidate_sources()?;
        Ok(())
    }

    pub fn prepare(
        &self,
        manager: &SandboxManager,
        current_profile: &Atari800Profile,
        current_media: &Atari800Media,
    ) -> Result<PreparedAtari800, Atari800Error> {
        self.fresh(current_profile, current_media)?;
        let sandbox = manager.prepare(&self.sandbox)?;
        self.fresh(current_profile, current_media)?;
        let command = self.command(&sandbox)?;
        Ok(PreparedAtari800 {
            plan: self.clone(),
            sandbox,
            command,
        })
    }

    fn command(&self, sandbox: &PreparedSandbox) -> Result<PreparedProcessCommand, Atari800Error> {
        let media: Vec<_> = sandbox
            .mappings()
            .iter()
            .filter(|m| m.role != MediaRole::Config)
            .collect();
        let mut arguments = sandbox.config_arguments();
        let (machine, rom_flag, revision) = match self.profile.machine {
            Atari800Machine::Atari400800 => ("-atari", "-osb_rom", "-800-rev"),
            Atari800Machine::Atari800Xl => ("-xl", "-xlxe_rom", "-xl-rev"),
            Atari800Machine::Atari130Xe => ("-xe", "-xlxe_rom", "-xl-rev"),
        };
        arguments.extend(
            [
                "-no-autosave-config",
                machine,
                match self.profile.video {
                    Atari800Video::Pal => "-pal",
                    Atari800Video::Ntsc => "-ntsc",
                },
                revision,
                "custom",
                "-hreadonly",
                rom_flag,
            ]
            .map(OsString::from),
        );
        arguments.push(media[1].scratch_path.clone().into_os_string());
        if self.profile.basic.is_some() {
            arguments.push("-basic_rom".into());
            arguments.push(media[2].scratch_path.clone().into_os_string());
            arguments.extend(["-basic-rev", "custom", "-basic"].map(OsString::from));
        } else {
            arguments.push("-nobasic".into());
        }
        arguments.push(media[0].scratch_path.clone().into_os_string());
        Ok(PreparedProcessCommand {
            executable: self.profile.executable.clone(),
            arguments,
            working_directory: Some(sandbox.workspace_path().to_owned()),
        })
    }
}

/// Bounded read-only planning. Errors distinguish setup, identity and safety;
/// a successful plan still needs whole-set disk-space preflight/preparation.
pub fn plan_atari800(
    profile: &Atari800Profile,
    media: &Atari800Media,
) -> Result<Atari800Plan, Atari800Error> {
    let suffix = media.format.suffix()?;
    if !profile.disposable_session_acknowledged {
        return Err(Atari800Error::DisposableSessionRequired);
    }
    if profile.id.trim().is_empty() || profile.id.len() > 128 {
        return Err(Atari800Error::IsolatedProfileRequired);
    }
    let bound_executable = executable_binding(&profile.executable)?;
    let (kind, size) = match profile.machine {
        Atari800Machine::Atari400800 => (Atari800RomKind::Os400800, 10240),
        _ => (Atari800RomKind::OsXlXe, 16384),
    };
    check_rom(&profile.os, kind, size)?;
    if let Some(rom) = &profile.basic {
        check_rom(rom, Atari800RomKind::Basic, 8192)?;
    }
    if !profile.accept_present_unverified_roms {
        return Err(Atari800Error::FirmwareNeedsAcknowledgement);
    }
    let mut members = vec![
        member(
            &media.path,
            MediaRole::PrimaryMedia,
            MediaKind::Floppy,
            suffix,
        ),
        member(
            &profile.os.path,
            MediaRole::SecondaryMedia,
            MediaKind::Firmware,
            "rom",
        ),
    ];
    if let Some(rom) = &profile.basic {
        members.push(member(
            &rom.path,
            MediaRole::SecondaryMedia,
            MediaKind::Firmware,
            "rom",
        ));
    }
    let sandbox = plan_sandbox(
        media.format.safety(),
        &members,
        ProfileSeed::KnownProfile {
            source: profile.isolated_seed.clone(),
            scratch_relative: "atari800.cfg".into(),
        },
    )?;
    let sources: Vec<_> = sandbox.sources().collect();
    validate_media(media, sources[0])?;
    let seed = sources.last().expect("config seed");
    let expected = isolated_config_seed(profile.machine, profile.basic.is_some());
    if !seed_is_exact(seed, &expected)? {
        return Err(Atari800Error::IsolatedProfileRequired);
    }
    let plan = Atari800Plan {
        profile: profile.clone(),
        media: media.clone(),
        sandbox,
        executable_binding: bound_executable,
    };
    plan.fresh(profile, media)?;
    Ok(plan)
}

pub struct PreparedAtari800 {
    plan: Atari800Plan,
    sandbox: PreparedSandbox,
    command: PreparedProcessCommand,
}
impl PreparedAtari800 {
    pub fn preparation_state(&self) -> Atari800PreparationState {
        Atari800PreparationState::Prepared
    }
    pub fn command_preview(&self) -> &PreparedProcessCommand {
        &self.command
    }
    pub fn workspace_path(&self) -> &Path {
        self.sandbox.workspace_path()
    }
    pub fn original_plan(&self) -> &Atari800Plan {
        &self.plan
    }
    /// Caller supplies freshly selected identity/profile again; a stale GUI
    /// selection cannot silently execute a different game under this approval.
    pub fn spawn(
        self,
        current_profile: &Atari800Profile,
        current_media: &Atari800Media,
    ) -> Result<SandboxedProcess, Atari800Error> {
        self.plan.fresh(current_profile, current_media)?;
        Ok(self.sandbox.spawn(self.command)?)
    }
}
