//! First supported slice of real native Dolphin launch execution: safely
//! revalidating and spawning exactly one native Dolphin process for one
//! direct, loose, regular GameCube content file.
//!
//! # Scope (Phase 2, first slice)
//!
//! - Native Dolphin profiles only - Flatpak, AppImage, and any profile
//!   [`crate::patch_manager::resolve_dolphin_native_launch_binding`] itself
//!   refuses are never attempted.
//! - Direct loose regular `.iso`/`.gcm`/`.rvz`/`.ciso`/`.wbfs` content - no
//!   archive members or mounted content.
//! - Strictly [`LaunchReadiness::Ready`] - `ReadyWithWarnings` and
//!   `Blocked` are both refused, never silently accepted.
//! - Exactly one requested, already-discovered Dolphin profile, matched by
//!   profile id - never a silent substitution of a different profile or
//!   executable the fresh discovery happened to also find.
//!
//! # What this module is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never launches Flatpak/AppImage Dolphin, PCSX2, PPSSPP, or
//!   DuckStation.
//! - It never touches Dolphin texture mods, cheats, RomM, DAT, Library View
//!   History, ES-DE writes, or the shared transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/`-u`/`-e`
//!   argument list always comes from
//!   [`crate::launch::dolphin_command::build_dolphin_command_plan`], rebuilt
//!   fresh from a freshly re-validated identity/content/profile/binding (see
//!   [`preflight_dolphin_launch`]'s own doc comment for exactly why the old
//!   readiness a caller may have seen earlier is never trusted alone).
//! - It never adds an automatic timeout, kill, or relaunch - Dolphin is a
//!   long-running, user-facing process the caller owns; see
//!   [`LaunchedDolphinProcess`]'s own doc comment.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::game_identity::inspect_catalogued_game_identity;
use crate::launch::dolphin_command::{
    DolphinCommand, build_dolphin_command_plan, direct_dolphin_extension,
    dolphin_supported_platform,
};
use crate::launch::evidence_bridge::canonical_identity_from_game_report;
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    LaunchTarget, StandaloneProfileInput, build_launch_plan,
};
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, ProcessExitReport, WatchedProcess,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::patch_manager::{
    DolphinLocalDiscoveryRoots, DolphinUserDirectoryMode, discover_dolphin_local_profiles,
    resolve_dolphin_native_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native Dolphin launch is being requested. Never an
/// arbitrary command string - every field here only ever *selects* which
/// already-discovered profile/binding to revalidate and launch; none of it
/// is ever passed to a shell or used to build argv directly (see
/// [`preflight_dolphin_launch`]).
///
/// `expected_game_id` is the verified GameCube disc-header Game ID the
/// caller already approved (from an earlier
/// [`CanonicalIdentityStatus::Resolved`] the user was shown) - re-checked
/// fresh at preflight time, never trusted from the moment this request was
/// built.
///
/// `expected_executable`/`expected_user_directory_mode` are the exact launch
/// binding facts the user was shown at readiness time. A freshly resolved
/// binding that differs from either is treated as drift and refused rather
/// than silently substituted - see step 7 of [`preflight_dolphin_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinLaunchRequest {
    /// The exact, direct content file the user selected - never an outer
    /// archive path and never a mount point.
    pub selected_content_path: PathBuf,
    pub expected_game_id: String,
    /// Which discovered [`crate::patch_manager::DolphinLocalProfile::profile_id`]
    /// the binding must belong to.
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub expected_user_directory_mode: DolphinUserDirectoryMode,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DolphinLaunchPreflightErrorKind {
    /// The requested content path is not absolute.
    ContentPathNotAbsolute,
    /// The requested content path does not exist or could not be
    /// inspected.
    ContentNotFound,
    /// The requested content path is a symlink.
    ContentIsSymlink,
    /// The requested content path is not a regular file.
    ContentNotRegularFile,
    /// The requested content path is itself a mount-input archive
    /// (zip/7z/rar) - an outer archive path is never a runnable content
    /// path in this module.
    ContentRequiresMount,
    /// The requested content path is not a direct `.iso`/`.gcm` file - no
    /// RVZ/CISO/WBFS in this phase.
    ContentFormatUnsupported,
    /// Fresh re-inspection produced `Unknown` or `Conflicting` identity -
    /// never resolved to one trustworthy answer.
    IdentityUnresolved,
    /// Fresh identity resolved, but its platform or Game ID differs from
    /// what the request expected - the content at this path is not the
    /// game the user approved, or it is not `GameCube`.
    IdentityMismatch,
    /// No discovered Dolphin profile matches
    /// [`DolphinLaunchRequest::profile_id`] - never substituted with a
    /// different profile.
    ProfileNotFound,
    /// [`resolve_dolphin_native_launch_binding`] itself refused to produce a
    /// binding for the matched profile.
    BindingUnavailable,
    /// The freshly resolved binding's executable/user-directory mode no
    /// longer matches what the request expected - the binding drifted
    /// between readiness time and this click, so it is never silently
    /// substituted.
    BindingDrift,
    /// No candidate in the freshly rebuilt plan matches the requested
    /// Dolphin profile.
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
    /// [`build_dolphin_command_plan`] itself reported blockers.
    CommandBlocked,
    /// [`build_dolphin_command_plan`] reported no blockers but also no
    /// command - should be unreachable given the checks above, but never
    /// assumed.
    CommandMissing,
    /// The resolved executable no longer exists.
    ExecutableMissing,
    /// The resolved executable is a symlink or not a regular file.
    ExecutableUnsafe,
    /// The resolved executable is not marked executable.
    ExecutableNotExecutable,
    /// The resolved explicit Dolphin user root no longer exists, is not a
    /// directory, or has become a symlink.
    ExplicitRootInvalid,
    /// The content file's filesystem identity at the final pre-spawn check
    /// no longer matches what was captured earlier in this same preflight
    /// call - the file was swapped underneath the launch.
    ContentChangedBeforeSpawn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinLaunchPreflightError {
    pub kind: DolphinLaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: DolphinLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> DolphinLaunchPreflightError {
    DolphinLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum DolphinLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum DolphinLaunchExecutionError {
    Preflight(DolphinLaunchPreflightError),
    Spawn(DolphinLaunchSpawnError),
}

impl From<DolphinLaunchPreflightError> for DolphinLaunchExecutionError {
    fn from(error: DolphinLaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<DolphinLaunchSpawnError> for DolphinLaunchExecutionError {
    fn from(error: DolphinLaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`DolphinCommand`] safe to spawn - or refuses with a
/// [`DolphinLaunchPreflightError`] naming exactly why.
///
/// # Why nothing from before this call is trusted
///
/// See [`crate::launch::execution::preflight_retroarch_launch`]'s own doc
/// comment for the general rationale; the same reasoning applies here -
/// the content file, the Dolphin installation, and even the profile's
/// user-directory layout can all have changed between whenever the user was
/// shown "Ready" and the moment they click Launch.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, be a regular file, not be
///    an outer archive/mount-input path, and have a `.iso`/`.gcm`
///    extension.
/// 3. A [`CapturedFileIdentity`] is captured from the content's current
///    metadata.
/// 4. The content is freshly re-identified via
///    [`inspect_catalogued_game_identity`] (never a caller-supplied old
///    report); the result must resolve to exactly
///    a supported Dolphin platform and `request.expected_game_id`.
/// 5. Dolphin local profiles are freshly rediscovered via
///    [`discover_dolphin_local_profiles`] - never a caller's cached
///    discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_dolphin_native_launch_binding`] is called fresh against that
///    profile; its executable and user-directory mode must exactly equal
///    `request.expected_executable`/`expected_user_directory_mode`.
/// 8. [`build_launch_plan`] is rebuilt from the fresh identity, a
///    [`LaunchContentRef`] describing exactly this direct, non-mounted
///    plain file, and a single Dolphin [`StandaloneProfileInput`] projected
///    from the matched profile (no BIOS/firmware requirement - GameCube
///    optical discs need none). The resulting candidate must be exactly
///    [`LaunchReadiness::Ready`] and still the narrow direct-plain-file
///    shape this phase supports.
/// 9. [`build_dolphin_command_plan`] is rebuilt from the fresh identity/
///    candidate/binding; it must report no blockers and a command.
/// 10. Immediately before returning: the executable is re-checked to still
///     exist, not be a symlink, be a regular file, and be marked
///     executable; an explicit user-directory root (if present) is
///     re-checked to still exist, be a directory, and not be a symlink; the
///     content is re-inspected once more and its [`CapturedFileIdentity`]
///     must still equal the one captured in step 3.
pub fn preflight_dolphin_launch(
    request: &DolphinLaunchRequest,
    roots: &DolphinLocalDiscoveryRoots,
) -> Result<DolphinCommand, DolphinLaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: fresh identity re-inspection ---
    let identity_status = fresh_identity_status(content_path, request)?;

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_dolphin_local_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                DolphinLaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered Dolphin profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_dolphin_native_launch_binding(profile, roots);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            DolphinLaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable
        || binding.user_directory_mode != request.expected_user_directory_mode
    {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized \
             executable/user-directory mode",
        ));
    }

    // --- 8: rebuild the plan, find the exact requested candidate ---
    let content_ref = LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(content_path.clone()),
        requires_mount: false,
        provenance: "dolphin launch execution preflight: revalidated direct regular file"
            .to_string(),
    };
    let standalone_input = StandaloneProfileInput {
        adapter_id: "dolphin",
        profile_id: profile.profile_id.clone(),
        profile_path: Some(profile.configuration_root.clone()),
        eligible: profile.eligible,
        firmware: FirmwareReadiness::NotRequired,
    };
    let empty_retroarch = RetroArchEnvironmentReport {
        format_version: 1,
        profiles: Vec::new(),
        diagnostics: Vec::new(),
    };
    let plan = build_launch_plan(
        &identity_status,
        &content_ref,
        std::slice::from_ref(&standalone_input),
        &empty_retroarch,
        &[],
    );
    let candidate = plan
        .candidates
        .iter()
        .find(|candidate| match &candidate.target {
            LaunchTarget::Standalone {
                adapter_id,
                profile_id,
                ..
            } => *adapter_id == "dolphin" && *profile_id == request.profile_id,
            LaunchTarget::RetroArchCore { .. } => false,
        })
        .ok_or_else(|| {
            preflight_error(
                DolphinLaunchPreflightErrorKind::RequestedCandidateNotFound,
                "no candidate in the freshly rebuilt plan matches the requested Dolphin profile",
            )
        })?;

    // --- strict readiness + content-shape gate ---
    if candidate.readiness != LaunchReadiness::Ready {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::CandidateNotReady,
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
            DolphinLaunchPreflightErrorKind::CandidateContentUnsupported,
            "candidate content is not a direct, non-mounted plain file",
        ));
    }

    // --- 9: rebuild the command plan ---
    let command_plan = build_dolphin_command_plan(&identity_status, candidate, &binding_result);
    if !command_plan.blockers.is_empty() {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::CommandBlocked,
            format!(
                "command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    let command = command_plan.command.ok_or_else(|| {
        preflight_error(
            DolphinLaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    if let DolphinUserDirectoryMode::ExplicitRoot(root) = &command.selection.user_directory_mode {
        recheck_explicit_root(root)?;
    }
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content file changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, DolphinLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            DolphinLaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ContentNotRegularFile,
            "content path is not a regular file",
        ));
    }
    if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ContentRequiresMount,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    if !direct_dolphin_extension(path) {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ContentFormatUnsupported,
            "only a direct .iso, .gcm, .rvz, .ciso, or .wbfs file is supported by this native Dolphin launch path",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity_status(
    content_path: &Path,
    request: &DolphinLaunchRequest,
) -> Result<CanonicalIdentityStatus, DolphinLaunchPreflightError> {
    // `inspect_catalogued_game_identity`, not the plain `inspect_game_identity` -
    // this request only ever exists because the game was already
    // catalogued/identified earlier; the platform hint here is the caller's
    // already-approved identity, never derived from this path's name or
    // extension.
    // The existing catalogued identity reader uses the platform hint for
    // minimal synthetic/legacy fixtures that do not carry enough container
    // bytes to infer a platform on their own. Try both Dolphin platforms in
    // turn; each reader still validates the actual identity and no filename
    // or extension is promoted into identity.
    let report = inspect_catalogued_game_identity(content_path, Some("GameCube"));
    let (mut identity_status, _facts) = canonical_identity_from_game_report(&report);
    if matches!(identity_status, CanonicalIdentityStatus::Unknown) {
        let wii_report = inspect_catalogued_game_identity(content_path, Some("Wii"));
        (identity_status, _) = canonical_identity_from_game_report(&wii_report);
    }
    match &identity_status {
        CanonicalIdentityStatus::Resolved(resolved) => {
            if !dolphin_supported_platform(&resolved.platform_id)
                || resolved.game_key != request.expected_game_id
            {
                return Err(preflight_error(
                    DolphinLaunchPreflightErrorKind::IdentityMismatch,
                    format!(
                        "resolved identity {}/{} does not match expected {}/{}",
                        resolved.platform_id,
                        resolved.game_key,
                        "GameCube or Wii",
                        request.expected_game_id
                    ),
                ));
            }
        }
        CanonicalIdentityStatus::Unknown | CanonicalIdentityStatus::Conflicting => {
            return Err(preflight_error(
                DolphinLaunchPreflightErrorKind::IdentityUnresolved,
                format!("fresh identity re-inspection produced {identity_status:?}"),
            ));
        }
    }
    Ok(identity_status)
}

fn recheck_executable(path: &Path) -> Result<(), DolphinLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            DolphinLaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                DolphinLaunchPreflightErrorKind::ExecutableNotExecutable,
                "executable has no execute bit set",
            ));
        }
    }
    Ok(())
}

