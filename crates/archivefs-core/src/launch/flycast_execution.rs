//! First supported slice of real native Flycast launch execution: safely
//! revalidating and spawning exactly one native Flycast process for one
//! direct, loose, regular Dreamcast content file.
//!
//! # Scope (first slice)
//!
//! - Native Flycast profiles only, matched by exact profile id - never a
//!   silent substitution of a different profile or executable the fresh
//!   discovery happened to also find. Unlike
//!   [`crate::launch::duckstation_execution`], this slice does not refuse
//!   `FlatpakUser`/`Portable`/`Explicit` installation types outright: no
//!   equivalent upstream research exists proving those installation types
//!   are unsafe for Flycast the way DuckStation's Flatpak/Portable/Explicit
//!   refusals are proven - see
//!   [`crate::patch_manager::resolve_flycast_native_launch_binding`]'s own
//!   module doc comment for exactly what claim this binding does and does
//!   not make.
//! - `Dreamcast` only - the only platform
//!   [`crate::launch::flycast_command::FLYCAST_SUPPORTED_PLATFORM_ID`] names
//!   in this phase.
//! - One direct loose regular `.iso`, `.cue`, or `.chd` file - no archive
//!   members, no mounted content, no GDI/CDI, no multi-track GD-ROM. See
//!   `flycast_command`'s own module doc comment for exactly why these three
//!   formats (and only these three) are accepted: they are exactly what
//!   [`crate::game_identity`]'s Dreamcast IP.BIN identity check already
//!   verifies authoritatively.
//! - A verified Dreamcast product code is always required.
//! - Strictly [`LaunchReadiness::Ready`] - `ReadyWithWarnings` and
//!   `Blocked` are both refused, never silently accepted. Because
//!   [`crate::patch_manager::FlycastSystemFileState`] has no `Verified`
//!   variant at all (see `flycast_firmware_readiness`'s own doc comment),
//!   this is reachable only when Flycast's own BIOS inspection reports
//!   `Unknown` (config not read/BIOS presence not determined) - the same
//!   situation PCSX2's own strict-`Ready` gate is already in today, since
//!   its `Verified` variant is likewise not yet produced by any real
//!   verifier (see `pcsx2_firmware_readiness`'s doc comment). This is an
//!   existing accepted pattern in this codebase, not a new gap introduced
//!   here.
//!
//! # What this module is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never touches cheats, mods, RomM, DAT, Library View History, ES-DE
//!   writes, or the shared transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/argument list
//!   always comes from
//!   [`crate::launch::flycast_command::build_flycast_command_plan`],
//!   rebuilt fresh from a freshly re-validated identity/content/profile/
//!   binding (see [`preflight_flycast_launch`]'s own doc comment for
//!   exactly why the old readiness a caller may have seen earlier is never
//!   trusted alone).
//! - It never adds an automatic timeout, kill, or relaunch - Flycast is a
//!   long-running, user-facing process the caller owns; see
//!   [`LaunchedFlycastProcess`]'s own doc comment.
//! - It never fabricates firmware evidence: Flycast has no hash-verified
//!   BIOS state today, so unlike DuckStation's execution slice this module
//!   takes no `firmware_evidence` parameter at all.
//! - It never implements GDI/CDI parsing or multi-track GD-ROM support.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::game_identity::inspect_catalogued_game_identity;
use crate::launch::evidence_bridge::canonical_identity_from_game_report;
use crate::launch::flycast_command::{
    FLYCAST_SUPPORTED_PLATFORM_ID, FlycastCommand, build_flycast_command_plan,
    direct_dreamcast_extension,
};
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
use crate::patch_manager::{
    FlycastGameRequest, FlycastProfileDiscoveryRoots, discover_flycast_profiles,
    inspect_flycast_game, resolve_flycast_native_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native Flycast launch is being requested. Never an
/// arbitrary command string - every field here only ever *selects* which
/// already-discovered profile/binding to revalidate and launch; none of it
/// is ever passed to a shell or used to build argv directly (see
/// [`preflight_flycast_launch`]).
///
/// `expected_platform_id`/`expected_game_key` are the caller's already
/// approved [`CanonicalIdentityStatus::Resolved`] fields, re-checked fresh
/// at preflight time. `expected_dreamcast_product_code` is the verified
/// Dreamcast product code the caller already approved - re-checked fresh at
/// preflight time.
///
/// `expected_executable` is the exact launch binding fact the user was shown
/// at readiness time. A freshly resolved binding whose executable differs
/// is treated as drift and refused rather than silently substituted - see
/// step 7 of [`preflight_flycast_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastLaunchRequest {
    /// The exact, direct content file the user selected - never an outer
    /// archive path and never a mount point.
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub expected_dreamcast_product_code: String,
    /// Which discovered [`crate::patch_manager::FlycastProfile::profile_id`]
    /// the binding must belong to.
    pub profile_id: String,
    pub expected_executable: PathBuf,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlycastLaunchPreflightErrorKind {
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
    /// The requested content path is not a direct `.iso`/`.cue`/`.chd`
    /// file - includes GDI/CDI and any other non-supported extension.
    ContentFormatUnsupported,
    /// Fresh re-inspection produced `Unknown` or `Conflicting` identity -
    /// never resolved to one trustworthy answer.
    IdentityUnresolved,
    /// Fresh identity resolved, but its platform or game key differs from
    /// what the request expected - the content at this path is not the
    /// game the user approved, or it is not `Dreamcast`.
    IdentityMismatch,
    /// Fresh re-inspection found no verified Dreamcast product code for
    /// this content.
    DreamcastProductCodeUnavailable,
    /// Fresh re-inspection found a verified Dreamcast product code, but it
    /// differs from `request.expected_dreamcast_product_code`.
    DreamcastProductCodeMismatch,
    /// Fresh Flycast profile discovery itself failed.
    DiscoveryFailed,
    /// No discovered Flycast profile matches
    /// [`FlycastLaunchRequest::profile_id`] - never substituted with a
    /// different profile.
    ProfileNotFound,
    /// [`resolve_flycast_native_launch_binding`] itself refused to produce
    /// a binding for the matched profile.
    BindingUnavailable,
    /// The freshly resolved binding's executable no longer matches what the
    /// request expected - the binding drifted between readiness time and
    /// this click, so it is never silently substituted.
    BindingDrift,
    /// No candidate in the freshly rebuilt plan matches the requested
    /// Flycast profile.
    RequestedCandidateNotFound,
    /// The matched candidate's own readiness is not exactly
    /// [`LaunchReadiness::Ready`] - covers both `ReadyWithWarnings` and
    /// `Blocked`. See this module's own doc comment for why Flycast's own
    /// never-`Verified` BIOS state means this is reachable only via the
    /// `Unknown` BIOS-state path.
    CandidateNotReady,
    /// The matched candidate's content is not the narrow, direct,
    /// non-mounted plain-file shape this phase supports, even though its
    /// readiness reported `Ready` - defense in depth against a future
    /// planner change silently widening what counts as "Ready".
    CandidateContentUnsupported,
    /// [`build_flycast_command_plan`] itself reported blockers.
    CommandBlocked,
    /// [`build_flycast_command_plan`] reported no blockers but also no
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
pub struct FlycastLaunchPreflightError {
    pub kind: FlycastLaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: FlycastLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> FlycastLaunchPreflightError {
    FlycastLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum FlycastLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum FlycastLaunchExecutionError {
    Preflight(FlycastLaunchPreflightError),
    Spawn(FlycastLaunchSpawnError),
}

impl From<FlycastLaunchPreflightError> for FlycastLaunchExecutionError {
    fn from(error: FlycastLaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<FlycastLaunchSpawnError> for FlycastLaunchExecutionError {
    fn from(error: FlycastLaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`FlycastCommand`] safe to spawn - or refuses with a
/// [`FlycastLaunchPreflightError`] naming exactly why.
///
/// # Why nothing from before this call is trusted
///
/// See [`crate::launch::execution::preflight_retroarch_launch`]'s own doc
/// comment for the general rationale; the same reasoning applies here - the
/// content file, the Flycast installation, and even the profile's
/// executable can all have changed between whenever the user was shown
/// "Ready" and the moment they click Launch.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, be a regular file, not be
///    an outer archive/mount-input path, and have a `.iso`, `.cue`, or
///    `.chd` extension.
/// 3. A [`CapturedFileIdentity`] is captured from the content's current
///    metadata.
/// 4. The content is freshly re-identified via
///    [`inspect_catalogued_game_identity`] (never a caller-supplied old
///    report); the result must resolve to exactly
///    [`FLYCAST_SUPPORTED_PLATFORM_ID`]/`request.expected_game_key`, and a
///    verified Dreamcast product code matching
///    `request.expected_dreamcast_product_code` must be present among the
///    fresh evidence.
/// 5. Flycast profiles are freshly rediscovered via
///    [`discover_flycast_profiles`] - never a caller's cached discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_flycast_native_launch_binding`] is called fresh against
///    that profile; its executable must exactly equal
///    `request.expected_executable` - otherwise this is `BindingDrift`.
/// 8. The standalone launch plan is rebuilt via the existing
///    [`build_launch_plan_from_results`] integration entry point (a single
///    [`DiscoveredStandaloneProfile::flycast`] projected from the matched
///    profile and a fresh [`inspect_flycast_game`] call). The resulting
///    candidate must be exactly [`LaunchReadiness::Ready`] and still the
///    narrow direct-plain-file shape this phase supports.
/// 9. [`build_flycast_command_plan`] is rebuilt from the fresh
///    identity/product-code/candidate/binding; it must report no blockers
///    and a command.
/// 10. Immediately before returning: the executable is re-checked to still
///     exist, not be a symlink, be a regular file, and be marked
///     executable; the content is re-inspected once more and its
///     [`CapturedFileIdentity`] must still equal the one captured in step
///     3.
pub fn preflight_flycast_launch(
    request: &FlycastLaunchRequest,
    roots: &FlycastProfileDiscoveryRoots,
) -> Result<FlycastCommand, FlycastLaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: fresh identity re-inspection + verified Dreamcast product code ---
    let (identity_status, facts, verified_product_code) =
        fresh_identity_status(content_path, request)?;

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_flycast_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                FlycastLaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered Flycast profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_flycast_native_launch_binding(profile);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            FlycastLaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized \
             executable",
        ));
    }

    // --- 8: rebuild the plan via the existing integration entry point ---
    let content_ref = LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(content_path.clone()),
        requires_mount: false,
        provenance: "flycast launch execution preflight: revalidated direct regular file"
            .to_string(),
    };
    let inspection = inspect_flycast_game(
        profile,
        &FlycastGameRequest {
            verified_dreamcast_product_code: Some(verified_product_code.clone()),
            ..Default::default()
        },
    );
    let standalone_profiles = [DiscoveredStandaloneProfile::flycast(profile, &inspection)];
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
            } => *adapter_id == "flycast" && *profile_id == request.profile_id,
            LaunchTarget::RetroArchCore { .. } => false,
        })
        .ok_or_else(|| {
            preflight_error(
                FlycastLaunchPreflightErrorKind::RequestedCandidateNotFound,
                "no candidate in the freshly rebuilt plan matches the requested Flycast profile",
            )
        })?;

    // --- strict readiness + content-shape gate ---
    if candidate.readiness != LaunchReadiness::Ready {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::CandidateNotReady,
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
            FlycastLaunchPreflightErrorKind::CandidateContentUnsupported,
            "candidate content is not a direct, non-mounted plain file",
        ));
    }

    // --- 9: rebuild the command plan ---
    let command_plan = build_flycast_command_plan(
        &identity_status,
        Some(&verified_product_code),
        candidate,
        &binding_result,
    );
    if !command_plan.blockers.is_empty() {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::CommandBlocked,
            format!(
                "command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    let command = command_plan.command.ok_or_else(|| {
        preflight_error(
            FlycastLaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content file changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, FlycastLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            FlycastLaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::ContentNotRegularFile,
            "content path is not a regular file",
        ));
    }
    if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::ContentRequiresMount,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    if !direct_dreamcast_extension(path) {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::ContentFormatUnsupported,
            "only a direct .iso, .cue, or .chd file is supported by this native Flycast launch \
             slice",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity_status(
    content_path: &Path,
    request: &FlycastLaunchRequest,
) -> Result<(CanonicalIdentityStatus, Vec<VerifiedIdentityFact>, String), FlycastLaunchPreflightError>
{
    // `inspect_catalogued_game_identity`, not the plain `inspect_game_identity` -
    // this request only ever exists because the game was already
    // catalogued/identified earlier; the platform hint here is fixed to
    // this slice's own supported platform, never derived from this path's
    // name or extension.
    let report =
        inspect_catalogued_game_identity(content_path, Some(FLYCAST_SUPPORTED_PLATFORM_ID));
    let (identity_status, facts) = canonical_identity_from_game_report(&report);
    match &identity_status {
        CanonicalIdentityStatus::Resolved(resolved) => {
            if resolved.platform_id != FLYCAST_SUPPORTED_PLATFORM_ID
                || resolved.platform_id != request.expected_platform_id
                || resolved.game_key != request.expected_game_key
            {
                return Err(preflight_error(
                    FlycastLaunchPreflightErrorKind::IdentityMismatch,
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
                FlycastLaunchPreflightErrorKind::IdentityUnresolved,
                format!("fresh identity re-inspection produced {identity_status:?}"),
            ));
        }
    }
    let product_code = facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::DreamcastProductCode(value) => Some(value.clone()),
        _ => None,
    });
    let Some(product_code) = product_code else {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::DreamcastProductCodeUnavailable,
            "fresh identity re-inspection found no verified Dreamcast product code for this \
             content",
        ));
    };
    if product_code != request.expected_dreamcast_product_code {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::DreamcastProductCodeMismatch,
            format!(
                "resolved Dreamcast product code {product_code} does not match expected {}",
                request.expected_dreamcast_product_code
            ),
        ));
    }
    Ok((identity_status, facts, product_code))
}

