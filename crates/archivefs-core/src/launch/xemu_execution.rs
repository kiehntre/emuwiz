//! First supported slice of real native xemu launch execution: safely
//! revalidating and spawning exactly one native xemu process for one direct,
//! loose, regular original-Xbox disc image.
//!
//! # Scope (first slice)
//!
//! - Native xemu profiles only - any profile
//!   [`crate::patch_manager::resolve_xemu_native_launch_binding`] itself
//!   refuses is never attempted.
//! - `Xbox` only - the only platform
//!   [`crate::launch::xemu_command::XEMU_SUPPORTED_PLATFORM_ID`] names.
//! - One direct loose regular `.iso`/`.xiso` disc image - no archive
//!   members, no mounted content, no loose `.xbe`.
//! - A verified original-Xbox XBE title ID is always required.
//! - Strictly [`LaunchReadiness::Ready`] - `ReadyWithWarnings` and `Blocked`
//!   are both refused, never silently accepted.
//! - Exactly one requested, already-discovered xemu profile, matched by
//!   profile id - never a silent substitution of a different profile or
//!   executable the fresh discovery happened to also find.
//!
//! # What this module is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never launches Flatpak/Portable/Explicit xemu, RetroArch, PCSX2, or
//!   Dolphin.
//! - It never touches cheats, mods, RomM, DAT, Library View History, ES-DE
//!   writes, or the shared transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/argument list
//!   always comes from
//!   [`crate::launch::xemu_command::build_xemu_command_plan`], rebuilt fresh
//!   from a freshly re-validated identity/content/profile/binding/health
//!   (see [`preflight_xemu_launch`]'s own doc comment for exactly why the
//!   old readiness a caller may have seen earlier is never trusted alone).
//! - It never reads, writes, or copies `xemu.toml` - see
//!   [`crate::launch::xemu_command`]'s own module doc comment; the reused
//!   command plan's `-dvd_path` flag is a one-time runtime override only.
//! - It never adds an automatic timeout, kill, or relaunch - xemu is a
//!   long-running, user-facing process the caller owns; see
//!   [`LaunchedXemuProcess`]'s own doc comment.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::game_identity::inspect_catalogued_game_identity;
use crate::launch::evidence_bridge::canonical_identity_from_game_report;
use crate::launch::input_projection::VerifiedIdentityFact;
use crate::launch::integration::{
    DiscoveredStandaloneProfile, LaunchPlanResults, build_launch_plan_from_results,
};
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef, LaunchTarget,
};
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, ProcessExitReport, WatchedProcess,
};
use crate::launch::readiness::LaunchReadiness;
use crate::launch::xemu_command::{
    XEMU_SUPPORTED_PLATFORM_ID, XemuCommand, build_xemu_command_plan, direct_xbox_disc_extension,
};
use crate::patch_manager::{
    XemuGameRequest, XemuProfileDiscoveryRoots, discover_xemu_profiles, inspect_xemu_game,
    resolve_xemu_native_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native xemu launch is being requested. Never an arbitrary
/// command string - every field here only ever *selects* which
/// already-discovered profile/binding to revalidate and launch; none of it
/// is ever passed to a shell or used to build argv directly (see
/// [`preflight_xemu_launch`]).
///
/// `expected_platform_id`/`expected_game_key` are the caller's already
/// approved [`CanonicalIdentityStatus::Resolved`] fields, re-checked fresh at
/// preflight time. `expected_xbox_title_id` is the verified original-Xbox
/// XBE title ID the caller already approved.
///
/// `expected_executable` is the exact launch binding fact the user was shown
/// at readiness time. A freshly resolved binding whose executable differs is
/// treated as drift and refused rather than silently substituted - see step
/// 7 of [`preflight_xemu_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuLaunchRequest {
    /// The exact, direct content file the user selected - never an outer
    /// archive path and never a mount point.
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub expected_xbox_title_id: String,
    /// Which discovered [`crate::patch_manager::XemuProfile::profile_id`]
    /// the binding must belong to.
    pub profile_id: String,
    pub expected_executable: PathBuf,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XemuLaunchPreflightErrorKind {
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
    /// The requested content path is not a direct `.iso`/`.xiso` disc image
    /// - a loose `.xbe` (xemu cannot boot one directly) is refused here too.
    ContentFormatUnsupported,
    /// Fresh re-inspection produced `Unknown` or `Conflicting` identity -
    /// never resolved to one trustworthy answer.
    IdentityUnresolved,
    /// Fresh identity resolved, but its platform or game key differs from
    /// what the request expected - the content at this path is not the game
    /// the user approved, or it is not `Xbox`.
    IdentityMismatch,
    /// Fresh re-inspection found no verified original-Xbox XBE title ID for
    /// this content at all - this launch slice always requires one.
    XboxTitleIdUnavailable,
    /// Fresh re-inspection found a verified XBE title ID, but it differs
    /// from `request.expected_xbox_title_id`.
    XboxTitleIdMismatch,
    /// No discovered xemu profile matches [`XemuLaunchRequest::profile_id`]
    /// - never substituted with a different profile.
    ProfileNotFound,
    /// [`resolve_xemu_native_launch_binding`] itself refused to produce a
    /// binding for the matched profile.
    BindingUnavailable,
    /// The freshly resolved binding's executable no longer matches what the
    /// request expected - the binding drifted between readiness time and
    /// this click, so it is never silently substituted.
    BindingDrift,
    /// No candidate in the freshly rebuilt plan matches the requested xemu
    /// profile.
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
    /// [`build_xemu_command_plan`] itself reported blockers (including a
    /// missing MCPX/flash BIOS/EEPROM/HDD system file).
    CommandBlocked,
    /// [`build_xemu_command_plan`] reported no blockers but also no command
    /// - should be unreachable given the checks above, but never assumed.
    CommandMissing,
    /// The resolved executable no longer exists.
    ExecutableMissing,
    /// The resolved executable is a symlink or not a regular file.
    ExecutableUnsafe,
    /// The resolved executable is not marked executable.
    ExecutableNotExecutable,
    /// The content file's filesystem identity at the final pre-spawn check
    /// no longer matches what was captured earlier in this same preflight
    /// call - the file was swapped underneath the launch.
    ContentChangedBeforeSpawn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuLaunchPreflightError {
    pub kind: XemuLaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: XemuLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> XemuLaunchPreflightError {
    XemuLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum XemuLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum XemuLaunchExecutionError {
    Preflight(XemuLaunchPreflightError),
    Spawn(XemuLaunchSpawnError),
}

impl From<XemuLaunchPreflightError> for XemuLaunchExecutionError {
    fn from(error: XemuLaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<XemuLaunchSpawnError> for XemuLaunchExecutionError {
    fn from(error: XemuLaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`XemuCommand`] safe to spawn - or refuses with a
/// [`XemuLaunchPreflightError`] naming exactly why.
///
/// # Why nothing from before this call is trusted
///
/// See [`crate::launch::execution::preflight_retroarch_launch`]'s own doc
/// comment for the general rationale; the same reasoning applies here - the
/// content file, the xemu installation, and even the profile's system-file
/// health can all have changed between whenever the user was shown "Ready"
/// and the moment they click Launch.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, be a regular file, not be
///    an outer archive/mount-input path, and have a `.iso`/`.xiso`
///    extension.
/// 3. A [`CapturedFileIdentity`] is captured from the content's current
///    metadata.
/// 4. The content is freshly re-identified via
///    [`inspect_catalogued_game_identity`] (never a caller-supplied old
///    report); the result must resolve to exactly
///    [`XEMU_SUPPORTED_PLATFORM_ID`]/`request.expected_game_key`, and a
///    verified original-Xbox XBE title ID matching
///    `request.expected_xbox_title_id` must be present among the fresh
///    evidence.
/// 5. xemu profiles are freshly rediscovered via [`discover_xemu_profiles`]
///    - never a caller's cached discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_xemu_native_launch_binding`] is called fresh against that
///    profile; its executable must exactly equal
///    `request.expected_executable`.
/// 8. The standalone launch plan is rebuilt via the existing
///    [`build_launch_plan_from_results`] integration entry point (a single
///    [`DiscoveredStandaloneProfile::xemu`] projected from the matched
///    profile). The resulting candidate must be exactly
///    [`LaunchReadiness::Ready`] and still the narrow direct-plain-file
///    shape this phase supports.
/// 9. [`inspect_xemu_game`] is called fresh against the matched profile to
///    obtain current system-file health, and
///    [`build_xemu_command_plan`] is rebuilt from the fresh identity/title
///    id/candidate/binding/health; it must report no blockers and a
///    command.
/// 10. Immediately before returning: the executable is re-checked to still
///     exist, not be a symlink, be a regular file, and be marked
///     executable; the content is re-inspected once more and its
///     [`CapturedFileIdentity`] must still equal the one captured in step 3.
pub fn preflight_xemu_launch(
    request: &XemuLaunchRequest,
    roots: &XemuProfileDiscoveryRoots,
) -> Result<XemuCommand, XemuLaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: fresh identity re-inspection + verified Xbox title ID ---
    let (identity_status, facts, verified_title_id) = fresh_identity_status(content_path, request)?;

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_xemu_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                XemuLaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered xemu profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_xemu_native_launch_binding(profile);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            XemuLaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized executable",
        ));
    }

    // --- 8: rebuild the plan via the existing integration entry point ---
    let content_ref = LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(content_path.clone()),
        requires_mount: false,
        provenance: "xemu launch execution preflight: revalidated direct regular file".to_string(),
    };
    let standalone_profiles = [DiscoveredStandaloneProfile::xemu(profile)];
    let empty_retroarch = RetroArchEnvironmentReport {
        format_version: 1,
        profiles: Vec::new(),
        diagnostics: Vec::new(),
    };
    let plan = build_launch_plan_from_results(&LaunchPlanResults {
        identity: &identity_status,
        verified_identity_facts: &facts,
        content: &content_ref,
        standalone_profiles: &standalone_profiles,
        retroarch: &empty_retroarch,
        remembered: &[],
    });
    let candidate = plan
        .candidates
        .iter()
        .find(|candidate| match &candidate.target {
            LaunchTarget::Standalone {
                adapter_id,
                profile_id,
                ..
            } => *adapter_id == "xemu" && *profile_id == request.profile_id,
            LaunchTarget::RetroArchCore { .. } => false,
        })
        .ok_or_else(|| {
            preflight_error(
                XemuLaunchPreflightErrorKind::RequestedCandidateNotFound,
                "no candidate in the freshly rebuilt plan matches the requested xemu profile",
            )
        })?;

    // --- strict readiness + content-shape gate ---
    if candidate.readiness != LaunchReadiness::Ready {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::CandidateNotReady,
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
            XemuLaunchPreflightErrorKind::CandidateContentUnsupported,
            "candidate content is not a direct, non-mounted plain file",
        ));
    }

    // --- 9: fresh system-file health + rebuild the command plan ---
    let inspection = inspect_xemu_game(
        profile,
        &XemuGameRequest {
            verified_xbox_title_id: Some(verified_title_id.clone()),
            ..Default::default()
        },
    );
    let command_plan = build_xemu_command_plan(
        &identity_status,
        Some(&verified_title_id),
        candidate,
        &binding_result,
        &inspection.health,
    );
    if !command_plan.blockers.is_empty() {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::CommandBlocked,
            format!(
                "command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    let command = command_plan.command.ok_or_else(|| {
        preflight_error(
            XemuLaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content file changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, XemuLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            XemuLaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::ContentNotRegularFile,
            "content path is not a regular file",
        ));
    }
    if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::ContentRequiresMount,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    if !direct_xbox_disc_extension(path) {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::ContentFormatUnsupported,
            "only a direct .iso/.xiso original-Xbox disc image is supported by this native xemu \
             launch slice",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity_status(
    content_path: &Path,
    request: &XemuLaunchRequest,
) -> Result<(CanonicalIdentityStatus, Vec<VerifiedIdentityFact>, String), XemuLaunchPreflightError>
{
    // `inspect_catalogued_game_identity`, not the plain `inspect_game_identity` -
    // this request only ever exists because the game was already
    // catalogued/identified earlier; the platform hint here is fixed to
    // this slice's own supported platform, never derived from this path's
    // name or extension.
    let report = inspect_catalogued_game_identity(content_path, Some(XEMU_SUPPORTED_PLATFORM_ID));
    let (identity_status, facts) = canonical_identity_from_game_report(&report);
    match &identity_status {
        CanonicalIdentityStatus::Resolved(resolved) => {
            if resolved.platform_id != XEMU_SUPPORTED_PLATFORM_ID
                || resolved.platform_id != request.expected_platform_id
                || resolved.game_key != request.expected_game_key
            {
                return Err(preflight_error(
                    XemuLaunchPreflightErrorKind::IdentityMismatch,
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
                XemuLaunchPreflightErrorKind::IdentityUnresolved,
                format!("fresh identity re-inspection produced {identity_status:?}"),
            ));
        }
    }
    let title_id = facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::XboxTitleId(value) => Some(value.clone()),
        _ => None,
    });
    let Some(title_id) = title_id else {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::XboxTitleIdUnavailable,
            "fresh identity re-inspection found no verified original-Xbox XBE title ID for this \
             content",
        ));
    };
    if title_id != request.expected_xbox_title_id {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::XboxTitleIdMismatch,
            format!(
                "resolved Xbox title ID {title_id} does not match expected {}",
                request.expected_xbox_title_id
            ),
        ));
    }
    Ok((identity_status, facts, title_id))
}

