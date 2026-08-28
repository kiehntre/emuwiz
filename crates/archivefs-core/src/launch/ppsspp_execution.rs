//! First supported slice of real native PPSSPP launch execution: safely
//! revalidating and spawning exactly one native PPSSPP process for one
//! direct, loose, regular PSP disc image.
//!
//! # Scope (first slice)
//!
//! - Native PPSSPP profiles only - any profile
//!   [`crate::patch_manager::resolve_ppsspp_native_launch_binding`] itself
//!   refuses is never attempted.
//! - `PSP` only - the only platform
//!   [`crate::launch::ppsspp_command::PPSSPP_SUPPORTED_PLATFORM_ID`] names.
//! - One direct loose regular `.iso` disc image - no CSO, CHD, PBP, ZIP,
//!   or mounted content. See [`crate::launch::ppsspp_command`]'s own module
//!   doc comment for why those remain unsupported.
//! - A verified PSP disc ID is always required.
//! - Strictly [`LaunchReadiness::Ready`] - `ReadyWithWarnings` and `Blocked`
//!   are both refused, never silently accepted.
//! - Exactly one requested, already-discovered PPSSPP profile, matched by
//!   profile id - never a silent substitution of a different profile or
//!   executable the fresh discovery happened to also find.
//!
//! # What this module is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never launches Flatpak/Portable/Explicit PPSSPP, RetroArch, PCSX2,
//!   xemu, or Dolphin.
//! - It never touches cheats, mods, RomM, DAT, Library View History, ES-DE
//!   writes, or the shared transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/argument list
//!   always comes from
//!   [`crate::launch::ppsspp_command::build_ppsspp_command_plan`], rebuilt
//!   fresh from a freshly re-validated identity/content/profile/binding
//!   (see [`preflight_ppsspp_launch`]'s own doc comment for exactly why the
//!   old readiness a caller may have seen earlier is never trusted alone).
//! - It never adds Wine/Proton - native Linux PPSSPP only.
//! - It never adds an automatic timeout, kill, or relaunch - PPSSPP is a
//!   long-running, user-facing process the caller owns; see
//!   [`LaunchedPpssppProcess`]'s own doc comment.

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
use crate::launch::ppsspp_command::{
    PPSSPP_SUPPORTED_PLATFORM_ID, PpssppCommand, build_ppsspp_command_plan, direct_psp_extension,
};
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, ProcessExitReport, WatchedProcess,
};
use crate::launch::readiness::LaunchReadiness;
use crate::patch_manager::{
    PpssppProfileDiscoveryRoots, discover_ppsspp_profiles, resolve_ppsspp_native_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native PPSSPP launch is being requested. Never an
/// arbitrary command string - every field here only ever *selects* which
/// already-discovered profile/binding to revalidate and launch; none of it
/// is ever passed to a shell or used to build argv directly (see
/// [`preflight_ppsspp_launch`]).
///
/// `expected_platform_id`/`expected_game_key` are the caller's already
/// approved [`CanonicalIdentityStatus::Resolved`] fields, re-checked fresh at
/// preflight time. `expected_psp_disc_id` is the verified PSP disc ID the
/// caller already approved.
///
/// `expected_executable` is the exact launch binding fact the user was shown
/// at readiness time. A freshly resolved binding whose executable differs is
/// treated as drift and refused rather than silently substituted - see step
/// 7 of [`preflight_ppsspp_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppLaunchRequest {
    /// The exact, direct content file the user selected - never an outer
    /// archive path and never a mount point.
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub expected_psp_disc_id: String,
    /// Which discovered [`crate::patch_manager::PpssppProfile::profile_id`]
    /// the binding must belong to.
    pub profile_id: String,
    pub expected_executable: PathBuf,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PpssppLaunchPreflightErrorKind {
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
    /// The requested content path is not a direct `.iso` file - CSO, CHD,
    /// PBP, and ZIP-contained PSP games are all refused here too.
    ContentFormatUnsupported,
    /// Fresh re-inspection produced `Unknown` or `Conflicting` identity -
    /// never resolved to one trustworthy answer.
    IdentityUnresolved,
    /// Fresh identity resolved, but its platform or game key differs from
    /// what the request expected - the content at this path is not the game
    /// the user approved, or it is not `PSP`.
    IdentityMismatch,
    /// Fresh re-inspection found no verified PSP disc ID for this content
    /// at all - this launch slice always requires one.
    PspDiscIdUnavailable,
    /// Fresh re-inspection found a verified PSP disc ID, but it differs
    /// from `request.expected_psp_disc_id`.
    PspDiscIdMismatch,
    /// No discovered PPSSPP profile matches
    /// [`PpssppLaunchRequest::profile_id`] - never substituted with a
    /// different profile.
    ProfileNotFound,
    /// [`resolve_ppsspp_native_launch_binding`] itself refused to produce a
    /// binding for the matched profile.
    BindingUnavailable,
    /// The freshly resolved binding's executable no longer matches what the
    /// request expected - the binding drifted between readiness time and
    /// this click, so it is never silently substituted.
    BindingDrift,
    /// No candidate in the freshly rebuilt plan matches the requested
    /// PPSSPP profile.
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
    /// [`build_ppsspp_command_plan`] itself reported blockers.
    CommandBlocked,
    /// [`build_ppsspp_command_plan`] reported no blockers but also no
    /// command - should be unreachable given the checks above, but never
    /// assumed.
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
pub struct PpssppLaunchPreflightError {
    pub kind: PpssppLaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: PpssppLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> PpssppLaunchPreflightError {
    PpssppLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum PpssppLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum PpssppLaunchExecutionError {
    Preflight(PpssppLaunchPreflightError),
    Spawn(PpssppLaunchSpawnError),
}

impl From<PpssppLaunchPreflightError> for PpssppLaunchExecutionError {
    fn from(error: PpssppLaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<PpssppLaunchSpawnError> for PpssppLaunchExecutionError {
    fn from(error: PpssppLaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`PpssppCommand`] safe to spawn - or refuses with a
/// [`PpssppLaunchPreflightError`] naming exactly why.
///
/// # Why nothing from before this call is trusted
///
/// See [`crate::launch::execution::preflight_retroarch_launch`]'s own doc
/// comment for the general rationale; the same reasoning applies here - the
/// content file, the PPSSPP installation, and even the profile's data
/// directory can all have changed between whenever the user was shown
/// "Ready" and the moment they click Launch.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, be a regular file, not be
///    an outer archive/mount-input path, and have a `.iso` extension.
/// 3. A [`CapturedFileIdentity`] is captured from the content's current
///    metadata.
/// 4. The content is freshly re-identified via
///    [`inspect_catalogued_game_identity`] (never a caller-supplied old
///    report); the result must resolve to exactly
///    [`PPSSPP_SUPPORTED_PLATFORM_ID`]/`request.expected_game_key`, and a
///    verified PSP disc ID matching `request.expected_psp_disc_id` must be
///    present among the fresh evidence.
/// 5. PPSSPP profiles are freshly rediscovered via
///    [`discover_ppsspp_profiles`] - never a caller's cached discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_ppsspp_native_launch_binding`] is called fresh against that
///    profile; its executable must exactly equal
///    `request.expected_executable`.
/// 8. The standalone launch plan is rebuilt via the existing
///    [`build_launch_plan_from_results`] integration entry point (a single
///    [`DiscoveredStandaloneProfile::ppsspp`] projected from the matched
///    profile - PPSSPP needs no BIOS, so
///    [`crate::launch::readiness::ppsspp_firmware_readiness`] is reused
///    unchanged). The resulting candidate must be exactly
///    [`LaunchReadiness::Ready`] and still the narrow direct-plain-file
///    shape this phase supports.
/// 9. [`build_ppsspp_command_plan`] is rebuilt from the fresh identity/disc
///    id/candidate/binding; it must report no blockers and a command.
/// 10. Immediately before returning: the executable is re-checked to still
///     exist, not be a symlink, be a regular file, and be marked
///     executable; the content is re-inspected once more and its
///     [`CapturedFileIdentity`] must still equal the one captured in step 3.
pub fn preflight_ppsspp_launch(
    request: &PpssppLaunchRequest,
    roots: &PpssppProfileDiscoveryRoots,
) -> Result<PpssppCommand, PpssppLaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: fresh identity re-inspection + verified PSP disc ID ---
    let (identity_status, facts, verified_disc_id) = fresh_identity_status(content_path, request)?;

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_ppsspp_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                PpssppLaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered PPSSPP profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_ppsspp_native_launch_binding(profile);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            PpssppLaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized executable",
        ));
    }

    // --- 8: rebuild the plan via the existing integration entry point ---
    let content_ref = LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(content_path.clone()),
        requires_mount: false,
        provenance: "ppsspp launch execution preflight: revalidated direct regular file"
            .to_string(),
    };
    let standalone_profiles = [DiscoveredStandaloneProfile::ppsspp(profile)];
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
            } => *adapter_id == "ppsspp" && *profile_id == request.profile_id,
            LaunchTarget::RetroArchCore { .. } => false,
        })
        .ok_or_else(|| {
            preflight_error(
                PpssppLaunchPreflightErrorKind::RequestedCandidateNotFound,
                "no candidate in the freshly rebuilt plan matches the requested PPSSPP profile",
            )
        })?;

    // --- strict readiness + content-shape gate ---
    if candidate.readiness != LaunchReadiness::Ready {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::CandidateNotReady,
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
            PpssppLaunchPreflightErrorKind::CandidateContentUnsupported,
            "candidate content is not a direct, non-mounted plain file",
        ));
    }

    // --- 9: rebuild the command plan ---
    let command_plan = build_ppsspp_command_plan(
        &identity_status,
        Some(&verified_disc_id),
        candidate,
        &binding_result,
    );
    if !command_plan.blockers.is_empty() {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::CommandBlocked,
            format!(
                "command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    let command = command_plan.command.ok_or_else(|| {
        preflight_error(
            PpssppLaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content file changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, PpssppLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            PpssppLaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::ContentNotRegularFile,
            "content path is not a regular file",
        ));
    }
    if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::ContentRequiresMount,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    if !direct_psp_extension(path) {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::ContentFormatUnsupported,
            "only a direct .iso file is supported by this native PPSSPP launch slice - CSO, \
             CHD, PBP, and ZIP-contained PSP games are all refused",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity_status(
    content_path: &Path,
    request: &PpssppLaunchRequest,
) -> Result<(CanonicalIdentityStatus, Vec<VerifiedIdentityFact>, String), PpssppLaunchPreflightError>
{
    // `inspect_catalogued_game_identity`, not the plain `inspect_game_identity` -
    // this request only ever exists because the game was already
    // catalogued/identified earlier; the platform hint here is fixed to
    // this slice's own supported platform, never derived from this path's
    // name or extension.
    let report = inspect_catalogued_game_identity(content_path, Some(PPSSPP_SUPPORTED_PLATFORM_ID));
    let (identity_status, facts) = canonical_identity_from_game_report(&report);
    match &identity_status {
        CanonicalIdentityStatus::Resolved(resolved) => {
            if resolved.platform_id != PPSSPP_SUPPORTED_PLATFORM_ID
                || resolved.platform_id != request.expected_platform_id
                || resolved.game_key != request.expected_game_key
            {
                return Err(preflight_error(
                    PpssppLaunchPreflightErrorKind::IdentityMismatch,
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
                PpssppLaunchPreflightErrorKind::IdentityUnresolved,
                format!("fresh identity re-inspection produced {identity_status:?}"),
            ));
        }
    }
    let disc_id = facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::PspDiscId(value) => Some(value.clone()),
        _ => None,
    });
    let Some(disc_id) = disc_id else {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::PspDiscIdUnavailable,
            "fresh identity re-inspection found no verified PSP disc ID for this content",
        ));
    };
    if disc_id != request.expected_psp_disc_id {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::PspDiscIdMismatch,
            format!(
                "resolved PSP disc ID {disc_id} does not match expected {}",
                request.expected_psp_disc_id
            ),
        ));
    }
    Ok((identity_status, facts, disc_id))
}

