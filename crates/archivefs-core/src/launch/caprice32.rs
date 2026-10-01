//! Preservation-safe Caprice32 (Amstrad CPC) launch adapter.
//!
//! Classification: VERIFIED LAUNCHABLE (Caprice32 4.6.0 built from source and
//! run headless). Evidence, from the emulator's own source and a real run, is
//! recorded in `docs/research/NATIVE_ADAPTERS_FIVE_PLATFORM_RESCUE.md`:
//!
//! * `--cfg_file` is searched FIRST, but an unreadable file silently falls back
//!   to `<exe dir>/cap32.cfg`, `$XDG_CONFIG_HOME/cap32.cfg`, `~/.cap32.cfg`.
//!   Planning and spawn both require the scratch config to exist.
//! * The config format is INI (`[system] model=<0..3>`), not dotted keys.
//! * Writable locations default to directories UNDER THE EXECUTABLE'S OWN
//!   DIRECTORY (`snap/`, `disk/`, `tape/`, `cart/`, `printer.dat`,
//!   `screenshots/`), not HOME/XDG, so HOME isolation alone would not contain
//!   them. The seed redirects every one to the workspace's `state/`, and the
//!   process runs with the workspace as its working directory.
//! * Firmware (`cpc464.rom`, `cpc664.rom`, `cpc6128.rom`, `amsdos.rom`) is read
//!   read-only from `<exe dir>/rom/`; it is hashed at planning and rechecked.
//!
//! This adapter consumes identity verified upstream. A DSK/CDT suffix is never
//! platform identity. Media is copied into an EmuWiz-owned disposable workspace
//! and Caprice32 receives only the scratch path plus the isolated config.

#[cfg(test)]
mod tests;

use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

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

pub const ADAPTER_ID: &str = "caprice32";
pub const DISPLAY_NAME: &str = "Caprice32";
pub const CLASSIFICATION: super::native_support::NativeAdapterClassification =
    super::native_support::NativeAdapterClassification::VerifiedLaunchable;
pub const PLATFORM_ID: &str = "Amstrad CPC";
pub const DISPOSABLE_SESSION_WARNING: &str = "Temporary Caprice32 session: disk and tape changes are discarded; persistent-save sessions are not supported.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caprice32Machine {
    Cpc464,
    Cpc664,
    Cpc6128,
}

impl Caprice32Machine {
    /// `[system] model=` as Caprice32 parses it (0..=3; 3 is the CPC6128+,
    /// which this adapter does not support).
    pub const fn model_value(self) -> u8 {
        match self {
            Self::Cpc464 => 0,
            Self::Cpc664 => 1,
            Self::Cpc6128 => 2,
        }
    }

