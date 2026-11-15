//! The user's own Atari800 launch setup, and the one launch path built on it.
//!
//! [`super::atari800`] plans a disposable session from an explicit profile but
//! deliberately owns no settings. This module is the missing owner: a small
//! EmuWiz file holding what the user chose (emulator, machine, video standard,
//! OS/BASIC ROM bindings and the two consents), plus the EmuWiz-owned isolated
//! config seed. Nothing here is inferred: a machine, a ROM role and a consent
//! are only ever what the user selected, and a ROM is bound by role, size and
//! SHA-256 so a later replacement is refused rather than silently accepted.
//!
//! The user's own Atari800 configuration (`~/.atari800.cfg`) is never read or
//! written. Original game and firmware files are only ever opened read-only.

#[cfg(test)]
mod tests;

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identity_source::model::LocalEvidenceStrength;

use super::atari800::{
    ADAPTER_ID, Atari800Error, Atari800Machine, Atari800Media, Atari800MediaFormat,
    Atari800Profile, Atari800Rom, Atari800RomKind, Atari800Video, DISPOSABLE_SESSION_WARNING,
    PLATFORM_ID, discover_executables, isolated_config_seed, plan_atari800,
};
use super::native_support::{SymlinkedExecutable, executable_identity};
use super::planning::{CanonicalIdentityStatus, ResolvedIdentity, StandaloneProfileInput};
use super::process_spawn::{CapturedFileIdentity, open_regular_file_nonblocking};
use super::readiness::FirmwareReadiness;
use super::safe_launch_sandbox::{SandboxManager, SandboxedProcess};

/// Leaf under the EmuWiz config directory.
pub const SETUP_FILE: &str = "atari800_setup.json";
/// Leaf directory under the EmuWiz data directory holding the config seeds.
pub const SEED_DIRECTORY: &str = "atari800";
const SETUP_VERSION: u32 = 1;
const MAX_SETUP_BYTES: u64 = 64 * 1024;
/// Largest ATR envelope [`super::atari800`] accepts, header included.
const MAX_ATR_BYTES: u64 = 184_320 + 16;

/// A ROM the user explicitly assigned to one role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Atari800RomBinding {
    pub path: PathBuf,
    /// Lower-case hex SHA-256 of the file when it was bound.
    pub sha256: String,
    pub size: u64,
}

/// Everything the user chose. Every field starts unset: a machine, a video
/// standard and both consents are never defaulted on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Atari800Setup {
    #[serde(default)]
    pub version: u32,
    /// An explicitly selected emulator. `None` uses the single `atari800`
    /// found on `PATH`, and refuses to pick between several.
    #[serde(default)]
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub machine: Option<Atari800Machine>,
    #[serde(default)]
    pub video: Option<Atari800Video>,
    #[serde(default)]
    pub os_400_800: Option<Atari800RomBinding>,
    #[serde(default)]
    pub os_xl_xe: Option<Atari800RomBinding>,
    #[serde(default)]
    pub basic: Option<Atari800RomBinding>,
    /// Start with the bound BASIC ROM. Off means BASIC is disabled.
    #[serde(default)]
    pub basic_enabled: bool,
    #[serde(default)]
    pub accept_present_unverified_roms: bool,
    #[serde(default)]
    pub disposable_session_acknowledged: bool,
}

/// Where the setup and seeds live. Tests supply scratch paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atari800Paths {
    pub setup_file: PathBuf,
    pub seed_directory: PathBuf,
}

impl Atari800Paths {
    pub fn from_environment() -> Result<Self, String> {
        Ok(Self {
            setup_file: crate::app_dirs::config_path(SETUP_FILE).map_err(|e| e.to_string())?,
            // The data folder itself may legitimately be reached through a
            // link (a relocated XDG data home). Resolve that once here; the
            // seed folder below it must then be a real directory.
            seed_directory: resolved_existing_prefix(
                &crate::app_dirs::data_dir().map_err(|e| e.to_string())?,
            )
            .join(SEED_DIRECTORY),
        })
    }
}

