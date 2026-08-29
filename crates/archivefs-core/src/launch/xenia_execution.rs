//! First supported slice of real native Xenia launch execution: safely
//! revalidating and spawning exactly one native Linux Xenia (Canary)
//! process for one direct, loose, regular Xbox 360 `.xex` file.
//!
//! # Scope (first slice)
//!
//! - `Xbox360` only - the only platform
//!   [`crate::launch::xenia_command::XENIA_SUPPORTED_PLATFORM_ID`] names.
//! - One direct loose regular `.xex` file - no Xbox 360 ISO, no ZIP/archive-
//!   contained game, no GOD container, no STFS package. See
//!   [`crate::launch::xenia_command`]'s own module doc comment for why.
//! - A verified XEX title ID or media ID is always required (either is
//!   sufficient - the same condition [`crate::launch::evidence_bridge`] and
//!   [`crate::launch::xenia_command::build_xenia_command_plan`] already
//!   use).
//! - Strictly [`LaunchReadiness::Ready`] - `ReadyWithWarnings` and
//!   `Blocked` are both refused. Xenia has no firmware/BIOS concept at all
//!   (see [`crate::launch::xenia_command`] - no candidate field is ever
//!   consulted for it), so unlike RPCS3 there is no reason to relax this.
//! - Exactly one requested, already-discovered Xenia profile, matched by
//!   profile id - never a silent substitution of a different profile or
//!   executable the fresh discovery happened to also find.
//!
//! # Native Linux executable only - no Wine/Proton
//!
//! [`crate::patch_manager::resolve_xenia_launch_binding`] was, before this
//! module existed, the one binding resolver in this crate that could never
//! produce a genuinely spawnable native Linux binding at all: it only ever
//! searched for `xenia_canary.exe`, the Windows PE binary, with no native
//! Linux executable name considered. Spawning that file directly with
//! [`std::process::Command::new`] on Linux would either fail outright or
//! silently depend on a system-wide Wine/binfmt_misc association this crate
//! never assumes or configures - exactly the implicit Wine/Proton
//! dependency this launch slice must not have. That resolver has been
//! fixed, as part of this same change, to search only for a native Linux
//! executable (`xenia_canary` or `xenia`, never `.exe`) - see its own
//! updated doc comment in `patch_manager::xenia_local`. A profile that only
//! has the Windows binary remains a real, eligible profile, but has no
//! valid launch binding until a native Linux executable is placed there
//! too; this module still never invents a Wine/Proton fallback.
//!
//! # What this module is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never launches RetroArch, PCSX2, xemu, PPSSPP, RPCS3, or Dolphin.
//! - It never touches cheats, mods, RomM, DAT, Library View History, ES-DE
//!   writes, or the shared transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/argument list
//!   always comes from
//!   [`crate::launch::xenia_command::build_xenia_command_plan`], rebuilt
//!   fresh from a freshly re-validated identity/content/profile/binding.
//! - It never adds an automatic timeout, kill, or relaunch - Xenia is a
//!   long-running, user-facing process the caller owns; see
//!   [`LaunchedXeniaProcess`]'s own doc comment.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::game_identity::inspect_catalogued_game_identity;
use crate::launch::evidence_bridge::canonical_identity_from_game_report;
use crate::launch::integration::{
    DiscoveredStandaloneProfile, LaunchPlanResults, build_launch_plan_from_results,
};
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentRef, LaunchTarget,
};
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, ProcessExitReport, WatchedProcess,
};
use crate::launch::readiness::LaunchReadiness;
use crate::launch::xenia_command::{
    XENIA_SUPPORTED_PLATFORM_ID, XeniaCommand, build_xenia_command_plan, direct_xex_extension,
};
use crate::patch_manager::{
    XeniaProfileDiscoveryRoots, discover_xenia_profiles, resolve_xenia_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native Xenia launch is being requested. Never an
/// arbitrary command string - every field here only ever *selects* which
/// already-discovered profile/binding to revalidate and launch; none of it
/// is ever passed to a shell or used to build argv directly (see
/// [`preflight_xenia_launch`]).
///
/// `expected_platform_id`/`expected_game_key` are the caller's already
/// approved [`CanonicalIdentityStatus::Resolved`] fields, re-checked fresh
/// at preflight time. `expected_xex_title_id`/`expected_xex_media_id` are
/// the verified Xbox 360 XEX identity facts the caller already approved -
/// read directly from
/// [`crate::game_identity::GameIdentityReport::verified_xex_title_id`]/
/// [`verified_xex_media_id`](crate::game_identity::GameIdentityReport::verified_xex_media_id),
/// never through `VerifiedIdentityFact` (no variant names Xbox 360 - see
/// [`crate::launch::input_projection::project_xenia_launch_input`]'s own
/// doc comment). At least one of the two is required, but both are checked
/// for drift independently whenever present.
///
/// `expected_executable` is the exact launch binding fact the user was
/// shown at readiness time. A freshly resolved binding whose executable
/// differs is treated as drift and refused rather than silently
/// substituted - see step 7 of [`preflight_xenia_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XeniaLaunchRequest {
    /// The exact, direct content file the user selected - never an outer
    /// archive path and never a mount point.
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub expected_xex_title_id: Option<String>,
    pub expected_xex_media_id: Option<String>,
    /// Which discovered [`crate::patch_manager::XeniaProfile::profile_id`]
    /// the binding must belong to.
    pub profile_id: String,
    pub expected_executable: PathBuf,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XeniaLaunchPreflightErrorKind {
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
    /// The requested content path is not a direct `.xex` file - Xbox 360
    /// ISO, GOD containers, STFS packages, and ZIP-contained games are all
    /// refused here too.
    ContentFormatUnsupported,
    /// Fresh re-inspection produced `Unknown` or `Conflicting` identity -
    /// never resolved to one trustworthy answer.
    IdentityUnresolved,
    /// Fresh identity resolved, but its platform or game key differs from
    /// what the request expected - the content at this path is not the
    /// game the user approved, or it is not `Xbox360` (this also refuses
    /// original-Xbox content: `Xbox` is a distinct resolved platform id
    /// that can never equal `Xbox360`).
    IdentityMismatch,
    /// Fresh re-inspection found neither a verified XEX title ID nor a
    /// verified XEX media ID for this content at all - this launch slice
    /// always requires at least one.
    XexIdentityUnavailable,
    /// Fresh re-inspection found a verified XEX title ID or media ID, but
    /// it differs from what the request expected.
    XexIdentityMismatch,
    /// No discovered Xenia profile matches
    /// [`XeniaLaunchRequest::profile_id`] - never substituted with a
    /// different profile.
    ProfileNotFound,
    /// [`resolve_xenia_launch_binding`] itself refused to produce a binding
    /// for the matched profile (including a profile that only has the
    /// Windows `xenia_canary.exe` and no native Linux executable).
    BindingUnavailable,
    /// The freshly resolved binding's executable no longer matches what the
    /// request expected - the binding drifted between readiness time and
    /// this click, so it is never silently substituted.
    BindingDrift,
    /// No candidate in the freshly rebuilt plan matches the requested
    /// Xenia profile.
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
    /// [`build_xenia_command_plan`] itself reported blockers.
    CommandBlocked,
    /// [`build_xenia_command_plan`] reported no blockers but also no
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
pub struct XeniaLaunchPreflightError {
    pub kind: XeniaLaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: XeniaLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> XeniaLaunchPreflightError {
    XeniaLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum XeniaLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum XeniaLaunchExecutionError {
    Preflight(XeniaLaunchPreflightError),
    Spawn(XeniaLaunchSpawnError),
}

impl From<XeniaLaunchPreflightError> for XeniaLaunchExecutionError {
    fn from(error: XeniaLaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<XeniaLaunchSpawnError> for XeniaLaunchExecutionError {
    fn from(error: XeniaLaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`XeniaCommand`] safe to spawn - or refuses with a
/// [`XeniaLaunchPreflightError`] naming exactly why.
///
/// # Why nothing from before this call is trusted
///
/// See [`crate::launch::execution::preflight_retroarch_launch`]'s own doc
/// comment for the general rationale; the same reasoning applies here - the
/// content file and the Xenia installation can both have changed between
/// whenever the user was shown "Ready" and the moment they click Launch.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, be a regular file, not be
///    an outer archive/mount-input path, and have a `.xex` extension.
/// 3. A [`CapturedFileIdentity`] is captured from the content's current
///    metadata.
/// 4. The content is freshly re-identified via
///    [`inspect_catalogued_game_identity`] (never a caller-supplied old
///    report) - reusing the existing Xbox 360 identity authority directly,
///    never re-parsing title/module/media facts here. The result must
///    resolve to exactly
///    [`XENIA_SUPPORTED_PLATFORM_ID`]/`request.expected_game_key`, and the
///    freshly re-read
///    [`crate::game_identity::GameIdentityReport::verified_xex_title_id`]/
///    `verified_xex_media_id` must still match whichever of
///    `request.expected_xex_title_id`/`expected_xex_media_id` was
///    originally supplied.
/// 5. Xenia profiles are freshly rediscovered via
///    [`discover_xenia_profiles`] - never a caller's cached discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_xenia_launch_binding`] is called fresh against that
///    profile; its executable must exactly equal
///    `request.expected_executable`.
/// 8. The standalone launch plan is rebuilt via the existing
///    [`build_launch_plan_from_results`] integration entry point (a single
///    [`DiscoveredStandaloneProfile::xenia`] projected from the matched
///    profile and the freshly re-read title/media ID). The resulting
///    candidate must be exactly [`LaunchReadiness::Ready`] and still the
///    narrow direct-plain-file shape this phase supports.
/// 9. [`build_xenia_command_plan`] is rebuilt from the fresh identity/title
///    id/media id/candidate/binding; it must report no blockers and a
///    command.
/// 10. Immediately before returning: the executable is re-checked to still
///     exist, not be a symlink, be a regular file, and be marked
///     executable; the content is re-inspected once more and its
///     [`CapturedFileIdentity`] must still equal the one captured in step
///     3.
pub fn preflight_xenia_launch(
    request: &XeniaLaunchRequest,
    roots: &XeniaProfileDiscoveryRoots,
) -> Result<XeniaCommand, XeniaLaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: fresh identity re-inspection + verified XEX title/media ID ---
    let (identity_status, verified_title_id, verified_media_id) =
        fresh_identity_status(content_path, request)?;

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_xenia_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                XeniaLaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered Xenia profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_xenia_launch_binding(profile);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            XeniaLaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized executable",
        ));
    }

    // --- 8: rebuild the plan via the existing integration entry point ---
    let content_ref = LaunchContentRef {
        kind: Some(crate::launch::planning::LaunchContentKind::Executable),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(content_path.clone()),
        requires_mount: false,
        provenance: "xenia launch execution preflight: revalidated direct regular file".to_string(),
    };
    let standalone_profiles = [DiscoveredStandaloneProfile::xenia(
        profile,
        verified_title_id.as_deref(),
        verified_media_id.as_deref(),
    )];
    let empty_retroarch = RetroArchEnvironmentReport {
        format_version: 1,
        profiles: Vec::new(),
        diagnostics: Vec::new(),
    };
    let plan = build_launch_plan_from_results(&LaunchPlanResults {
        identity: &identity_status,
        verified_identity_facts: &[],
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
            } => *adapter_id == "xenia" && *profile_id == request.profile_id,
            LaunchTarget::RetroArchCore { .. } => false,
        })
        .ok_or_else(|| {
            preflight_error(
                XeniaLaunchPreflightErrorKind::RequestedCandidateNotFound,
                "no candidate in the freshly rebuilt plan matches the requested Xenia profile",
            )
        })?;

    // --- strict readiness + content-shape gate ---
    if candidate.readiness != LaunchReadiness::Ready {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::CandidateNotReady,
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
            XeniaLaunchPreflightErrorKind::CandidateContentUnsupported,
            "candidate content is not a direct, non-mounted plain file",
        ));
    }

    // --- 9: rebuild the command plan ---
    let command_plan = build_xenia_command_plan(
        &identity_status,
        verified_title_id.as_deref(),
        verified_media_id.as_deref(),
        candidate,
        &binding_result,
    );
    if !command_plan.blockers.is_empty() {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::CommandBlocked,
            format!(
                "command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    let command = command_plan.command.ok_or_else(|| {
        preflight_error(
            XeniaLaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content file changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, XeniaLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            XeniaLaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::ContentNotRegularFile,
            "content path is not a regular file",
        ));
    }
    if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::ContentRequiresMount,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    if !direct_xex_extension(path) {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::ContentFormatUnsupported,
            "only a direct .xex file is supported by this native Xenia launch slice - Xbox 360 \
             ISO, GOD containers, STFS packages, and ZIP-contained games are all refused",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity_status(
    content_path: &Path,
    request: &XeniaLaunchRequest,
) -> Result<(CanonicalIdentityStatus, Option<String>, Option<String>), XeniaLaunchPreflightError> {
    // `inspect_catalogued_game_identity`, not the plain `inspect_game_identity` -
    // this request only ever exists because the game was already
    // catalogued/identified earlier; the platform hint here is fixed to
    // this slice's own supported platform, never derived from this path's
    // name or extension.
    let report = inspect_catalogued_game_identity(content_path, Some(XENIA_SUPPORTED_PLATFORM_ID));
    let (identity_status, _facts) = canonical_identity_from_game_report(&report);
    match &identity_status {
        CanonicalIdentityStatus::Resolved(resolved) => {
            if resolved.platform_id != XENIA_SUPPORTED_PLATFORM_ID
                || resolved.platform_id != request.expected_platform_id
                || resolved.game_key != request.expected_game_key
            {
                return Err(preflight_error(
                    XeniaLaunchPreflightErrorKind::IdentityMismatch,
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
                XeniaLaunchPreflightErrorKind::IdentityUnresolved,
                format!("fresh identity re-inspection produced {identity_status:?}"),
            ));
        }
    }
    // No `VerifiedIdentityFact` variant names Xbox 360 (see this crate's
    // own `evidence_bridge`/`input_projection` doc comments), so the
    // verified XEX title/media ID is read directly off the fresh report -
    // never re-parsed here, and never derived from `_facts` above.
    let title_id = report.verified_xex_title_id().map(str::to_string);
    let media_id = report.verified_xex_media_id().map(str::to_string);
    if title_id.is_none() && media_id.is_none() {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::XexIdentityUnavailable,
            "fresh identity re-inspection found no verified Xbox 360 XEX title ID or media ID \
             for this content",
        ));
    }
    if title_id != request.expected_xex_title_id || media_id != request.expected_xex_media_id {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::XexIdentityMismatch,
            format!(
                "resolved XEX title/media ID {title_id:?}/{media_id:?} does not match expected \
                 {:?}/{:?}",
                request.expected_xex_title_id, request.expected_xex_media_id
            ),
        ));
    }
    Ok((identity_status, title_id, media_id))
}

