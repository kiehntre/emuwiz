//! First supported slice of real RetroArch launch execution: safely
//! revalidating and spawning exactly one native RetroArch process for one
//! direct, loose, regular content file.
//!
//! # Scope (Phase 1)
//!
//! - Native RetroArch profiles only ([`ProfileKind::Native`]) - Flatpak and
//!   AppImage are refused outright, never attempted.
//! - One direct loose regular content file
//!   ([`LaunchContainerKind::PlainFile`], `requires_mount == false`) - no
//!   archive members, no mounted content, no CUE/BIN multi-file handling.
//! - Strictly [`LaunchReadiness::Ready`] - `ReadyWithWarnings` and
//!   `Blocked` are both refused, never silently accepted.
//! - Exactly one requested RetroArch core candidate, matched by profile +
//!   core stem + platform id - never a silent substitution of a different
//!   candidate the plan happened to also find.
//!
//! # What this module is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never launches a standalone emulator.
//! - It never touches Dolphin mods, cheats, RomM, DAT, Library View
//!   History, ES-DE writes, or the shared transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`],
//!   never `sh -c` and never one concatenated command string. See
//!   [`spawn_retroarch`].
//! - It never re-derives argv itself - the exact executable/`-L`/core/
//!   content argument list always comes from
//!   [`crate::launch::retroarch_command::build_retroarch_command_plan`],
//!   rebuilt fresh from a freshly re-validated identity/content/environment
//!   (see [`preflight_retroarch_launch`]'s own doc comment for exactly why
//!   the old readiness report is never trusted alone).
//! - It never adds an automatic timeout, kill, or relaunch - RetroArch is a
//!   long-running, user-facing process the caller owns; see
//!   [`LaunchedRetroArchProcess`]'s own doc comment.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::SystemTime;

use crate::emulator_environment::ReadOnlyHostFilesystem;
use crate::emulator_environment::retroarch::{
    DiscoveryEnvironment, DiscoveryError, ProfileKind, ProfileRef, discover_retroarch_environment,
};
use crate::game_identity::inspect_catalogued_game_identity;
use crate::launch::evidence_bridge::canonical_identity_from_game_report;
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentRef, LaunchTarget, build_launch_plan,
};
use crate::launch::readiness::LaunchReadiness;
use crate::launch::retroarch_command::{RetroArchCommand, build_retroarch_command_plan};

/// Caps how much of a launched process's stderr this module ever retains
/// in memory - a diagnostic aid for a failed/crashed launch, never a full
/// log. Matches the existing `64 KiB` bound this crate already uses for
/// subprocess output elsewhere (`dat::archive::external_process`,
/// `run_command_os_with_timeout`).
pub const LAUNCH_STDERR_CAPTURE_LIMIT: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized RetroArch launch is being requested. Never an arbitrary
/// command string - every field here only ever *selects* which
/// already-existing, already-reviewed [`crate::launch::planning::LaunchCandidate`]
/// to revalidate and launch; none of it is ever passed to a shell or used
/// to build argv directly (see [`preflight_retroarch_launch`]).
///
/// `expected_platform_id` and `expected_game_key` together are the
/// "expected canonical identity" the caller already approved (typically
/// from an earlier [`crate::launch::planning::CanonicalIdentityStatus::Resolved`]
/// the user was shown) - re-checked fresh at preflight time, never trusted
/// from the moment this request was built. `expected_platform_id` doubles
/// as the platform the requested candidate's own
/// [`LaunchTarget::RetroArchCore::platform_id`] must match: in this
/// single-resolved-identity pipeline the two are always the same value by
/// construction, so this module deliberately carries one field rather than
/// two that could silently drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroArchLaunchRequest {
    /// The exact, direct content file the user selected - never an outer
    /// archive path and never a mount point.
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    /// Which discovered RetroArch profile (native/AppImage/Flatpak x
    /// user/system) the candidate must belong to.
    pub profile: ProfileRef,
    /// Which discovered core (by its `.info`/library stem) the candidate
    /// must use.
    pub core_stem: String,
}

// ---------------------------------------------------------------------------
// Content identity
// ---------------------------------------------------------------------------