/// `path` with its longest existing prefix resolved to the real location.
fn resolved_existing_prefix(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(real) = std::fs::canonicalize(ancestor) {
            return match path.strip_prefix(ancestor) {
                Ok(rest) if !rest.as_os_str().is_empty() => real.join(rest),
                _ => real,
            };
        }
    }
    path.to_owned()
}

pub fn required_rom_size(kind: Atari800RomKind) -> u64 {
    match kind {
        Atari800RomKind::Os400800 => 10240,
        Atari800RomKind::OsXlXe => 16384,
        Atari800RomKind::Basic => 8192,
    }
}

pub fn os_kind_for(machine: Atari800Machine) -> Atari800RomKind {
    match machine {
        Atari800Machine::Atari400800 => Atari800RomKind::Os400800,
        Atari800Machine::Atari800Xl | Atari800Machine::Atari130Xe => Atari800RomKind::OsXlXe,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// An absolute path with no symlink, `.` or `..` anywhere in it.
fn is_canonical(path: &Path) -> bool {
    path.is_absolute() && std::fs::canonicalize(path).ok().as_deref() == Some(path)
}

/// Reads at most `limit` bytes of one regular file through a single
/// non-blocking descriptor, and proves the path still names that file.
fn read_bound_file(path: &Path, limit: u64) -> Result<(Vec<u8>, CapturedFileIdentity), String> {
    if !is_canonical(path) {
        return Err(format!(
            "{} is missing, or its path is not absolute or passes through a symbolic link",
            path.display()
        ));
    }
    let file = open_regular_file_nonblocking(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    let identity = CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?);
    if identity.size > limit {
        return Err(format!("{} is larger than expected", path.display()));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    let after = std::fs::symlink_metadata(path)
        .map(|metadata| CapturedFileIdentity::capture(&metadata))
        .map_err(|error| error.to_string())?;
    if after != identity || bytes.len() as u64 != identity.size {
        return Err(format!("{} changed while it was read", path.display()));
    }
    Ok((bytes, identity))
}

/// Binds `path` to `kind`. The role is the caller's explicit choice; the file
/// only has to be a regular, non-symlinked file of that role's exact size.
pub fn bind_rom(path: &Path, kind: Atari800RomKind) -> Result<Atari800RomBinding, String> {
    let expected = required_rom_size(kind);
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "{} is a symbolic link or not a regular file. Select the real ROM file. Nothing was changed.",
            path.display()
        ));
    }
    let size = metadata.len();
    if size != expected {
        return Err(format!(
            "{} is {size} bytes; this ROM role needs exactly {expected} bytes. Nothing was changed.",
            path.display()
        ));
    }
    let (bytes, _) = read_bound_file(path, expected)?;
    if bytes.len() as u64 != expected {
        return Err(format!("{} changed while it was read", path.display()));
    }
    Ok(Atari800RomBinding {
        path: path.to_owned(),
        sha256: hex(&Sha256::digest(&bytes)),
        size: expected,
    })
}

/// What one bound ROM looks like right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atari800RomState {
    NotBound,
    Missing,
    /// The file is there but is no longer the bytes the user bound.
    Changed,
    Bound,
}

pub fn rom_state(binding: Option<&Atari800RomBinding>, kind: Atari800RomKind) -> Atari800RomState {
    let Some(binding) = binding else {
        return Atari800RomState::NotBound;
    };
    if std::fs::symlink_metadata(&binding.path).is_err() {
        return Atari800RomState::Missing;
    }
    let expected = required_rom_size(kind);
    match read_bound_file(&binding.path, expected) {
        Ok((bytes, _))
            if binding.size == expected
                && bytes.len() as u64 == expected
                && hex(&Sha256::digest(&bytes)) == binding.sha256 =>
        {
            Atari800RomState::Bound
        }
        _ => Atari800RomState::Changed,
    }
}