fn recheck_executable(path: &Path) -> Result<(), XeniaLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            XeniaLaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            XeniaLaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                XeniaLaunchPreflightErrorKind::ExecutableNotExecutable,
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
pub struct XeniaLaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub platform_id: String,
    pub verified_xex_title_id: Option<String>,
    pub verified_xex_media_id: Option<String>,
    pub content_path: PathBuf,
}

fn command_facts(command: &XeniaCommand) -> XeniaLaunchCommandFacts {
    XeniaLaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        platform_id: command.selection.platform_id.clone(),
        verified_xex_title_id: command.selection.verified_xex_title_id.clone(),
        verified_xex_media_id: command.selection.verified_xex_media_id.clone(),
        content_path: command.selection.content_path.clone(),
    }
}

/// What the background watcher thread reports once the process has exited.
pub use crate::launch::process_spawn::ProcessExitReport as XeniaLaunchExitReport;

/// A spawned, still-owned Xenia process. Never automatically killed, timed
/// out, or relaunched by this module - Xenia is a long-running, user-facing
/// program the caller (a future GUI) owns for as long as the user wants it
/// running. [`Self::poll`] is the narrow, non-blocking way to notice it has
/// exited.
pub struct LaunchedXeniaProcess {
    pub pid: u32,
    pub command_facts: XeniaLaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedXeniaProcess {
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
/// must already have passed [`preflight_xenia_launch`]; this function
/// performs no further validation of its own beyond what
/// [`crate::launch::process_spawn::spawn_watched_process`] itself requires
/// to spawn. See that function's own doc comment for the exact lifecycle
/// policy (stdin/stdout/stderr, environment, no timeout/kill).
pub fn spawn_xenia(command: XeniaCommand) -> Result<LaunchedXeniaProcess, XeniaLaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(XeniaLaunchSpawnError::Spawn)?;
    Ok(LaunchedXeniaProcess {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

// ---------------------------------------------------------------------------
// Convenience: preflight + spawn in one call
// ---------------------------------------------------------------------------

/// Composes [`preflight_xenia_launch`] and [`spawn_xenia`] - the single
/// call a future GUI Launch button would make. Kept as two separate public
/// functions above so preflight-only rejection scenarios can be tested
/// without ever spawning a real process.
pub fn preflight_and_launch_xenia(
    request: &XeniaLaunchRequest,
    roots: &XeniaProfileDiscoveryRoots,
) -> Result<LaunchedXeniaProcess, XeniaLaunchExecutionError> {
    let command = preflight_xenia_launch(request, roots)?;
    Ok(spawn_xenia(command)?)
}

#[cfg(test)]
mod tests;