fn recheck_executable(path: &Path) -> Result<(), PpssppLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            PpssppLaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            PpssppLaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                PpssppLaunchPreflightErrorKind::ExecutableNotExecutable,
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
pub struct PpssppLaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub platform_id: String,
    pub verified_psp_disc_id: String,
    pub content_path: PathBuf,
}

fn command_facts(command: &PpssppCommand) -> PpssppLaunchCommandFacts {
    PpssppLaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        platform_id: command.selection.platform_id.clone(),
        verified_psp_disc_id: command.selection.verified_psp_disc_id.clone(),
        content_path: command.selection.content_path.clone(),
    }
}

/// What the background watcher thread reports once the process has exited.
pub use crate::launch::process_spawn::ProcessExitReport as PpssppLaunchExitReport;

/// A spawned, still-owned PPSSPP process. Never automatically killed, timed
/// out, or relaunched by this module - PPSSPP is a long-running, user-facing
/// program the caller (a future GUI) owns for as long as the user wants it
/// running. [`Self::poll`] is the narrow, non-blocking way to notice it has
/// exited.
pub struct LaunchedPpssppProcess {
    pub pid: u32,
    pub command_facts: PpssppLaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedPpssppProcess {
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
/// must already have passed [`preflight_ppsspp_launch`]; this function
/// performs no further validation of its own beyond what
/// [`crate::launch::process_spawn::spawn_watched_process`] itself requires
/// to spawn. See that function's own doc comment for the exact lifecycle
/// policy (stdin/stdout/stderr, environment, no timeout/kill).
pub fn spawn_ppsspp(
    command: PpssppCommand,
) -> Result<LaunchedPpssppProcess, PpssppLaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(PpssppLaunchSpawnError::Spawn)?;
    Ok(LaunchedPpssppProcess {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

// ---------------------------------------------------------------------------
// Convenience: preflight + spawn in one call
// ---------------------------------------------------------------------------

/// Composes [`preflight_ppsspp_launch`] and [`spawn_ppsspp`] - the single
/// call a future GUI Launch button would make. Kept as two separate public
/// functions above so preflight-only rejection scenarios can be tested
/// without ever spawning a real process.
pub fn preflight_and_launch_ppsspp(
    request: &PpssppLaunchRequest,
    roots: &PpssppProfileDiscoveryRoots,
) -> Result<LaunchedPpssppProcess, PpssppLaunchExecutionError> {
    let command = preflight_ppsspp_launch(request, roots)?;
    Ok(spawn_ppsspp(command)?)
}

#[cfg(test)]
mod tests;
