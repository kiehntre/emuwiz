//! Alternative native launch of RollerCoaster Tycoon 2 through an
//! already-installed OpenRCT2.
//!
//! OpenRCT2 is a separate GPL-3.0 project (<https://openrct2.org>); EmuWiz
//! only *starts* an executable the user installed. It never bundles,
//! downloads, installs, configures or modifies OpenRCT2, and never touches,
//! copies, migrates or downloads the user's RollerCoaster Tycoon 2 files.
//! RollerCoaster Tycoon is a trademark of its owners; EmuWiz is not
//! affiliated with them or with OpenRCT2.
//!
//! Engine identity (which OpenRCT2 executable, which version) and game
//! identity (the user-declared RCT2 data folder) are separate facts, each
//! with its own evidence. Both feed the existing read-only
//! [`alternative_launch_provider_planner`]; that planner stays advisory, so
//! this module adds the missing launch contract around it: a launch request
//! binds the exact executable and data-folder identities the user saw, and
//! [`preflight_openrct2_launch`] rediscovers everything, re-runs the planner
//! and refuses unless it still says `Ready` and nothing drifted.
//!
//! Nothing is executed until the user acts. Discovery is passive (it only
//! looks at file metadata). Reading a version needs to *run* the executable
//! with `--version`; that happens only in [`check_openrct2`], for the one
//! candidate the user explicitly chose, after its path was shown. The
//! two-second and 16 KiB limits keep a misbehaving program from hanging the
//! check; they are not a sandbox and do not make running an untrusted file
//! safe. Launch preflight never re-runs `--version`.
//!
//! # Executable binding
//!
//! The file that is version-checked is the file that is launched, by
//! descriptor, not by pathname. [`PinnedOpenRct2`] opens the candidate once
//! (`O_NOFOLLOW`), requires a regular executable native ELF (scripts and
//! wrappers cannot be descriptor-pinned and are refused) of at most 128 MiB,
//! and hashes it through that same descriptor. Both the `--version` run and
//! the final launch exec `/proc/self/fd/N` of such a descriptor, and the
//! launch additionally requires the file's identity *and* SHA-256 to equal
//! what the explicit check saw, so replacing the path (rename, symlink, copy)
//! after the check can neither be executed nor reuse the checked version.
//! Limit: a process with write access to the very same inode can still edit
//! it in place between hashing and exec (the kernel only blocks that while a
//! writer is open at exec time); the identity is re-read from the descriptor
//! immediately before use to narrow, not eliminate, that. Shared libraries
//! and OpenRCT2's own data files are not pinned, and neither is the RCT2
//! folder between preflight and OpenRCT2 reading it.
//!
//! Game identity here is *user-declared*: the folder is the one the person
//! chose, and its only content evidence is `Data/g1.dat`, the file OpenRCT2
//! itself uses to recognise an RCT2 folder. No hash or catalogue match is
//! claimed, and other data files are not verified.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::alternative_launch_provider_planner::{
    AlternativeLaunchAssetEvidence, AlternativeLaunchAssetKind, AlternativeLaunchGameIdentity,
    AlternativeLaunchIdentityConfidence, AlternativeLaunchKind, AlternativeLaunchProjection,
    AlternativeLaunchProvider, AlternativeLaunchReadiness, AlternativeLaunchRequirement,
    project_provider,
};
use crate::emulator_inventory::parse_version_output;
use crate::launch::process_spawn::{
    CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
    spawn_watched_process_private_pinned,
};