pub fn load_setup(path: &Path) -> Result<Atari800Setup, String> {
    let file = match open_regular_file_nonblocking(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Atari800Setup::default());
        }
        Err(error) => return Err(format!("Atari800 setup cannot be read: {error}")),
    };
    let mut bytes = Vec::new();
    file.take(MAX_SETUP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Atari800 setup cannot be read: {error}"))?;
    if bytes.len() as u64 > MAX_SETUP_BYTES {
        return Err("Atari800 setup is too large to be a setup file.".into());
    }
    let setup: Atari800Setup = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Atari800 setup is not valid: {error}"))?;
    if setup.version > SETUP_VERSION {
        return Err("Atari800 setup was written by a newer EmuWiz.".into());
    }
    Ok(setup)
}

/// Writes `bytes` to `path` through a private temporary file in the same
/// directory. Refuses to replace anything that is not a regular file.
fn write_owned_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("{} is not a regular file", path.display()));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error.to_string());
        }
        _ => {}
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temp.write_all(bytes).map_err(|error| error.to_string())?;
    temp.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temp.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn save_setup(path: &Path, setup: &Atari800Setup) -> Result<(), String> {
    let mut setup = setup.clone();
    setup.version = SETUP_VERSION;
    let bytes = serde_json::to_vec_pretty(&setup).map_err(|error| error.to_string())?;
    write_owned_file(path, &bytes).map_err(|error| format!("Atari800 setup was not saved: {error}"))
}

/// The EmuWiz-owned isolated config seed for this machine/BASIC choice,
/// created or repaired so that it is exactly [`isolated_config_seed`]. This
/// is the only file a launch writes outside the scratch workspace, and it is
/// EmuWiz's own: the user's Atari800 configuration is never touched.
pub fn ensure_isolated_seed(
    directory: &Path,
    machine: Atari800Machine,
    basic: bool,
) -> Result<PathBuf, String> {
    // Checked before anything is created: nothing is ever made through a
    // link. The nearest existing ancestor must already be a real path.
    let through_link =
        || "The Atari800 seed folder passes through a symbolic link; EmuWiz will not use it.";
    let existing = directory
        .ancestors()
        .find(|ancestor| std::fs::symlink_metadata(ancestor).is_ok())
        .ok_or_else(through_link)?;
    if !is_canonical(existing) {
        return Err(through_link().into());
    }
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("The Atari800 seed folder could not be created: {error}"))?;
    if !is_canonical(directory) {
        return Err(through_link().into());
    }
    let path = directory.join(if basic {
        "isolated-v1-basic-on.cfg"
    } else {
        "isolated-v1-basic-off.cfg"
    });
    let expected = isolated_config_seed(machine, basic);
    let current = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && metadata.len() == expected.len() as u64 => {
            read_bound_file(&path, expected.len() as u64)
                .ok()
                .map(|(bytes, _)| bytes)
        }
        _ => None,
    };
    if current.as_deref() != Some(expected.as_bytes()) {
        write_owned_file(&path, expected.as_bytes())
            .map_err(|error| format!("The Atari800 config seed was not written: {error}"))?;
    }
    Ok(path)
}

/// One specific thing between the user and a launch, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Atari800SetupBlocker {
    EmulatorMissing,
    EmulatorUnsafe(PathBuf),
    EmulatorAmbiguous(Vec<PathBuf>),
    MachineNotChosen,
    VideoNotChosen,
    OsRomNotBound(Atari800RomKind),
    OsRomMissing(PathBuf),
    OsRomChanged(PathBuf),
    BasicRomNotBound,
    BasicRomMissing(PathBuf),
    BasicRomChanged(PathBuf),
    UnverifiedFirmwareNotAccepted,
    DisposableSessionNotAcknowledged,
}

