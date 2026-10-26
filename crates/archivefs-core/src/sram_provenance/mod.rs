//! Read-only, per-artifact origin evidence from an observed native SRAM write.
//! This does not prove unique ownership, meaningful progress, portability or
//! restore safety. Historical saves and save states never acquire this fact.
//!
//! The first adapter supports native x86-64/glibc RetroArch with the selected
//! Gambatte core and Gold-verified Game Boy/Color bytes. The selected native
//! executable/core are trusted code; this is not a hostile-code sandbox.
//! Capture requires the mapped executable/core inode, actual successful ROM
//! load payload, observed selected configuration, a full direct SAVE_RAM write
//! and successful close, then stable no-follow file evidence at process exit.
//! Any loaded state, extra content, unattested SRAM read, incomplete lifecycle
//! or cooperating Blue session overlap refuses new provenance. The session
//! lease coordinates this local app-data scope, not arbitrary external writers.
//! Artifact changes are detected at finalization and each inventory refresh.
//!
//! Only sealed metadata is persisted beside existing snapshot metadata. No save
//! bytes are copied or changed. Missing/moved/modified/conflicting evidence is
//! shown for review; no directory-wide identity or restore authority is minted.
//! Preparing observation precedes the executor's final source/asset guards.
//! A native session that cannot retain coordination is refused before spawning.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