pub const OPENRCT2_PROVIDER_ID: &str = "openrct2";
/// Engine-neutral game identity: the title, not any file or executable.
pub const RCT2_GAME_ID: &str = "rollercoaster-tycoon-2";
const EXECUTABLE_NAME: &str = "openrct2";
/// Conservative floor: the `--rct2-data-path` option is long established in
/// the 0.4 series. Older or unparsable versions are refused, not guessed.
const MIN_SUPPORTED: (u64, u64) = (0, 4);
const G1_RELATIVE: [&str; 2] = ["Data", "g1.dat"];
const MAX_VERSION_BYTES: usize = 16 * 1024;
const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
/// Larger files (for example AppImages) are refused, not trusted by metadata.
const MAX_EXECUTABLE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRct2Install {
    pub executable: PathBuf,
    /// Metadata identity from passive discovery.
    pub executable_identity: CapturedFileIdentity,
    /// Set only by [`check_openrct2`], together with `content_sha256`.
    pub version: Option<String>,
    /// SHA-256 of the exact bytes the explicit check executed.
    pub content_sha256: Option<[u8; 32]>,
}

/// One verified executable file object, held open. See the module docs.
#[derive(Debug)]
pub struct PinnedOpenRct2 {
    file: fs::File,
    path: PathBuf,
    identity: CapturedFileIdentity,
    sha256: [u8; 32],
}

impl PinnedOpenRct2 {
    /// Open without following a final symlink, accept only a native ELF
    /// regular executable, hash it through this descriptor and confirm the
    /// descriptor did not change while hashing.
    pub fn open(path: &Path) -> Result<Self, String> {
        use sha2::{Digest, Sha256};
        if !path.is_absolute() {
            return Err("the executable path must be absolute".into());
        }
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .map_err(|e| format!("the executable cannot be opened safely: {e}"))?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err("the executable is not a regular executable file".into());
        }
        if metadata.len() < 4 || metadata.len() > MAX_EXECUTABLE_BYTES {
            return Err("the executable is empty or larger than the 128 MiB limit".into());
        }
        let identity = CapturedFileIdentity::capture(&metadata);
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut first = true;
        let mut remaining = metadata.len();
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..count])
                .map_err(|e| format!("the executable could not be read: {e}"))?;
            if first && buffer[..4] != *b"\x7fELF" {
                return Err(
                    "not a native ELF program (scripts and wrappers cannot be pinned and are refused)"
                        .into(),
                );
            }
            first = false;
            hash.update(&buffer[..count]);
            remaining -= count as u64;
        }
        let pinned = Self {
            file,
            path: path.to_path_buf(),
            identity,
            sha256: hash.finalize().into(),
        };
        pinned.confirm_unchanged()?;
        Ok(pinned)
    }

    /// Re-read the identity from the descriptor itself.
    fn confirm_unchanged(&self) -> Result<(), String> {
        let now = self.file.metadata().map_err(|e| e.to_string())?;
        if CapturedFileIdentity::capture(&now) != self.identity {
            return Err("the executable was modified while it was being verified".into());
        }
        Ok(())
    }

    fn proc_path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rct2DataReport {
    pub root: PathBuf,
    pub root_identity: Option<CapturedFileIdentity>,
    pub g1_identity: Option<CapturedFileIdentity>,
    /// Why the folder cannot be used; `None` when `Data/g1.dat` is present.
    pub blocker: Option<String>,
}

/// Directories searched for the executable: `PATH`, Debian's `/usr/games`
/// and `~/.local/bin`. Nothing else is crawled.
pub fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.push(PathBuf::from("/usr/games"));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/bin"));
    }
    let mut seen = std::collections::BTreeSet::new();
    dirs.retain(|d| d.is_absolute() && seen.insert(d.clone()));
    dirs.truncate(64);
    dirs
}

fn is_executable_file(path: &Path) -> Option<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).ok()?;
    (metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.permissions().mode() & 0o111 != 0)
        .then_some(metadata)
}