impl std::fmt::Display for Atari800SetupBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmulatorMissing => f.write_str(
                "Atari800 is not installed, or the selected program is gone. Install atari800 or select its program.",
            ),
            Self::EmulatorUnsafe(path) => write!(
                f,
                "{} is not a regular, executable, non-symlinked program. Select the real atari800 program.",
                path.display()
            ),
            Self::EmulatorAmbiguous(paths) => write!(
                f,
                "More than one atari800 program was found ({}). Select which one to use.",
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::MachineNotChosen => {
                f.write_str("Choose the Atari machine: 400/800, 800XL or 130XE.")
            }
            Self::VideoNotChosen => f.write_str("Choose the video standard: PAL or NTSC."),
            Self::OsRomNotBound(Atari800RomKind::Os400800) => f.write_str(
                "Select your Atari 400/800 OS ROM (10,240 bytes). EmuWiz does not supply firmware.",
            ),
            Self::OsRomNotBound(_) => f.write_str(
                "Select your Atari XL/XE OS ROM (16,384 bytes). EmuWiz does not supply firmware.",
            ),
            Self::OsRomMissing(path) => write!(
                f,
                "The selected OS ROM is missing: {}. Select it again.",
                path.display()
            ),
            Self::OsRomChanged(path) => write!(
                f,
                "The OS ROM at {} is no longer the file you selected. Select it again to accept the new file.",
                path.display()
            ),
            Self::BasicRomNotBound => f.write_str(
                "BASIC is switched on but no BASIC ROM (8,192 bytes) is selected. Select one or switch BASIC off.",
            ),
            Self::BasicRomMissing(path) => write!(
                f,
                "The selected BASIC ROM is missing: {}. Select it again or switch BASIC off.",
                path.display()
            ),
            Self::BasicRomChanged(path) => write!(
                f,
                "The BASIC ROM at {} is no longer the file you selected. Select it again to accept the new file.",
                path.display()
            ),
            Self::UnverifiedFirmwareNotAccepted => f.write_str(
                "The ROMs have the right role and size but EmuWiz cannot verify their contents. Confirm that you accept using them.",
            ),
            Self::DisposableSessionNotAcknowledged => write!(
                f,
                "Confirm the temporary session: {DISPOSABLE_SESSION_WARNING}"
            ),
        }
    }
}

/// The setup as it stands right now. Read-only: nothing is written or run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atari800Resolution {
    /// The one emulator a launch would use, when there is exactly one.
    pub executable: Option<PathBuf>,
    /// Every eligible `atari800` found, for the setup panel.
    pub discovered: Vec<PathBuf>,
    pub symlink_refusals: Vec<SymlinkedExecutable>,
    pub firmware: FirmwareReadiness,
    pub blockers: Vec<Atari800SetupBlocker>,
    /// The OS ROM the chosen machine needs, as it is right now.
    pub os_rom: Atari800RomState,
    /// The BASIC ROM as it is right now; not examined while BASIC is off.
    pub basic_rom: Atari800RomState,
    /// The user has stored at least one choice.
    pub configured: bool,
}

/// Profile id of a setup that has no usable emulator yet. It never matches a
/// launchable profile, so it can only ever name a blocked candidate.
pub const UNSELECTED_PROFILE_ID: &str = "atari800:unselected";

impl Atari800Resolution {
    pub fn eligible(&self) -> bool {
        self.blockers.is_empty()
    }

    /// A short, bounded id for the selected emulator: a digest of its path,
    /// so a long or non-UTF-8 path cannot exceed Core's profile-id limit.
    pub fn profile_id(&self) -> Option<String> {
        self.executable.as_ref().map(|path| {
            let digest = Sha256::digest(path.as_os_str().as_encoded_bytes());
            format!("{ADAPTER_ID}:{}", hex(&digest[..8]))
        })
    }

    /// The planner candidate. Present whenever there is something for the
    /// user to act on: an Atari800 program was found, or a setup was started.
    /// Without a usable emulator it is a blocked candidate, so the setup
    /// panel stays reachable to fix exactly that.
    pub fn standalone_input(&self) -> Option<StandaloneProfileInput> {
        if self.executable.is_none() && self.discovered.is_empty() && !self.configured {
            return None;
        }
        Some(StandaloneProfileInput {
            adapter_id: ADAPTER_ID,
            profile_id: self
                .profile_id()
                .unwrap_or_else(|| UNSELECTED_PROFILE_ID.into()),
            profile_path: self.executable.clone(),
            eligible: self.eligible(),
            firmware: self.firmware,
        })
    }

    /// One sentence for the first thing to fix.
    pub fn first_blocker(&self) -> Option<String> {
        self.blockers.first().map(ToString::to_string)
    }
}