fn recheck_executable(path: &Path) -> Result<(), XemuLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            XemuLaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            XemuLaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                XemuLaunchPreflightErrorKind::ExecutableNotExecutable,
                "executable has no execute bit set",
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

/// The exact facts about a launched process a future GUI needs to render
/// state with, captured once at spawn time - never re-derived from the live
/// process afterward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuLaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub platform_id: String,
    pub verified_xbox_title_id: String,
    pub content_path: PathBuf,
}

fn command_facts(command: &XemuCommand) -> XemuLaunchCommandFacts {
    XemuLaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        platform_id: command.selection.platform_id.clone(),
        verified_xbox_title_id: command.selection.verified_xbox_title_id.clone(),
        content_path: command.selection.content_path.clone(),
    }
}

/// What the background watcher thread reports once the process has exited.
pub use crate::launch::process_spawn::ProcessExitReport as XemuLaunchExitReport;

/// A spawned, still-owned xemu process. Never automatically killed, timed
/// out, or relaunched by this module - xemu is a long-running, user-facing
/// program the caller (a future GUI) owns for as long as the user wants it
/// running. [`Self::poll`] is the narrow, non-blocking way to notice it has
/// exited.
pub struct LaunchedXemuProcess {
    pub pid: u32,
    pub command_facts: XemuLaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedXemuProcess {
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
/// must already have passed [`preflight_xemu_launch`]; this function
/// performs no further validation of its own beyond what
/// [`crate::launch::process_spawn::spawn_watched_process`] itself requires
/// to spawn. See that function's own doc comment for the exact lifecycle
/// policy (stdin/stdout/stderr, environment, no timeout/kill).
pub fn spawn_xemu(command: XemuCommand) -> Result<LaunchedXemuProcess, XemuLaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(XemuLaunchSpawnError::Spawn)?;
    Ok(LaunchedXemuProcess {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

// ---------------------------------------------------------------------------
// Convenience: preflight + spawn in one call
// ---------------------------------------------------------------------------

/// Composes [`preflight_xemu_launch`] and [`spawn_xemu`] - the single call a
/// future GUI Launch button would make. Kept as two separate public
/// functions above so preflight-only rejection scenarios can be tested
/// without ever spawning a real process.
pub fn preflight_and_launch_xemu(
    request: &XemuLaunchRequest,
    roots: &XemuProfileDiscoveryRoots,
) -> Result<LaunchedXemuProcess, XemuLaunchExecutionError> {
    let command = preflight_xemu_launch(request, roots)?;
    Ok(spawn_xemu(command)?)
}

#[cfg(test)]
mod tests;