/// Bounded `openrct2 --version` of the *pinned descriptor* (never a path).
/// Executes the file: call only from [`check_openrct2`], after an explicit
/// user action. The environment is cleared and stdin is closed; the time and
/// output limits only stop a hang or flood and are not a sandbox.
pub fn probe_openrct2_version(pinned: &PinnedOpenRct2) -> Option<String> {
    let mut command = Command::new(pinned.proc_path());
    command
        .arg0(&pinned.path)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let started = Instant::now();
    while child.try_wait().ok()?.is_none() {
        if started.elapsed() >= VERSION_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().ok()?;
    let mut bytes = output.stdout;
    bytes.extend_from_slice(&output.stderr);
    bytes.truncate(MAX_VERSION_BYTES);
    parse_version_output(&String::from_utf8(bytes).ok()?)
}

/// Passive discovery: candidate paths only, read-only metadata, nothing is
/// executed. Paths are resolved through directory symlinks and de-duplicated;
/// an executable that is itself a symlink is skipped. `version` is `None`
/// until [`check_openrct2`] is explicitly run.
pub fn discover_openrct2(search_dirs: &[PathBuf]) -> Vec<OpenRct2Install> {
    let mut found: Vec<OpenRct2Install> = Vec::new();
    for dir in search_dirs.iter().filter(|d| d.is_absolute()).take(64) {
        let Ok(resolved_dir) = fs::canonicalize(dir) else {
            continue;
        };
        let path = resolved_dir.join(EXECUTABLE_NAME);
        if found.iter().any(|f| f.executable == path) {
            continue;
        }
        if let Some(metadata) = is_executable_file(&path) {
            found.push(OpenRct2Install {
                executable_identity: CapturedFileIdentity::capture(&metadata),
                version: None,
                content_sha256: None,
                executable: path,
            });
        }
    }
    found
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRct2CheckError {
    /// The candidate is gone, a symlink, not executable, or not the file the
    /// user was shown. Nothing was executed.
    Refused(String),
    /// It was executed but no usable version came back.
    NoVersion,
}

/// The explicit "Check OpenRCT2" action: revalidates the one selected
/// candidate against fresh, still-passive discovery immediately before
/// running `<executable> --version`. This is the only place a discovered
/// program is executed.
pub fn check_openrct2(
    candidate: &OpenRct2Install,
    search_dirs: &[PathBuf],
    probe: &dyn Fn(&PinnedOpenRct2) -> Option<String>,
) -> Result<OpenRct2Install, OpenRct2CheckError> {
    let matches: Vec<_> = discover_openrct2(search_dirs)
        .into_iter()
        .filter(|f| f.executable == candidate.executable)
        .collect();
    let [fresh] = matches.as_slice() else {
        return Err(OpenRct2CheckError::Refused(
            "the selected OpenRCT2 is no longer found at that exact path".into(),
        ));
    };
    if fresh.executable_identity != candidate.executable_identity {
        return Err(OpenRct2CheckError::Refused(
            "the selected OpenRCT2 file changed since it was listed".into(),
        ));
    }
    // Pin the exact file object, require it to be the one that was listed,
    // and run the probe on that descriptor, never on the pathname.
    let pinned = PinnedOpenRct2::open(&fresh.executable).map_err(OpenRct2CheckError::Refused)?;
    if pinned.identity != candidate.executable_identity {
        return Err(OpenRct2CheckError::Refused(
            "the selected OpenRCT2 file changed since it was listed".into(),
        ));
    }
    let version = probe(&pinned).ok_or(OpenRct2CheckError::NoVersion)?;
    pinned
        .confirm_unchanged()
        .map_err(OpenRct2CheckError::Refused)?;
    Ok(OpenRct2Install {
        version: Some(version),
        content_sha256: Some(pinned.sha256),
        ..fresh.clone()
    })
}

fn version_supported(version: &str) -> bool {
    let mut parts = version.split(['.', '-', '+']);
    let number = |p: Option<&str>| p.and_then(|p| p.parse::<u64>().ok());
    matches!((number(parts.next()), number(parts.next())), (Some(a), Some(b)) if (a, b) >= MIN_SUPPORTED)
}

/// Inspect a user-chosen folder. Never writes, renames or follows symlinks
/// for the evidence files.
pub fn inspect_rct2_data(root: &Path) -> Rct2DataReport {
    let blocked = |root_identity, blocker: String| Rct2DataReport {
        root: root.to_path_buf(),
        root_identity,
        g1_identity: None,
        blocker: Some(blocker),
    };
    if !root.is_absolute() {
        return blocked(None, "the data folder path must be absolute".into());
    }
    let metadata = match fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(e) => return blocked(None, format!("the data folder cannot be read: {e}")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return blocked(None, "the data folder is not a real directory".into());
    }
    let root_identity = Some(CapturedFileIdentity::capture(&metadata));
    let data = root.join(G1_RELATIVE[0]);
    let g1 = data.join(G1_RELATIVE[1]);
    match fs::symlink_metadata(&g1) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() && m.len() > 0 => Rct2DataReport {
            root: root.to_path_buf(),
            root_identity,
            g1_identity: Some(CapturedFileIdentity::capture(&m)),
            blocker: None,
        },
        Ok(_) => blocked(
            root_identity,
            "Data/g1.dat is not a non-empty regular file".into(),
        ),
        Err(_) => {
            // OpenRCT2 on Linux is case-sensitive; say so rather than rename.
            let wrong_case = fs::read_dir(root).ok().is_some_and(|entries| {
                entries.flatten().any(|e| {
                    e.file_name().to_string_lossy().eq_ignore_ascii_case("data")
                        && e.file_name() != "Data"
                })
            }) || fs::read_dir(&data).ok().is_some_and(|entries| {
                entries.flatten().any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("g1.dat")
                        && e.file_name() != "g1.dat"
                })
            });
            let reason = if wrong_case {
                "Data/g1.dat was not found, but a differently capitalised name exists; OpenRCT2 \
                 on Linux needs exactly Data/g1.dat. EmuWiz does not rename your files"
            } else {
                "Data/g1.dat was not found; choose the folder that contains the Data folder of \
                 your RollerCoaster Tycoon 2 installation"
            };
            blocked(root_identity, reason.into())
        }
    }
}