pub fn resolve_setup(setup: &Atari800Setup, path_env: Option<&OsStr>) -> Atari800Resolution {
    let mut blockers = Vec::new();
    let discovery = discover_executables(&[], path_env);
    // One program reached through two PATH entries (a merged /usr makes /bin
    // a link to /usr/bin) is one program: keep each real path once.
    let mut discovered: Vec<PathBuf> = discovery
        .executables
        .iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .filter(|path| executable_identity(path).is_ok())
        .collect();
    discovered.sort();
    discovered.dedup();
    // Presence and type only: the binary's bytes are bound when a launch is
    // planned, not on every redraw of a readiness page.
    let executable = match &setup.executable {
        Some(path) => match executable_identity(path) {
            Ok(_) if is_canonical(path) => Some(path.clone()),
            Ok(_) => {
                blockers.push(Atari800SetupBlocker::EmulatorUnsafe(path.clone()));
                None
            }
            Err(super::native_support::ExecutableRefusal::Missing) => {
                blockers.push(Atari800SetupBlocker::EmulatorMissing);
                None
            }
            Err(super::native_support::ExecutableRefusal::Unsafe) => {
                blockers.push(Atari800SetupBlocker::EmulatorUnsafe(path.clone()));
                None
            }
        },
        None => match discovered.as_slice() {
            [only] => Some(only.clone()),
            [] => {
                blockers.push(Atari800SetupBlocker::EmulatorMissing);
                None
            }
            several => {
                blockers.push(Atari800SetupBlocker::EmulatorAmbiguous(several.to_vec()));
                None
            }
        },
    };
    if setup.machine.is_none() {
        blockers.push(Atari800SetupBlocker::MachineNotChosen);
    }
    if setup.video.is_none() {
        blockers.push(Atari800SetupBlocker::VideoNotChosen);
    }
    let mut firmware = FirmwareReadiness::Unknown;
    let mut os_rom = Atari800RomState::NotBound;
    // Read only when it is used: a BASIC ROM left selected on a drive that
    // is gone must not stall a setup that has BASIC switched off.
    let basic_rom = if setup.basic_enabled {
        rom_state(setup.basic.as_ref(), Atari800RomKind::Basic)
    } else {
        Atari800RomState::NotBound
    };
    if let Some(machine) = setup.machine {
        let kind = os_kind_for(machine);
        let binding = os_binding(setup, machine);
        os_rom = rom_state(binding, kind);
        firmware = FirmwareReadiness::PresentUnverified;
        if os_rom != Atari800RomState::Bound {
            firmware = FirmwareReadiness::Missing;
            let path = binding.map(|b| b.path.clone()).unwrap_or_default();
            blockers.push(match os_rom {
                Atari800RomState::NotBound => Atari800SetupBlocker::OsRomNotBound(kind),
                Atari800RomState::Missing => Atari800SetupBlocker::OsRomMissing(path),
                _ => Atari800SetupBlocker::OsRomChanged(path),
            });
        }
        if setup.basic_enabled && basic_rom != Atari800RomState::Bound {
            firmware = FirmwareReadiness::Missing;
            let path = setup
                .basic
                .as_ref()
                .map(|b| b.path.clone())
                .unwrap_or_default();
            blockers.push(match basic_rom {
                Atari800RomState::NotBound => Atari800SetupBlocker::BasicRomNotBound,
                Atari800RomState::Missing => Atari800SetupBlocker::BasicRomMissing(path),
                _ => Atari800SetupBlocker::BasicRomChanged(path),
            });
        }
    }
    if firmware == FirmwareReadiness::PresentUnverified && !setup.accept_present_unverified_roms {
        blockers.push(Atari800SetupBlocker::UnverifiedFirmwareNotAccepted);
    }
    if !setup.disposable_session_acknowledged {
        blockers.push(Atari800SetupBlocker::DisposableSessionNotAcknowledged);
    }
    Atari800Resolution {
        executable,
        discovered,
        symlink_refusals: discovery.symlink_refusals,
        firmware,
        blockers,
        os_rom,
        basic_rom,
        configured: *setup != Atari800Setup::default(),
    }
}