/// A launch-specific, point-in-time filesystem identity for the content
/// file being launched - captured once during preflight and re-checked
/// immediately before spawn (step 9), so a swap of the file at the same
/// path between those two moments is detected rather than silently
/// launched. Mirrors the `(device, inode, size, mtime)` shape this crate
/// already uses for the same purpose elsewhere (`shared_transaction`'s
/// `same_file`, `SharedDirectoryIdentity`; `dolphin_local`'s
/// `DolphinDirectoryIdentity`) - device/inode are the *only* platform-
/// specific part (`0` on non-Unix, where they carry no comparable meaning),
/// size and modification time are always real.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchContentIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl LaunchContentIdentity {
    fn capture(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        let (device, inode) = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let (device, inode) = (0u64, 0u64);
        Self {
            device,
            inode,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchPreflightErrorKind {
    /// The requested content path is not absolute.
    ContentPathNotAbsolute,
    /// The requested content path does not exist or could not be
    /// inspected.
    ContentNotFound,
    /// The requested content path is a symlink.
    ContentIsSymlink,
    /// The requested content path is not a regular file (directory,
    /// device, etc).
    ContentNotRegularFile,
    /// The requested content path is itself a mount-input archive
    /// (zip/7z/rar) - an outer archive path is never a runnable content
    /// path in this module, exactly like the rest of the launch pipeline.
    ContentRequiresMount,
    /// Only [`ProfileKind::Native`] is supported in this phase - both
    /// Flatpak and AppImage profiles are refused here, before any plan is
    /// even built.
    UnsupportedProfileKind,
    /// Fresh re-inspection produced `Unknown` or `Conflicting` identity -
    /// never resolved to one trustworthy answer.
    IdentityUnresolved,
    /// Fresh identity resolved, but its platform id or game key differs
    /// from what the request expected - the content at this path is not
    /// the game the user approved.
    IdentityMismatch,
    /// Fresh RetroArch environment discovery itself failed (e.g. no home
    /// directory could be determined).
    DiscoveryFailed,
    /// No candidate in the freshly rebuilt plan matches the requested
    /// profile/core stem/platform id - never substituted with a different
    /// candidate the plan happened to also contain.
    RequestedCandidateNotFound,
    /// The matched candidate's own readiness is not exactly
    /// [`LaunchReadiness::Ready`] - covers both `ReadyWithWarnings` and
    /// `Blocked`.
    CandidateNotReady,
    /// The matched candidate's content is not the narrow, direct,
    /// non-mounted plain-file shape this phase supports, even though its
    /// readiness reported `Ready` - defense in depth against a future
    /// planner change silently widening what counts as "Ready".
    CandidateContentUnsupported,
    /// [`build_retroarch_command_plan`] itself reported blockers.
    CommandBlocked,
    /// [`build_retroarch_command_plan`] reported no blockers but also no
    /// command - should be unreachable given the checks above, but never
    /// assumed.
    CommandMissing,
    /// The resolved executable no longer exists.
    ExecutableMissing,
    /// The resolved executable is a symlink or not a regular file.
    ExecutableUnsafe,
    /// The resolved executable is not marked executable.
    ExecutableNotExecutable,
    /// The resolved core library no longer exists.
    CoreMissing,
    /// The resolved core library is a symlink or not a regular file.
    CoreUnsafe,
    /// The content file's filesystem identity at the final pre-spawn check
    /// no longer matches what was captured earlier in this same preflight
    /// call - the file was swapped underneath the launch.
    ContentChangedBeforeSpawn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPreflightError {
    pub kind: LaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: LaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> LaunchPreflightError {
    LaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum LaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum LaunchExecutionError {
    Preflight(LaunchPreflightError),
    Spawn(LaunchSpawnError),
}

impl From<LaunchPreflightError> for LaunchExecutionError {
    fn from(error: LaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<LaunchSpawnError> for LaunchExecutionError {
    fn from(error: LaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`RetroArchCommand`] safe to spawn - or refuses with a
/// [`LaunchPreflightError`] naming exactly why.
///
/// # Why nothing from before this call is trusted
///
/// A [`crate::launch::planning::LaunchPlan`]/readiness report the user saw
/// earlier (e.g. on the Selected page) may be arbitrarily stale by the time
/// they click Launch: the content file could have been deleted, replaced,
/// or moved; RetroArch could have been uninstalled, updated, or had a core
/// removed; the disc/cartridge identity itself could have changed if the
/// path was reused for different content. This function repeats every
/// identity/content/environment inspection fresh, using exactly the same
/// authoritative machinery the rest of this crate already trusts
/// ([`inspect_game_identity`], [`discover_retroarch_environment`],
/// [`build_launch_plan`], [`build_retroarch_command_plan`]) - it never
/// re-derives or shortcuts any of their own logic.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, be a regular file, and not
///    be an outer archive/mount-input path (`crate::archive_kind` reports
///    a mount-input [`crate::ArchiveKind`]).
/// 3. A [`LaunchContentIdentity`] is captured from the content's current
///    metadata.
/// 4. `request.profile.profile_kind` must be [`ProfileKind::Native`] -
///    Flatpak and AppImage are refused here, before any plan exists.
/// 5. The content is freshly re-identified via [`inspect_game_identity`]
///    (never the caller's old report) and converted through
///    [`canonical_identity_from_game_report`]; the result must be
///    `Resolved` with exactly `request.expected_platform_id`/
///    `expected_game_key` - `Unknown`, `Conflicting`, or any mismatch is
///    refused.
/// 6. The RetroArch environment is freshly rediscovered via
///    [`discover_retroarch_environment`] - never the caller's cached
///    discovery.
/// 7. [`build_launch_plan`] is rebuilt from the fresh identity, a
///    [`LaunchContentRef`] describing exactly this direct, non-mounted
///    plain file, the fresh environment, and no standalone profiles/
///    remembered preferences (out of scope for this phase). The candidate
///    matching `request.profile`/`core_stem`/`expected_platform_id` is
///    found - never substituted with a different one.
/// 8. That candidate's `readiness` must be exactly [`LaunchReadiness::Ready`]
///    and its content must still be the narrow direct-plain-file shape
///    this phase supports.
/// 9. [`build_retroarch_command_plan`] is rebuilt from the fresh identity/
///    candidate/environment; it must report no blockers and a command.
/// 10. Immediately before returning: the executable and core library are
///     re-checked to still exist, not be symlinks, and be regular files
///     (the executable must also still be marked executable); the content
///     is re-inspected once more and its [`LaunchContentIdentity`] must
///     still equal the one captured in step 3.
pub fn preflight_retroarch_launch(
    request: &RetroArchLaunchRequest,
    filesystem: &dyn ReadOnlyHostFilesystem,
    environment: &DiscoveryEnvironment,
) -> Result<RetroArchCommand, LaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            LaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: profile kind gate (Flatpak/AppImage refused outright) ---
    if request.profile.profile_kind != ProfileKind::Native {
        return Err(preflight_error(
            LaunchPreflightErrorKind::UnsupportedProfileKind,
            format!(
                "only native RetroArch profiles are supported in this phase, got {:?}",
                request.profile.profile_kind
            ),
        ));
    }

    // --- 5: fresh identity re-inspection ---
    let identity_status = fresh_identity_status(content_path, request)?;

    // --- 6: fresh environment discovery ---
    let fresh_environment = discover_retroarch_environment(filesystem, environment).map_err(
        |error: DiscoveryError| {
            preflight_error(LaunchPreflightErrorKind::DiscoveryFailed, error.to_string())
        },
    )?;

    // --- 7: rebuild the plan, find the exact requested candidate ---
    let content_ref = LaunchContentRef {
        kind: None,
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(content_path.clone()),
        requires_mount: false,
        provenance: "launch execution preflight: revalidated direct regular file".to_string(),
    };
    let plan = build_launch_plan(&identity_status, &content_ref, &[], &fresh_environment, &[]);
    let candidate = plan
        .candidates
        .iter()
        .find(|candidate| match &candidate.target {
            LaunchTarget::RetroArchCore {
                profile,
                core_stem,
                platform_id,
            } => {
                *profile == request.profile
                    && *core_stem == request.core_stem
                    && *platform_id == request.expected_platform_id
            }
            LaunchTarget::Standalone { .. } => false,
        })
        .ok_or_else(|| {
            preflight_error(
                LaunchPreflightErrorKind::RequestedCandidateNotFound,
                "no candidate in the freshly rebuilt plan matches the requested profile/core/\
                 platform",
            )
        })?;

    // --- 8: strict readiness + content-shape gate ---
    if candidate.readiness != LaunchReadiness::Ready {
        return Err(preflight_error(
            LaunchPreflightErrorKind::CandidateNotReady,
            format!(
                "requested candidate readiness is {:?}, not exactly Ready",
                candidate.readiness
            ),
        ));
    }
    if candidate.content.container != Some(LaunchContainerKind::PlainFile)
        || candidate.content.requires_mount
        || !candidate.content.has_runnable_path()
    {
        return Err(preflight_error(
            LaunchPreflightErrorKind::CandidateContentUnsupported,
            "candidate content is not a direct, non-mounted plain file",
        ));
    }

    // --- 9: rebuild the command plan ---
    let command_plan =
        build_retroarch_command_plan(&identity_status, candidate, &fresh_environment);
    if !command_plan.blockers.is_empty() {
        return Err(preflight_error(
            LaunchPreflightErrorKind::CommandBlocked,
            format!(
                "command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    let command = command_plan.command.ok_or_else(|| {
        preflight_error(
            LaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    recheck_core_library(&command.selection.core_library)?;
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            LaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content file changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<LaunchContentIdentity, LaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            LaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            LaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(preflight_error(
            LaunchPreflightErrorKind::ContentNotRegularFile,
            "content path is not a regular file",
        ));
    }
    if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
        return Err(preflight_error(
            LaunchPreflightErrorKind::ContentRequiresMount,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    Ok(LaunchContentIdentity::capture(&metadata))
}

fn fresh_identity_status(
    content_path: &Path,
    request: &RetroArchLaunchRequest,
) -> Result<CanonicalIdentityStatus, LaunchPreflightError> {
    // `inspect_catalogued_game_identity`, not the plain `inspect_game_identity` -
    // this request only ever exists because the game was already
    // catalogued/identified earlier (that identification is exactly what
    // `expected_platform_id`/`expected_game_key` came from); re-inspecting
    // with `trusted_platform = true` is the same "explicit opt-in at the
    // catalogue boundary" its own doc comment requires, not a filename
    // guess - the platform hint here is the caller's already-approved
    // identity, never derived from this path's name or extension.
    let report =
        inspect_catalogued_game_identity(content_path, Some(&request.expected_platform_id));
    let (identity_status, _facts) = canonical_identity_from_game_report(&report);
    match &identity_status {
        CanonicalIdentityStatus::Resolved(resolved) => {
            if resolved.platform_id != request.expected_platform_id
                || resolved.game_key != request.expected_game_key
            {
                return Err(preflight_error(
                    LaunchPreflightErrorKind::IdentityMismatch,
                    format!(
                        "resolved identity {}/{} does not match expected {}/{}",
                        resolved.platform_id,
                        resolved.game_key,
                        request.expected_platform_id,
                        request.expected_game_key
                    ),
                ));
            }
        }
        CanonicalIdentityStatus::Unknown | CanonicalIdentityStatus::Conflicting => {
            return Err(preflight_error(
                LaunchPreflightErrorKind::IdentityUnresolved,
                format!("fresh identity re-inspection produced {identity_status:?}"),
            ));
        }
    }
    Ok(identity_status)
}

fn recheck_executable(path: &Path) -> Result<(), LaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            LaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            LaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                LaunchPreflightErrorKind::ExecutableNotExecutable,
                "executable has no execute bit set",
            ));
        }
    }
    Ok(())
}

fn recheck_core_library(path: &Path) -> Result<(), LaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            LaunchPreflightErrorKind::CoreMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            LaunchPreflightErrorKind::CoreUnsafe,
            "core library is a symlink or not a regular file",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

/// The exact facts about a launched process a future GUI needs to render
/// state with, captured once at spawn time - never re-derived from the
/// live process afterward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile: ProfileRef,
    pub core_stem: String,
    pub platform_id: String,
    pub content_path: PathBuf,
}

fn command_facts(command: &RetroArchCommand) -> LaunchCommandFacts {
    LaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile: command.selection.profile,
        core_stem: command.selection.core_stem.clone(),
        platform_id: command.selection.platform_id.clone(),
        content_path: command.selection.content_path.clone(),
    }
}

/// What the background reaper thread reports once the process has exited.
#[derive(Debug)]
pub struct LaunchExitReport {
    /// `Err` only when `wait()` on the child itself failed (not when the
    /// process exited non-zero - that is a normal, successful `Ok(status)`
    /// with `status.success() == false`).
    pub status: std::io::Result<ExitStatus>,
    /// Bounded (see [`LAUNCH_STDERR_CAPTURE_LIMIT`]) capture of the
    /// process's stderr, for diagnosing a failed/crashed launch. Never a
    /// full, unbounded log.
    pub stderr: Vec<u8>,
}

/// A spawned, still-owned RetroArch process. Never automatically killed,
/// timed out, or relaunched by this module - RetroArch is a long-running,
/// user-facing program the caller (a future GUI) owns for as long as the
/// user wants it running. [`Self::poll`] is the narrow, non-blocking way to
/// notice it has exited, backed by a background thread that only ever
/// drains stderr and waits - it never sends a signal.
pub struct LaunchedRetroArchProcess {
    pub pid: u32,
    pub command_facts: LaunchCommandFacts,
    receiver: Receiver<LaunchExitReport>,
    exit_report: Option<LaunchExitReport>,
}

impl LaunchedRetroArchProcess {
    /// Non-blocking: returns the exit report once the background reaper
    /// thread has observed the process exit, `None` while it is still
    /// running. Safe to call every GUI frame.
    pub fn poll(&mut self) -> Option<&LaunchExitReport> {
        if self.exit_report.is_none() {
            match self.receiver.try_recv() {
                Ok(report) => self.exit_report = Some(report),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {}
            }
        }
        self.exit_report.as_ref()
    }

    pub fn is_running(&self) -> bool {
        self.exit_report.is_none()
    }
}

/// Spawns exactly the process `command` describes - never a shell, never
/// one concatenated command string. `command` must already have passed
/// [`preflight_retroarch_launch`]; this function performs no further
/// validation of its own beyond what [`std::process::Command`] itself
/// requires to spawn.
///
/// - `Command::new(&command.executable)` + `.args(&command.arguments)` -
///   every argument (including the content path) is passed as its own
///   `OsString`, so spaces, quotes, and shell metacharacters in a path are
///   inert data, never re-parsed as syntax.
/// - `stdin` is `Stdio::null()` - RetroArch never needs to read from this
///   process's stdin.
/// - `stdout` is `Stdio::null()` - RetroArch is a graphical process; it
///   does not need its own stdout consumed by this process, and inheriting
///   it would tie this process's terminal to RetroArch's for no benefit.
/// - `stderr` is `Stdio::piped()` and drained on a background thread,
///   bounded at [`LAUNCH_STDERR_CAPTURE_LIMIT`], purely as a diagnostic aid
///   if the launch fails or crashes.
/// - No environment variables are injected or overridden - the spawned
///   process inherits this process's environment exactly (display/XDG/
///   RetroArch environment stays intact), and nothing user-derived is ever
///   added to it.
/// - The working directory is exactly `command.working_directory` (`None`
///   today, per the command planner - inherited from this process, not set
///   to some new directory).
/// - No timeout, no automatic kill: see [`LaunchedRetroArchProcess`]'s own
///   doc comment.
pub fn spawn_retroarch(
    command: RetroArchCommand,
) -> Result<LaunchedRetroArchProcess, LaunchSpawnError> {
    let facts = command_facts(&command);
    let mut process = Command::new(&command.executable);
    process.args(&command.arguments);
    process.stdin(Stdio::null());
    process.stdout(Stdio::null());
    process.stderr(Stdio::piped());
    if let Some(working_directory) = &command.working_directory {
        process.current_dir(working_directory);
    }
    let mut child: Child = process.spawn().map_err(LaunchSpawnError::Spawn)?;
    let pid = child.id();
    let stderr = child.stderr.take();

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let stderr_bytes = stderr.map(read_bounded_stderr).unwrap_or_default();
        let status = child.wait();
        let _ = sender.send(LaunchExitReport {
            status,
            stderr: stderr_bytes,
        });
    });

    Ok(LaunchedRetroArchProcess {
        pid,
        command_facts: facts,
        receiver,
        exit_report: None,
    })
}

fn read_bounded_stderr(mut stderr: impl Read) -> Vec<u8> {
    let mut buffer = vec![0u8; LAUNCH_STDERR_CAPTURE_LIMIT];
    let mut filled = 0usize;
    while filled < buffer.len() {
        match stderr.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => break,
        }
    }
    buffer.truncate(filled);
    // Drain and discard anything past the cap, so a chatty process still
    // exits cleanly (an unread, full pipe can otherwise block the child on
    // write) without this module ever retaining more than the bound.
    let mut discard = [0u8; 4096];
    while matches!(stderr.read(&mut discard), Ok(read) if read > 0) {}
    buffer
}

// ---------------------------------------------------------------------------
// Convenience: preflight + spawn in one call
// ---------------------------------------------------------------------------

/// Composes [`preflight_retroarch_launch`] and [`spawn_retroarch`] - the
/// single call a future GUI Launch button would make. Kept as two separate
/// public functions above so preflight-only rejection scenarios can be
/// tested without ever spawning a real process.
pub fn preflight_and_launch_retroarch(
    request: &RetroArchLaunchRequest,
    filesystem: &dyn ReadOnlyHostFilesystem,
    environment: &DiscoveryEnvironment,
) -> Result<LaunchedRetroArchProcess, LaunchExecutionError> {
    let command = preflight_retroarch_launch(request, filesystem, environment)?;
    Ok(spawn_retroarch(command)?)
}

#[cfg(test)]
mod tests;