    /// System ROM files Caprice32 loads for this model from `rom_path`.
    pub fn required_roms(self) -> &'static [&'static str] {
        match self {
            Self::Cpc464 => &["cpc464.rom"],
            Self::Cpc664 => &["cpc664.rom", "amsdos.rom"],
            Self::Cpc6128 => &["cpc6128.rom", "amsdos.rom"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caprice32MediaFormat {
    Dsk,
    Cdt,
    Ipf,
    Cpr,
    Sna,
    Unknown,
}

impl Caprice32MediaFormat {
    pub const fn safety(self) -> LaunchMediaSafetyDeclaration {
        LaunchMediaSafetyDeclaration {
            safety: match self {
                Self::Dsk | Self::Cdt => LaunchMediaSafety::ScratchCopyWithIsolatedConfig,
                _ => LaunchMediaSafety::UnsafeUnsupported,
            },
            // Explicit config AND HOME/XDG/TMPDIR: the flag alone does not
            // catch every state write (see module docs).
            config_isolation: ConfigIsolation::Combined { flag: "--cfg_file" },
            persistent_state: PersistentStatePolicy::DisposableSession,
        }
    }

    fn suffix(self) -> Result<&'static str, Caprice32Error> {
        match self {
            Self::Dsk => Ok("dsk"),
            Self::Cdt => Ok("cdt"),
            _ => Err(Caprice32Error::UnsupportedMedia(
                "Caprice32 V1 accepts only bounded, verified DSK and CDT media; IPF, CPR and SNA are refused",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caprice32Profile {
    pub id: String,
    pub executable: PathBuf,
    pub machine: Caprice32Machine,
    /// Must be EXACTLY `isolated_config_seed(machine)`. Live user config is
    /// never imported or overwritten.
    pub isolated_seed: PathBuf,
    /// Explicit consent: the system ROMs in `<exe dir>/rom/` are present and
    /// hash-bound but there is no trust catalogue to call them verified.
    pub accept_present_unverified_roms: bool,
    pub disposable_session_acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caprice32Media {
    pub path: PathBuf,
    pub format: Caprice32MediaFormat,
    pub identity: CanonicalIdentityStatus,
    pub platform_evidence: LocalEvidenceStrength,
    pub verified_source_sha256: [u8; 32],
}

#[derive(Debug)]
pub enum Caprice32Error {
    ExecutableMissing,
    ExecutableUnsafe,
    IdentityNotVerified,
    UnsupportedMedia(&'static str),
    FirmwareMissing,
    FirmwareNeedsAcknowledgement,
    IsolatedProfileRequired,
    DisposableSessionRequired,
    ProfileChanged,
    Sandbox(SafeLaunchSandboxError),
    Io(std::io::Error),
}

impl std::fmt::Display for Caprice32Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExecutableMissing => f.write_str("Caprice32 executable is missing; select a native installation"),
            Self::ExecutableUnsafe => f.write_str(
                "Caprice32 requires an unchanged regular (non-symlink) executable with execute permission",
            ),
            Self::IdentityNotVerified => f.write_str(
                "A fresh, resolved Amstrad CPC identity and matching verified source hash are required",
            ),
            Self::UnsupportedMedia(reason) => f.write_str(reason),
            Self::FirmwareMissing => f.write_str(
                "A required Caprice32 system ROM is missing from <executable directory>/rom/",
            ),
            Self::FirmwareNeedsAcknowledgement => f.write_str(
                "System ROMs are present but unverified; explicit acknowledgement is required",
            ),
            Self::IsolatedProfileRequired => {
                f.write_str("An exact EmuWiz isolated Caprice32 profile seed is required")
            }
            Self::DisposableSessionRequired => f.write_str(DISPOSABLE_SESSION_WARNING),
            Self::ProfileChanged => {
                f.write_str("Caprice32 profile, executable, firmware or identity changed since planning")
            }
            Self::Sandbox(error) => write!(f, "Caprice32 safe preparation refused: {error}"),
            Self::Io(error) => write!(f, "Caprice32 input unavailable: {error}"),
        }
    }
}
impl std::error::Error for Caprice32Error {}
impl From<SafeLaunchSandboxError> for Caprice32Error {
    fn from(error: SafeLaunchSandboxError) -> Self {
        Self::Sandbox(error)
    }
}
impl From<std::io::Error> for Caprice32Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// The only seed accepted by the adapter (an INI file, as Caprice32 parses it).
/// Every default-writable location is redirected to the workspace's `state/`;
/// the paths are relative because the process runs in the workspace.
pub fn isolated_config_seed(machine: Caprice32Machine) -> String {
    format!(
        "# EmuWiz Caprice32 disposable profile V2\n\
         [system]\n\
         model={}\n\
         [file]\n\
         snap_path=state/\n\
         cart_path=state/\n\
         dsk_path=state/\n\
         tape_path=state/\n\
         printer_file=state/printer.dat\n\
         sdump_dir=state\n",
        machine.model_value()
    )
}

/// Bounded discovery only; never runs a candidate. PATH symlinks are reported
/// with their eligible target rather than followed.
pub fn discover_executables(explicit: &[PathBuf], path_env: Option<&OsStr>) -> ExecutableDiscovery {
    super::native_support::discover_executables(explicit, path_env, &[ADAPTER_ID, "cap32"])
}

fn executable_binding(path: &Path) -> Result<ExecutableBinding, Caprice32Error> {
    ExecutableBinding::capture(path).map_err(|refusal| match refusal {
        ExecutableRefusal::Missing => Caprice32Error::ExecutableMissing,
        ExecutableRefusal::Unsafe => Caprice32Error::ExecutableUnsafe,
    })
}

/// Hash-bound system ROMs from the default `rom_path` (`<exe dir>/rom/`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareBinding {
    roms: Vec<(PathBuf, [u8; 32], u64)>,
}

impl FirmwareBinding {
    /// `Missing` unless every ROM this model loads is a regular, non-symlink
    /// file next to the executable. Never `Verified`: no trust catalogue.
    pub fn capture(executable: &Path, machine: Caprice32Machine) -> Result<Self, Caprice32Error> {
        let directory = executable
            .parent()
            .ok_or(Caprice32Error::FirmwareMissing)?
            .join("rom");
        let mut roms = Vec::new();
        for name in machine.required_roms() {
            let path = directory.join(name);
            let meta = fs::symlink_metadata(&path).map_err(|_| Caprice32Error::FirmwareMissing)?;
            if !meta.is_file()
                || meta.file_type().is_symlink()
                || meta.len() == 0
                || meta.len() > 1024 * 1024
            {
                return Err(Caprice32Error::FirmwareMissing);
            }
            let mut file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)?;
            let mut bytes = Vec::new();
            file.by_ref()
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            roms.push((path, Sha256::digest(&bytes).into(), bytes.len() as u64));
        }
        Ok(Self { roms })
    }

    pub fn readiness(&self) -> FirmwareReadiness {
        FirmwareReadiness::PresentUnverified
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.roms.iter().map(|(path, _, _)| path.as_path())
    }
}

fn prefix(source: &SourceProvenance, count: usize) -> Result<Vec<u8>, Caprice32Error> {
    read_prefix(source, count).map_err(|error| {
        if error.kind() == std::io::ErrorKind::Other {
            Caprice32Error::IdentityNotVerified
        } else {
            Caprice32Error::Io(error)
        }
    })
}

fn validate_media(media: &Caprice32Media, source: &SourceProvenance) -> Result<(), Caprice32Error> {
    if media.platform_evidence != LocalEvidenceStrength::Verified
        || !matches!(&media.identity, CanonicalIdentityStatus::Resolved(id)
            if id.platform_id == PLATFORM_ID && !id.game_key.trim().is_empty())
        || media.verified_source_sha256 != source.sha256
    {
        return Err(Caprice32Error::IdentityNotVerified);
    }
    let header = prefix(source, 64)?;
    let size = source.original_identity.size;
    let valid = match media.format {
        // CPCEMU's two authoritative container signatures. The parser remains
        // upstream; these are only bounded attachment guards.
        Caprice32MediaFormat::Dsk => {
            header.starts_with(b"MV - CPCEMU Disk-File")
                || header.starts_with(b"EXTENDED CPC DSK File")
        }
        // CDT is a TZX-compatible container. Platform identity must already
        // be verified because the same signature is shared with Spectrum TZX.
        Caprice32MediaFormat::Cdt => header.starts_with(b"ZXTape!\x1a"),
        _ => false,
    } && size > 64
        && size <= 32 * 1024 * 1024;
    if !valid {
        return Err(Caprice32Error::UnsupportedMedia(
            "media is not a bounded Caprice32 DSK/CDT container",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Caprice32Plan {
    profile: Caprice32Profile,
    media: Caprice32Media,
    sandbox: SandboxPlan,
    executable_binding: ExecutableBinding,
    firmware: FirmwareBinding,
}

impl Caprice32Plan {
    pub fn sandbox_plan(&self) -> &SandboxPlan {
        &self.sandbox
    }
    pub fn profile(&self) -> &Caprice32Profile {
        &self.profile
    }
    pub fn media(&self) -> &Caprice32Media {
        &self.media
    }
    pub fn firmware(&self) -> &FirmwareBinding {
        &self.firmware
    }
    pub fn firmware_readiness(&self) -> FirmwareReadiness {
        self.firmware.readiness()
    }
    pub fn readiness(&self) -> LaunchReadiness {
        LaunchReadiness::ReadyWithWarnings
    }
    pub fn warnings(&self) -> [&'static str; 2] {
        [
            DISPOSABLE_SESSION_WARNING,
            "System ROMs are present and hash-bound; contents remain unverified",
        ]
    }

    fn fresh(
        &self,
        profile: &Caprice32Profile,
        media: &Caprice32Media,
    ) -> Result<(), Caprice32Error> {
        if profile != &self.profile || media != &self.media {
            return Err(Caprice32Error::ProfileChanged);
        }
        if executable_binding(&profile.executable)? != self.executable_binding {
            return Err(Caprice32Error::ProfileChanged);
        }
        // Firmware is read from the install directory: a changed, replaced or
        // removed ROM is a different launch than the one reviewed.
        match FirmwareBinding::capture(&profile.executable, profile.machine) {
            Ok(now) if now == self.firmware => {}
            Ok(_) => return Err(Caprice32Error::ProfileChanged),
            Err(error) => return Err(error),
        }
        self.sandbox.revalidate_sources()?;
        validate_media(media, self.sandbox.sources().next().expect("primary"))?;
        Ok(())
    }

    fn command(&self, sandbox: &PreparedSandbox) -> Result<PreparedProcessCommand, Caprice32Error> {
        let primary = sandbox
            .mappings()
            .iter()
            .find(|m| m.role == MediaRole::PrimaryMedia)
            .ok_or(Caprice32Error::IsolatedProfileRequired)?;
        let mut arguments = sandbox.config_arguments();
        arguments.push(primary.scratch_path.clone().into_os_string());
        Ok(PreparedProcessCommand {
            executable: self.profile.executable.clone(),
            arguments,
            working_directory: Some(sandbox.workspace_path().to_owned()),
        })
    }

    pub fn prepare(
        &self,
        manager: &SandboxManager,
        profile: &Caprice32Profile,
        media: &Caprice32Media,
    ) -> Result<PreparedCaprice32, Caprice32Error> {
        self.fresh(profile, media)?;
        let sandbox = manager.prepare(&self.sandbox)?;
        self.fresh(profile, media)?;
        let command = self.command(&sandbox)?;
        Ok(PreparedCaprice32 {
            plan: self.clone(),
            sandbox,
            command,
        })
    }
}

pub fn plan_caprice32(
    profile: &Caprice32Profile,
    media: &Caprice32Media,
) -> Result<Caprice32Plan, Caprice32Error> {
    let suffix = media.format.suffix()?;
    if !profile.disposable_session_acknowledged {
        return Err(Caprice32Error::DisposableSessionRequired);
    }
    if profile.id.trim().is_empty() || profile.id.len() > 128 {
        return Err(Caprice32Error::IsolatedProfileRequired);
    }
    let executable_binding = executable_binding(&profile.executable)?;
    let firmware = FirmwareBinding::capture(&profile.executable, profile.machine)?;
    if !profile.accept_present_unverified_roms {
        return Err(Caprice32Error::FirmwareNeedsAcknowledgement);
    }
    let seed = isolated_config_seed(profile.machine);
    let members = [member(
        &media.path,
        MediaRole::PrimaryMedia,
        if matches!(media.format, Caprice32MediaFormat::Dsk) {
            MediaKind::Floppy
        } else {
            MediaKind::Tape
        },
        suffix,
    )];
    let sandbox = plan_sandbox(
        media.format.safety(),
        &members,
        ProfileSeed::KnownProfile {
            source: profile.isolated_seed.clone(),
            scratch_relative: "caprice32.cfg".into(),
        },
    )?;
    let sources: Vec<_> = sandbox.sources().collect();
    validate_media(media, sources[0])?;
    let config = sources
        .last()
        .ok_or(Caprice32Error::IsolatedProfileRequired)?;
    if !seed_is_exact(config, &seed)? {
        return Err(Caprice32Error::IsolatedProfileRequired);
    }
    let plan = Caprice32Plan {
        profile: profile.clone(),
        media: media.clone(),
        sandbox,
        executable_binding,
        firmware,
    };
    plan.fresh(profile, media)?;
    Ok(plan)
}

pub struct PreparedCaprice32 {
    plan: Caprice32Plan,
    sandbox: PreparedSandbox,
    command: PreparedProcessCommand,
}

impl PreparedCaprice32 {
    pub fn command_preview(&self) -> &PreparedProcessCommand {
        &self.command
    }
    pub fn workspace_path(&self) -> &Path {
        self.sandbox.workspace_path()
    }
    pub fn original_plan(&self) -> &Caprice32Plan {
        &self.plan
    }
    pub fn spawn(
        self,
        profile: &Caprice32Profile,
        media: &Caprice32Media,
    ) -> Result<SandboxedProcess, Caprice32Error> {
        self.plan.fresh(profile, media)?;
        Ok(self.sandbox.spawn(self.command)?)
    }
}