mod capture;
mod inventory;
mod protocol;
mod store;
pub(crate) use capture::prepare;
pub use inventory::inventory_with_sram_origins;
static DIAGNOSTIC: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
fn diagnostic(reason: String) {
    if let Ok(mut d) = DIAGNOSTIC.lock() {
        *d = Some(reason);
    }
}
static REVISION: AtomicU64 = AtomicU64::new(0);
pub fn completion_revision() -> u64 {
    REVISION.load(Ordering::Acquire)
}
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_RAM: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified: i64,
    pub modified_ns: i64,
    pub changed: i64,
    pub changed_ns: i64,
    pub links: u64,
    pub mode: u32,
    pub uid: u32,
}
impl Stamp {
    fn from_metadata(m: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: m.dev(),
            inode: m.ino(),
            size: m.len(),
            modified: m.mtime(),
            modified_ns: m.mtime_nsec(),
            changed: m.ctime(),
            changed_ns: m.ctime_nsec(),
            links: m.nlink(),
            mode: m.mode(),
            uid: m.uid(),
        }
    }
    fn object(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Object {
    pub path: PathBuf,
    pub stamp: Stamp,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryIdentity {
    pub device: u64,
    pub inode: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub configuration: Object,
    pub selected_executable: PathBuf,
    pub selected_core: PathBuf,
    pub saves: PathBuf,
    pub saves_identity: DirectoryIdentity,
    pub profile: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub cartridge: Object,
    pub cartridge_identity: crate::game_identity::IdentityEvidence,
    pub executable: Object,
    pub core: Object,
    pub scope: Scope,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OriginState {
    Associated,
    NeedsReview,
}
/// An observation, never an alternative spelling of Verified game identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SramOrigin {
    pub state: OriginState,
    pub session: String,
    pub context: Context,
    pub artifact: Object,
    pub explanation: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    version: u32,
    session: String,
    context: Context,
    origins: Vec<SramOrigin>,
    reason: Option<String>,
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn check_components(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("An absolute observation path is required".into());
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        if matches!(part, std::path::Component::ParentDir) {
            return Err("Parent traversal is not observed".into());
        }
        current.push(part.as_os_str());
        let m =
            fs::symlink_metadata(&current).map_err(|e| format!("{}: {e}", current.display()))?;
        if m.file_type().is_symlink() {
            return Err(format!(
                "Linked observation path refused: {}",
                current.display()
            ));
        }
    }
    Ok(())
}
// Walk each component through a retained directory descriptor. A parent path
// replaced by a link cannot redirect a hash to a different tree.
fn open_object(path: &Path) -> Result<File, String> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    use std::path::Component;
    if !path.is_absolute() {
        return Err("Absolute evidence path required".into());
    }
    let mut directory = File::open("/").map_err(|e| e.to_string())?;
    let parts: Vec<_> = path
        .components()
        .filter(|c| !matches!(c, Component::RootDir))
        .collect();
    for (i, part) in parts.iter().enumerate() {
        let Component::Normal(name) = part else {
            return Err("Non-normal evidence path refused".into());
        };
        let name = std::ffi::CString::new(name.as_bytes()).map_err(|e| e.to_string())?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if i + 1 < parts.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        directory = unsafe { File::from_raw_fd(fd) };
    }
    if !directory.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Evidence must be a regular file".into());
    }
    Ok(directory)
}
// Create only missing metadata directories, using no-follow descriptor traversal.
fn open_directory(path: &Path, create: bool) -> Result<File, String> {
    use std::{
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
        path::Component,
    };
    if !path.is_absolute() {
        return Err("Absolute directory required".into());
    }
    let mut file = File::open("/").map_err(|e| e.to_string())?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            if component == Component::RootDir {
                continue;
            }
            return Err("Invalid directory component".into());
        };
        let name = std::ffi::CString::new(name.as_bytes()).map_err(|e| e.to_string())?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let mut fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0
            && create
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
            if unsafe { libc::mkdirat(file.as_raw_fd(), name.as_ptr(), 0o700) } < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) };
        }
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        file = unsafe { File::from_raw_fd(fd) };
    }
    Ok(file)
}
fn object(path: &Path) -> Result<(Object, File), String> {
    object_file(path, open_object(path)?)
}
fn object_file(path: &Path, mut file: File) -> Result<(Object, File), String> {
    let before = Stamp::from_metadata(&file.metadata().map_err(|e| e.to_string())?);
    if before.size > MAX_FILE || before.links != 1 {
        return Err("Unbounded or hard-linked observation file refused".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = Stamp::from_metadata(&file.metadata().map_err(|e| e.to_string())?);
    if before != after || bytes.len() as u64 != before.size {
        return Err("File changed while its evidence was read".into());
    }
    Ok((
        Object {
            path: path.to_path_buf(),
            stamp: before,
            sha256: sha(&bytes),
        },
        file,
    ))
}
fn current(object_expected: &Object) -> Result<(), String> {
    let (now, _) = object(&object_expected.path)?;
    if &now != object_expected {
        return Err(format!(
            "Evidence changed: {}",
            object_expected.path.display()
        ));
    }
    Ok(())
}
fn catalogue_root() -> Result<PathBuf, String> {
    crate::app_dirs::data_dir()
        .map(|p| p.join("snapshots/sram-origins-v1"))
        .map_err(|e| e.to_string())
}
fn discover_scope(
    profile: crate::emulator_environment::retroarch::ProfileRef,
) -> Result<Scope, String> {
    use crate::emulator_environment::{HostReadOnlyFilesystem, retroarch::*};
    let env = DiscoveryEnvironment::from_process_environment();
    let report = discover_retroarch_environment(&HostReadOnlyFilesystem, &env)
        .map_err(|e| format!("{e:?}"))?;
    let p = report
        .profiles
        .iter()
        .find(|p| p.profile_kind == profile.profile_kind && p.scope == profile.scope)
        .ok_or("Selected RetroArch settings are unavailable")?;
    let configuration = &p.config_file.path;
    if configuration.lossy {
        return Err("Lossy configuration path refused".into());
    }
    let saves = p
        .paths
        .iter()
        .find(|p| p.purpose == PathPurpose::Saves)
        .and_then(|p| p.resolved_path.as_ref())
        .filter(|p| !p.lossy)
        .ok_or("Explicit RetroArch save directory is unavailable")?;
    let save_path = PathBuf::from(&saves.display);
    let save_directory = open_directory(&save_path, false)?;
    let stamp = Stamp::from_metadata(&save_directory.metadata().map_err(|e| e.to_string())?);
    let [exe] = p.evidence.executables.as_slice() else {
        return Err("Selected native executable is unavailable or ambiguous".into());
    };
    let cores: Vec<_> = p
        .cores
        .iter()
        .filter(|c| c.core_stem == "gambatte")
        .collect();
    let [core] = cores.as_slice() else {
        return Err("Selected Gambatte core is unavailable or ambiguous".into());
    };
    if exe.lossy || core.full_path.lossy {
        return Err("Lossy selected installation path refused".into());
    }
    let scope = Scope {
        selected_executable: exe.display.clone().into(),
        selected_core: core.full_path.display.clone().into(),
        configuration: object(Path::new(&configuration.display))?.0,
        saves: saves.display.clone().into(),
        saves_identity: DirectoryIdentity {
            device: stamp.device,
            inode: stamp.inode,
        },
        profile: format!("{:?}/{:?}", profile.profile_kind, profile.scope),
    };
    check_components(&scope.configuration.path)?;
    check_components(&scope.saves)?;
    Ok(Scope {
        saves: fs::canonicalize(&scope.saves).map_err(|e| e.to_string())?,
        ..scope
    })
}
#[cfg(test)]
mod tests;