fn os_binding(setup: &Atari800Setup, machine: Atari800Machine) -> Option<&Atari800RomBinding> {
    match os_kind_for(machine) {
        Atari800RomKind::Os400800 => setup.os_400_800.as_ref(),
        _ => setup.os_xl_xe.as_ref(),
    }
}

/// Why a launch did not happen. `Display` is the sentence the user reads.
#[derive(Debug)]
pub enum Atari800LaunchRefusal {
    Setup(String),
    /// The game file is not the one that was offered, or is not a valid ATR.
    Game(String),
    Core(Atari800Error),
    /// The scratch workspace could not be opened at all.
    Workspace(String),
}

impl std::fmt::Display for Atari800LaunchRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup(detail) => write!(f, "Atari800 was not started: {detail}"),
            Self::Game(detail) => write!(f, "Atari800 was not started: {detail}"),
            Self::Core(error) => write!(f, "Atari800 was not started: {error}."),
            Self::Workspace(detail) => write!(
                f,
                "Atari800 was not started: the temporary launch workspace is unavailable ({detail})."
            ),
        }
    }
}
impl std::error::Error for Atari800LaunchRefusal {}

/// Re-reads the selected game through one descriptor and proves it is still
/// the offered file, still the catalogued bytes, and an ATR by its header.
///
/// `expected_game_key` is the library's own SHA-256 of this file. The
/// resulting platform evidence is a header and a hash EmuWiz computed itself;
/// it is a local identity, never a DAT verification.
pub fn verify_selected_atr(
    path: &Path,
    expected_game_key: &str,
    offered: &CapturedFileIdentity,
) -> Result<Atari800Media, Atari800LaunchRefusal> {
    let game = Atari800LaunchRefusal::Game;
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("atr"))
    {
        return Err(game(
            "only a direct .atr disk image is supported; other Atari 8-bit formats are not launched.".into(),
        ));
    }
    let (bytes, identity) = read_bound_file(path, MAX_ATR_BYTES)
        .map_err(|detail| game(format!("{detail}. Recheck this game.")))?;
    if identity != *offered {
        return Err(game(
            "the game file changed since Play was offered. Recheck this game.".into(),
        ));
    }
    let digest = Sha256::digest(&bytes);
    if hex(&digest) != expected_game_key.to_ascii_lowercase() {
        return Err(game(
            "the game file no longer matches the game in your library. Rescan it.".into(),
        ));
    }
    if bytes.len() < 16 || bytes[..2] != [0x96, 0x02] {
        return Err(game(
            "this file does not have an ATR disk header, so EmuWiz will not hand it to Atari800 as a disk."
                .into(),
        ));
    }
    Ok(Atari800Media {
        path: path.to_owned(),
        format: Atari800MediaFormat::Atr,
        identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: PLATFORM_ID.into(),
            game_key: expected_game_key.to_owned(),
        }),
        platform_evidence: LocalEvidenceStrength::Verified,
        verified_source_sha256: digest.into(),
    })
}

/// The typed profile for a fully resolved setup. Fails with the first setup
/// blocker; never fills in a missing choice.
pub fn profile_from_setup(
    setup: &Atari800Setup,
    resolution: &Atari800Resolution,
    seed_directory: &Path,
) -> Result<Atari800Profile, Atari800LaunchRefusal> {
    if let Some(blocker) = resolution.first_blocker() {
        return Err(Atari800LaunchRefusal::Setup(blocker));
    }
    let incomplete = || Atari800LaunchRefusal::Setup("the Atari800 setup is incomplete.".into());
    let executable = resolution.executable.clone().ok_or_else(incomplete)?;
    let machine = setup.machine.ok_or_else(incomplete)?;
    let video = setup.video.ok_or_else(incomplete)?;
    let os = os_binding(setup, machine).ok_or_else(incomplete)?;
    let basic = if setup.basic_enabled {
        Some(setup.basic.as_ref().ok_or_else(incomplete)?)
    } else {
        None
    };
    let isolated_seed = ensure_isolated_seed(seed_directory, machine, basic.is_some())
        .map_err(Atari800LaunchRefusal::Setup)?;
    Ok(Atari800Profile {
        id: resolution.profile_id().ok_or_else(incomplete)?,
        executable,
        machine,
        video,
        os: Atari800Rom {
            path: os.path.clone(),
            kind: os_kind_for(machine),
        },
        basic: basic.map(|rom| Atari800Rom {
            path: rom.path.clone(),
            kind: Atari800RomKind::Basic,
        }),
        isolated_seed,
        accept_present_unverified_roms: setup.accept_present_unverified_roms,
        disposable_session_acknowledged: setup.disposable_session_acknowledged,
    })
}