fn recheck_explicit_root(root: &Path) -> Result<(), DolphinLaunchPreflightError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ExplicitRootInvalid,
            format!("{} is a symlink", root.display()),
        )),
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ExplicitRootInvalid,
            format!("{} is not a directory", root.display()),
        )),
        Err(io_error) => Err(preflight_error(
            DolphinLaunchPreflightErrorKind::ExplicitRootInvalid,
            format!("{}: {io_error}", root.display()),
        )),
    }
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

/// The exact facts about a launched process a future GUI needs to render
/// state with, captured once at spawn time - never re-derived from the live
/// process afterward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinLaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub user_directory_mode: DolphinUserDirectoryMode,
    pub platform_id: String,
    pub game_id: String,
    pub content_path: PathBuf,
}

fn command_facts(command: &DolphinCommand) -> DolphinLaunchCommandFacts {
    DolphinLaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        user_directory_mode: command.selection.user_directory_mode.clone(),
        platform_id: command.selection.platform_id.clone(),
        game_id: command.selection.game_id.clone(),
        content_path: command.selection.content_path.clone(),
    }
}

/// What the background watcher thread reports once the process has exited.
pub use crate::launch::process_spawn::ProcessExitReport as DolphinLaunchExitReport;

/// A spawned, still-owned Dolphin process. Never automatically killed, timed
/// out, or relaunched by this module - the Dolphin Qt process is a
/// long-running, user-facing program the caller (a future GUI) owns for as
/// long as the user wants it running. [`Self::poll`] is the narrow,
/// non-blocking way to notice it has exited.
pub struct LaunchedDolphinProcess {
    pub pid: u32,
    pub command_facts: DolphinLaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedDolphinProcess {
    /// Non-blocking: returns the exit report once the background watcher
    /// thread has observed the process exit, `None` while it is still
    /// running. Safe to call every GUI frame.
    pub fn poll(&mut self) -> Option<&ProcessExitReport> {
        self.watched.poll()
    }

    pub fn is_running(&self) -> bool {
        self.watched.is_running()
    }
}

/// Spawns exactly the process `command` describes - never a shell. `command`
/// must already have passed [`preflight_dolphin_launch`]; this function
/// performs no further validation of its own beyond what
/// [`crate::launch::process_spawn::spawn_watched_process`] itself requires
/// to spawn. See that function's own doc comment for the exact lifecycle
/// policy (stdin/stdout/stderr, environment, no timeout/kill).
pub fn spawn_dolphin(
    command: DolphinCommand,
) -> Result<LaunchedDolphinProcess, DolphinLaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(DolphinLaunchSpawnError::Spawn)?;
    Ok(LaunchedDolphinProcess {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

// ---------------------------------------------------------------------------
// Convenience: preflight + spawn in one call
// ---------------------------------------------------------------------------

/// Composes [`preflight_dolphin_launch`] and [`spawn_dolphin`] - the single
/// call a future GUI Launch button would make. Kept as two separate public
/// functions above so preflight-only rejection scenarios can be tested
/// without ever spawning a real process.
pub fn preflight_and_launch_dolphin(
    request: &DolphinLaunchRequest,
    roots: &DolphinLocalDiscoveryRoots,
) -> Result<LaunchedDolphinProcess, DolphinLaunchExecutionError> {
    let command = preflight_dolphin_launch(request, roots)?;
    Ok(spawn_dolphin(command)?)
}

#[cfg(test)]
mod tests;