/// Run the existing planner over the two independent evidence sets.
pub fn plan_openrct2(
    install: Option<&OpenRct2Install>,
    data: Option<&Rct2DataReport>,
) -> AlternativeLaunchProjection {
    let provider = AlternativeLaunchProvider {
        id: OPENRCT2_PROVIDER_ID.into(),
        name: "OpenRCT2".into(),
        kind: AlternativeLaunchKind::EngineReimplementation,
        version: install.and_then(|i| i.version.clone()),
        version_supported: install
            .and_then(|i| i.version.as_deref())
            .map(version_supported),
        installed: install.is_some(),
        supported_game_ids: [RCT2_GAME_ID.to_string()].into_iter().collect(),
        requirements: vec![AlternativeLaunchRequirement {
            asset_kind: AlternativeLaunchAssetKind::GameDataDirectory,
            role: "RollerCoaster Tycoon 2 data folder (Data/g1.dat)".into(),
            edition: None,
        }],
    };
    let declared = data.is_some();
    let game = AlternativeLaunchGameIdentity {
        canonical_id: declared.then(|| RCT2_GAME_ID.to_string()),
        edition: None,
        confidence: if declared {
            AlternativeLaunchIdentityConfidence::Exact
        } else {
            AlternativeLaunchIdentityConfidence::Unknown
        },
    };
    let assets = data
        .map(|report| AlternativeLaunchAssetEvidence {
            asset_kind: AlternativeLaunchAssetKind::GameDataDirectory,
            identity: Some(RCT2_GAME_ID.into()),
            edition: None,
            present: report.blocker.is_none(),
            source: "user-selected folder inspected by EmuWiz".into(),
            reason: report
                .blocker
                .clone()
                .unwrap_or_else(|| "Data/g1.dat is present".into()),
        })
        .into_iter()
        .collect();
    project_provider(provider, game, assets)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRct2LaunchRequest {
    pub install: OpenRct2Install,
    pub data: Rct2DataReport,
}

#[derive(Debug)]
pub enum OpenRct2LaunchError {
    /// The planner no longer reports `Ready` (or never did).
    NotReady(AlternativeLaunchReadiness, String),
    /// What was shown to the user is not what is on disk now.
    Changed(String),
    Spawn(std::io::Error),
}

/// A validated command together with the descriptor it must be executed by.
#[derive(Debug)]
pub struct PreparedOpenRct2Launch {
    pub command: PreparedProcessCommand,
    pinned: PinnedOpenRct2,
}

/// Fresh, click-time revalidation. Builds the exact argv only on success and
/// keeps the verified executable open so it, and nothing at the path, runs.
pub fn preflight_openrct2_launch(
    request: &OpenRct2LaunchRequest,
    search_dirs: &[PathBuf],
) -> Result<PreparedOpenRct2Launch, OpenRct2LaunchError> {
    use OpenRct2LaunchError::*;
    let fresh_data = inspect_rct2_data(&request.data.root);
    let checked = &request.install;
    let (Some(version), Some(sha)) = (checked.version.clone(), checked.content_sha256) else {
        return Err(NotReady(
            AlternativeLaunchReadiness::RequirementsUnknown,
            "this OpenRCT2 has not been checked".into(),
        ));
    };
    // Passive rediscovery; exactly one candidate at the checked path.
    let matches: Vec<_> = discover_openrct2(search_dirs)
        .into_iter()
        .filter(|i| i.executable == checked.executable)
        .collect();
    let [fresh] = matches.as_slice() else {
        let projection = plan_openrct2(None, Some(&fresh_data));
        return Err(NotReady(projection.readiness, projection.reason));
    };
    // Pin the file now. The checked version is evidence about these exact
    // bytes only: identity and content hash must both still match.
    let pinned = PinnedOpenRct2::open(&fresh.executable).map_err(Changed)?;
    if pinned.identity != checked.executable_identity || pinned.sha256 != sha {
        return Err(Changed(
            "the OpenRCT2 executable changed since it was checked".into(),
        ));
    }
    let install = OpenRct2Install {
        version: Some(version),
        content_sha256: Some(sha),
        ..fresh.clone()
    };
    let projection = plan_openrct2(Some(&install), Some(&fresh_data));
    if projection.readiness != AlternativeLaunchReadiness::Ready {
        return Err(NotReady(projection.readiness, projection.reason));
    }
    let data = fresh_data;
    if data.root_identity != request.data.root_identity
        || data.g1_identity != request.data.g1_identity
    {
        return Err(Changed("the RollerCoaster Tycoon 2 data changed".into()));
    }
    pinned.confirm_unchanged().map_err(Changed)?;
    let mut option = OsString::from("--rct2-data-path=").into_vec();
    option.extend(data.root.as_os_str().to_owned().into_vec());
    Ok(PreparedOpenRct2Launch {
        command: PreparedProcessCommand {
            executable: install.executable,
            arguments: vec![OsString::from_vec(option)],
            working_directory: None,
        },
        pinned,
    })
}

pub fn preflight_and_launch_openrct2(
    request: &OpenRct2LaunchRequest,
    search_dirs: &[PathBuf],
) -> Result<WatchedProcess, OpenRct2LaunchError> {
    let prepared = preflight_openrct2_launch(request, search_dirs)?;
    spawn_prepared_openrct2(&prepared).map_err(OpenRct2LaunchError::Spawn)
}

/// Executes the pinned descriptor. The child keeps the parent's environment
/// (a game needs DISPLAY, HOME, XDG_*), but every other descriptor is closed.
pub fn spawn_prepared_openrct2(
    prepared: &PreparedOpenRct2Launch,
) -> std::io::Result<WatchedProcess> {
    let environment: Vec<_> = std::env::vars_os().collect();
    prepared
        .pinned
        .confirm_unchanged()
        .map_err(std::io::Error::other)?;
    spawn_watched_process_private_pinned(
        &prepared.command,
        &environment,
        &prepared.pinned.file,
        |_| {},
    )
}

#[cfg(test)]
mod tests;