fn recheck_executable(path: &Path) -> Result<(), FlycastLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            FlycastLaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            FlycastLaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                FlycastLaunchPreflightErrorKind::ExecutableNotExecutable,
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
pub struct FlycastLaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub platform_id: String,
    pub verified_dreamcast_product_code: String,
    pub content_path: PathBuf,
}

fn command_facts(command: &FlycastCommand) -> FlycastLaunchCommandFacts {
    FlycastLaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        platform_id: command.selection.platform_id.clone(),
        verified_dreamcast_product_code: command.selection.verified_dreamcast_product_code.clone(),
        content_path: command.selection.content_path.clone(),
    }
}

/// What the background watcher thread reports once the process has exited.
pub use crate::launch::process_spawn::ProcessExitReport as FlycastLaunchExitReport;

/// A spawned, still-owned Flycast process. Never automatically killed,
/// timed out, or relaunched by this module - Flycast is a long-running,
/// user-facing program the caller (a future GUI) owns for as long as the
/// user wants it running. [`Self::poll`] is the narrow, non-blocking way to
/// notice a normal play-session exit has happened.
pub struct LaunchedFlycastProcess {
    pub pid: u32,
    pub command_facts: FlycastLaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedFlycastProcess {
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

/// Spawns exactly the process `command` describes - never a shell.
/// `command` must already have passed [`preflight_flycast_launch`]; this
/// function performs no further validation of its own beyond what
/// [`crate::launch::process_spawn::spawn_watched_process`] itself requires
/// to spawn. See that function's own doc comment for the exact lifecycle
/// policy (stdin/stdout/stderr, environment, no timeout/kill).
pub fn spawn_flycast(
    command: FlycastCommand,
) -> Result<LaunchedFlycastProcess, FlycastLaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(FlycastLaunchSpawnError::Spawn)?;
    Ok(LaunchedFlycastProcess {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

// ---------------------------------------------------------------------------
// Convenience: preflight + spawn in one call
// ---------------------------------------------------------------------------

/// Composes [`preflight_flycast_launch`] and [`spawn_flycast`] - the single
/// call a future GUI Launch button would make. Kept as two separate public
/// functions above so preflight-only rejection scenarios can be tested
/// without ever spawning a real process.
pub fn preflight_and_launch_flycast(
    request: &FlycastLaunchRequest,
    roots: &FlycastProfileDiscoveryRoots,
) -> Result<LaunchedFlycastProcess, FlycastLaunchExecutionError> {
    let command = preflight_flycast_launch(request, roots)?;
    Ok(spawn_flycast(command)?)
}

#[cfg(test)]
mod tests;