/// A started disposable session. Dropping it does not stop the emulator; the
/// workspace stays owned by the watcher until the emulator exits.
pub struct LaunchedAtari800 {
    pub process: SandboxedProcess,
    pub workspace: PathBuf,
    pub machine: Atari800Machine,
    pub video: Atari800Video,
    pub basic: bool,
    /// Older workspaces the startup sweep could not remove, with the reason.
    pub startup_cleanup_skipped: Vec<(PathBuf, String)>,
}

/// The whole click-time path: reload the setup, re-resolve the emulator and
/// firmware, re-verify the game file, plan, copy into a private workspace and
/// spawn. Every step is fresh; nothing is trusted from the earlier preview.
pub fn launch_selected_atr(
    paths: &Atari800Paths,
    path_env: Option<&OsStr>,
    expected_profile_id: &str,
    game: &Path,
    expected_game_key: &str,
    offered: &CapturedFileIdentity,
    open_manager: impl FnOnce() -> Result<
        (SandboxManager, super::safe_launch_sandbox::CleanupReport),
        super::safe_launch_sandbox::SafeLaunchSandboxError,
    >,
) -> Result<LaunchedAtari800, Atari800LaunchRefusal> {
    let setup = load_setup(&paths.setup_file).map_err(Atari800LaunchRefusal::Setup)?;
    let resolution = resolve_setup(&setup, path_env);
    if let Some(blocker) = resolution.first_blocker() {
        return Err(Atari800LaunchRefusal::Setup(blocker));
    }
    if resolution.profile_id().as_deref() != Some(expected_profile_id) {
        return Err(Atari800LaunchRefusal::Setup(
            "the Atari800 program changed since Play was offered. Recheck this game.".into(),
        ));
    }
    let media = verify_selected_atr(game, expected_game_key, offered)?;
    let profile = profile_from_setup(&setup, &resolution, &paths.seed_directory)?;
    let plan = plan_atari800(&profile, &media).map_err(Atari800LaunchRefusal::Core)?;
    // The plan hashed each ROM it will copy. Those must be the very bytes the
    // user bound, not merely a file of the right size at the same path.
    let mut bound = vec![
        os_binding(&setup, profile.machine)
            .map(|rom| rom.sha256.clone())
            .unwrap_or_default(),
    ];
    if profile.basic.is_some() {
        bound.push(
            setup
                .basic
                .as_ref()
                .map(|rom| rom.sha256.clone())
                .unwrap_or_default(),
        );
    }
    let planned: Vec<String> = plan
        .sandbox_plan()
        .sources()
        .skip(1)
        .take(bound.len())
        .map(|source| hex(&source.sha256))
        .collect();
    if planned != bound {
        return Err(Atari800LaunchRefusal::Setup(
            "a system ROM is no longer the file you selected. Select it again in the Atari800 setup."
                .into(),
        ));
    }
    let (manager, cleanup) =
        open_manager().map_err(|error| Atari800LaunchRefusal::Workspace(error.to_string()))?;
    let prepared = plan
        .prepare(&manager, &profile, &media)
        .map_err(Atari800LaunchRefusal::Core)?;
    let workspace = prepared.workspace_path().to_owned();
    let process = prepared
        .spawn(&profile, &media)
        .map_err(Atari800LaunchRefusal::Core)?;
    Ok(LaunchedAtari800 {
        process,
        workspace,
        machine: profile.machine,
        video: profile.video,
        basic: profile.basic.is_some(),
        startup_cleanup_skipped: cleanup.skipped,
    })
}
