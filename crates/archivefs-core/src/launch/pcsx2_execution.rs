//! First supported slice of real native PCSX2 launch execution: safely
//! revalidating and spawning exactly one native PCSX2 process for one
//! direct, loose, regular PS2 content file.
//!
//! # Scope (first slice)
//!
//! - Native PCSX2 profiles only - `NativeAlternate`, `Portable`,
//!   `FlatpakUser`, `FlatpakSystem`, and any profile
//!   [`crate::patch_manager::resolve_pcsx2_native_launch_binding`] itself
//!   refuses are never attempted.
//! - `PS2` only - the only platform
//!   [`crate::launch::pcsx2_command::PCSX2_SUPPORTED_PLATFORM_ID`] names in
//!   this phase.
//! - One direct loose regular `.iso` file - no archive members, no mounted
//!   content, no CHD.
//! - A verified PS2 serial is always required, even though
//!   [`crate::patch_manager::Pcsx2GameRequest`] can in principle authorize
//!   on a verified executable CRC alone - see
//!   [`crate::launch::pcsx2_command`]'s own module doc comment.
//! - Strictly [`LaunchReadiness::Ready`] - `ReadyWithWarnings` and
//!   `Blocked` are both refused, never silently accepted.
//! - Exactly one requested, already-discovered PCSX2 profile, matched by
//!   profile id - never a silent substitution of a different profile or
//!   executable the fresh discovery happened to also find.
//!
//! # What this module is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never launches Flatpak/Portable/AppImage/`NativeAlternate` PCSX2,
//!   RetroArch, or Dolphin.
//! - It never touches cheats, mods, RomM, DAT, Library View History, ES-DE
//!   writes, or the shared transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/argument list
//!   always comes from
//!   [`crate::launch::pcsx2_command::build_pcsx2_command_plan`], rebuilt
//!   fresh from a freshly re-validated identity/content/profile/binding
//!   (see [`preflight_pcsx2_launch`]'s own doc comment for exactly why the
//!   old readiness a caller may have seen earlier is never trusted alone).
//! - It never adds an automatic timeout, kill, or relaunch - PCSX2 is a
//!   long-running, user-facing process the caller owns; see
//!   [`LaunchedPcsx2Process`]'s own doc comment.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::dat::firmware_evidence::FirmwareIdentityRecord;
use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::game_identity::inspect_catalogued_game_identity;
use crate::launch::evidence_bridge::canonical_identity_from_game_report;
use crate::launch::input_projection::VerifiedIdentityFact;
use crate::launch::integration::{
    DiscoveredStandaloneProfile, LaunchPlanResults, build_launch_plan_from_results,
};
use crate::launch::pcsx2_command::{
    PCSX2_SUPPORTED_PLATFORM_ID, Pcsx2Command, build_pcsx2_command_plan, direct_ps2_extension,
};
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef, LaunchTarget,
};
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, ProcessExitReport, WatchedProcess,
};
use crate::launch::readiness::LaunchReadiness;
use crate::patch_manager::{
    Pcsx2GameRequest, Pcsx2ProfileDiscoveryRoots, Pcsx2UserDirectoryMode, discover_pcsx2_profiles,
    inspect_pcsx2_game_with_firmware_evidence, resolve_pcsx2_native_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native PCSX2 launch is being requested. Never an
/// arbitrary command string - every field here only ever *selects* which
/// already-discovered profile/binding to revalidate and launch; none of it
/// is ever passed to a shell or used to build argv directly (see
/// [`preflight_pcsx2_launch`]).
///
/// `expected_platform_id`/`expected_game_key` are the caller's already
/// approved [`CanonicalIdentityStatus::Resolved`] fields, re-checked fresh
/// at preflight time. `expected_ps2_serial` is the verified PS2 serial the
/// caller already approved - re-checked fresh and independently of
/// `expected_game_key` since a resolved identity's `game_key` could in
/// principle rest on an executable CRC alone.
///
/// `expected_executable`/`expected_user_directory_mode` are the exact
/// launch binding facts the user was shown at readiness time. A freshly
/// resolved binding that differs from either is treated as drift and
/// refused rather than silently substituted - see step 7 of
/// [`preflight_pcsx2_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2LaunchRequest {
    /// The exact, direct content file the user selected - never an outer
    /// archive path and never a mount point.
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub expected_ps2_serial: String,
    /// Which discovered [`crate::patch_manager::Pcsx2Profile::profile_id`]
    /// the binding must belong to.
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub expected_user_directory_mode: Pcsx2UserDirectoryMode,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pcsx2LaunchPreflightErrorKind {
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
    /// The requested content path is not a direct `.iso` file - no CHD in
    /// this phase.
    ContentFormatUnsupported,
    /// Fresh re-inspection produced `Unknown` or `Conflicting` identity -
    /// never resolved to one trustworthy answer.
    IdentityUnresolved,
    /// Fresh identity resolved, but its platform or game key differs from
    /// what the request expected - the content at this path is not the
    /// game the user approved, or it is not `PS2`.
    IdentityMismatch,
    /// Fresh re-inspection found no verified PS2 serial for this content at
    /// all (even though the resolved identity itself may still be valid,
    /// e.g. on a verified executable CRC alone) - this launch slice always
    /// requires one.
    Ps2SerialUnavailable,
    /// Fresh re-inspection found a verified PS2 serial, but it differs from
    /// `request.expected_ps2_serial`.
    Ps2SerialMismatch,
    /// Fresh PCSX2 profile discovery itself failed (e.g. no home directory
    /// could be determined).
    DiscoveryFailed,
    /// No discovered PCSX2 profile matches
    /// [`Pcsx2LaunchRequest::profile_id`] - never substituted with a
    /// different profile.
    ProfileNotFound,
    /// [`resolve_pcsx2_native_launch_binding`] itself refused to produce a
    /// binding for the matched profile.
    BindingUnavailable,
    /// The freshly resolved binding's executable/user-directory mode no
    /// longer matches what the request expected - the binding drifted
    /// between readiness time and this click, so it is never silently
    /// substituted.
    BindingDrift,
    /// No candidate in the freshly rebuilt plan matches the requested
    /// PCSX2 profile.
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
    /// [`build_pcsx2_command_plan`] itself reported blockers.
    CommandBlocked,
    /// [`build_pcsx2_command_plan`] reported no blockers but also no
    /// command - should be unreachable given the checks above, but never
    /// assumed.
    CommandMissing,
    /// The resolved executable no longer exists.
    ExecutableMissing,
    /// The resolved executable is a symlink or not a regular file.
    ExecutableUnsafe,
    /// The resolved executable is not marked executable.
    ExecutableNotExecutable,
    /// The resolved explicit PCSX2 data-path root no longer exists, is not
    /// a directory, or has become a symlink. Unreachable in this build
    /// (see [`Pcsx2UserDirectoryMode::ExplicitDataPath`]'s own doc
    /// comment), kept for forward compatibility.
    DataPathRootInvalid,
    /// The content file's filesystem identity at the final pre-spawn check
    /// no longer matches what was captured earlier in this same preflight
    /// call - the file was swapped underneath the launch.
    ContentChangedBeforeSpawn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2LaunchPreflightError {
    pub kind: Pcsx2LaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: Pcsx2LaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> Pcsx2LaunchPreflightError {
    Pcsx2LaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum Pcsx2LaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum Pcsx2LaunchExecutionError {
    Preflight(Pcsx2LaunchPreflightError),
    Spawn(Pcsx2LaunchSpawnError),
}

impl From<Pcsx2LaunchPreflightError> for Pcsx2LaunchExecutionError {
    fn from(error: Pcsx2LaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<Pcsx2LaunchSpawnError> for Pcsx2LaunchExecutionError {
    fn from(error: Pcsx2LaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`Pcsx2Command`] safe to spawn - or refuses with a
/// [`Pcsx2LaunchPreflightError`] naming exactly why.
///
/// # Why nothing from before this call is trusted
///
/// See [`crate::launch::execution::preflight_retroarch_launch`]'s own doc
/// comment for the general rationale; the same reasoning applies here - the
/// content file, the PCSX2 installation, and even the profile's data
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
///    [`PCSX2_SUPPORTED_PLATFORM_ID`]/`request.expected_game_key`, and a
///    verified PS2 serial matching `request.expected_ps2_serial` must be
///    present among the fresh evidence.
/// 5. PCSX2 profiles are freshly rediscovered via
///    [`discover_pcsx2_profiles`] - never a caller's cached discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_pcsx2_native_launch_binding`] is called fresh against that
///    profile; its executable and user-directory mode must exactly equal
///    `request.expected_executable`/`expected_user_directory_mode`.
/// 8. The standalone launch plan is rebuilt via the existing
///    [`build_launch_plan_from_results`] integration entry point (a single
///    [`DiscoveredStandaloneProfile::pcsx2`] projected from the matched
///    profile and a fresh
///    [`inspect_pcsx2_game_with_firmware_evidence`] call, so BIOS readiness
///    is genuinely `Verified` whenever the selected BIOS matches
///    `firmware_evidence`, not merely present - see
///    `crate::patch_manager::pcsx2_firmware`'s own module doc comment).
///    The resulting candidate must be exactly [`LaunchReadiness::Ready`]
///    and still the narrow direct-plain-file shape this phase supports.
/// 9. [`build_pcsx2_command_plan`] is rebuilt from the fresh identity/
///    serial/candidate/binding; it must report no blockers and a command.
/// 10. Immediately before returning: the executable is re-checked to still
///     exist, not be a symlink, be a regular file, and be marked
///     executable; an explicit data-path root (if present) is re-checked
///     to still exist, be a directory, and not be a symlink; the content is
///     re-inspected once more and its [`CapturedFileIdentity`] must still
///     equal the one captured in step 3.
pub fn preflight_pcsx2_launch(
    request: &Pcsx2LaunchRequest,
    roots: &Pcsx2ProfileDiscoveryRoots,
    firmware_evidence: &[FirmwareIdentityRecord],
) -> Result<Pcsx2Command, Pcsx2LaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: fresh identity re-inspection + verified PS2 serial ---
    let (identity_status, facts, verified_serial) = fresh_identity_status(content_path, request)?;

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_pcsx2_profiles(roots).map_err(|error| {
        preflight_error(
            Pcsx2LaunchPreflightErrorKind::DiscoveryFailed,
            error.to_string(),
        )
    })?;
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                Pcsx2LaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered PCSX2 profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_pcsx2_native_launch_binding(profile, roots);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            Pcsx2LaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable
        || binding.user_directory_mode != request.expected_user_directory_mode
    {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized \
             executable/user-directory mode",
        ));
    }

    // --- 8: rebuild the plan via the existing integration entry point ---
    let content_ref = LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(content_path.clone()),
        requires_mount: false,
        provenance: "pcsx2 launch execution preflight: revalidated direct regular file".to_string(),
    };
    let crc = facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::Ps2ExecutableCrc(value) => Some(value.clone()),
        _ => None,
    });
    let inspection_with_firmware = inspect_pcsx2_game_with_firmware_evidence(
        profile,
        &Pcsx2GameRequest {
            verified_ps2_serial: Some(verified_serial.clone()),
            verified_executable_crc: crc,
            emulator_serial: None,
        },
        firmware_evidence,
    );
    let standalone_profiles = [DiscoveredStandaloneProfile::pcsx2(
        profile,
        &inspection_with_firmware.inspection,
    )];
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
            } => *adapter_id == "pcsx2" && *profile_id == request.profile_id,
            LaunchTarget::RetroArchCore { .. } => false,
        })
        .ok_or_else(|| {
            preflight_error(
                Pcsx2LaunchPreflightErrorKind::RequestedCandidateNotFound,
                "no candidate in the freshly rebuilt plan matches the requested PCSX2 profile",
            )
        })?;

    // --- strict readiness + content-shape gate ---
    if candidate.readiness != LaunchReadiness::Ready {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::CandidateNotReady,
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
            Pcsx2LaunchPreflightErrorKind::CandidateContentUnsupported,
            "candidate content is not a direct, non-mounted plain file",
        ));
    }

    // --- 9: rebuild the command plan ---
    let command_plan = build_pcsx2_command_plan(
        &identity_status,
        Some(&verified_serial),
        candidate,
        &binding_result,
    );
    if !command_plan.blockers.is_empty() {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::CommandBlocked,
            format!(
                "command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    let command = command_plan.command.ok_or_else(|| {
        preflight_error(
            Pcsx2LaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    if let Pcsx2UserDirectoryMode::ExplicitDataPath(root) = &command.selection.user_directory_mode {
        recheck_datapath_root(root)?;
    }
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content file changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, Pcsx2LaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            Pcsx2LaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::ContentNotRegularFile,
            "content path is not a regular file",
        ));
    }
    if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::ContentRequiresMount,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    if !direct_ps2_extension(path) {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::ContentFormatUnsupported,
            "only a direct .iso file is supported by this native PCSX2 launch slice",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity_status(
    content_path: &Path,
    request: &Pcsx2LaunchRequest,
) -> Result<(CanonicalIdentityStatus, Vec<VerifiedIdentityFact>, String), Pcsx2LaunchPreflightError>
{
    // `inspect_catalogued_game_identity`, not the plain `inspect_game_identity` -
    // this request only ever exists because the game was already
    // catalogued/identified earlier; the platform hint here is fixed to
    // this slice's own supported platform, never derived from this path's
    // name or extension.
    let report = inspect_catalogued_game_identity(content_path, Some(PCSX2_SUPPORTED_PLATFORM_ID));
    let (identity_status, facts) = canonical_identity_from_game_report(&report);
    match &identity_status {
        CanonicalIdentityStatus::Resolved(resolved) => {
            if resolved.platform_id != PCSX2_SUPPORTED_PLATFORM_ID
                || resolved.platform_id != request.expected_platform_id
                || resolved.game_key != request.expected_game_key
            {
                return Err(preflight_error(
                    Pcsx2LaunchPreflightErrorKind::IdentityMismatch,
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
                Pcsx2LaunchPreflightErrorKind::IdentityUnresolved,
                format!("fresh identity re-inspection produced {identity_status:?}"),
            ));
        }
    }
    let serial = facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::Ps2Serial(value) => Some(value.clone()),
        _ => None,
    });
    let Some(serial) = serial else {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::Ps2SerialUnavailable,
            "fresh identity re-inspection found no verified PS2 serial for this content",
        ));
    };
    if serial != request.expected_ps2_serial {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::Ps2SerialMismatch,
            format!(
                "resolved PS2 serial {serial} does not match expected {}",
                request.expected_ps2_serial
            ),
        ));
    }
    Ok((identity_status, facts, serial))
}

fn recheck_executable(path: &Path) -> Result<(), Pcsx2LaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            Pcsx2LaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                Pcsx2LaunchPreflightErrorKind::ExecutableNotExecutable,
                "executable has no execute bit set",
            ));
        }
    }
    Ok(())
}

fn recheck_datapath_root(root: &Path) -> Result<(), Pcsx2LaunchPreflightError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::DataPathRootInvalid,
            format!("{} is a symlink", root.display()),
        )),
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::DataPathRootInvalid,
            format!("{} is not a directory", root.display()),
        )),
        Err(io_error) => Err(preflight_error(
            Pcsx2LaunchPreflightErrorKind::DataPathRootInvalid,
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
pub struct Pcsx2LaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub user_directory_mode: Pcsx2UserDirectoryMode,
    pub platform_id: String,
    pub verified_ps2_serial: String,
    pub content_path: PathBuf,
}

fn command_facts(command: &Pcsx2Command) -> Pcsx2LaunchCommandFacts {
    Pcsx2LaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        user_directory_mode: command.selection.user_directory_mode.clone(),
        platform_id: command.selection.platform_id.clone(),
        verified_ps2_serial: command.selection.verified_ps2_serial.clone(),
        content_path: command.selection.content_path.clone(),
    }
}

/// What the background watcher thread reports once the process has exited.
pub use crate::launch::process_spawn::ProcessExitReport as Pcsx2LaunchExitReport;

/// A spawned, still-owned PCSX2 process. Never automatically killed, timed
/// out, or relaunched by this module - the PCSX2 Qt process is a
/// long-running, user-facing program the caller (a future GUI) owns for as
/// long as the user wants it running. [`Self::poll`] is the narrow,
/// non-blocking way to notice it has exited.
pub struct LaunchedPcsx2Process {
    pub pid: u32,
    pub command_facts: Pcsx2LaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedPcsx2Process {
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
/// `command` must already have passed [`preflight_pcsx2_launch`]; this
/// function performs no further validation of its own beyond what
/// [`crate::launch::process_spawn::spawn_watched_process`] itself requires
/// to spawn. See that function's own doc comment for the exact lifecycle
/// policy (stdin/stdout/stderr, environment, no timeout/kill).
pub fn spawn_pcsx2(command: Pcsx2Command) -> Result<LaunchedPcsx2Process, Pcsx2LaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(Pcsx2LaunchSpawnError::Spawn)?;
    Ok(LaunchedPcsx2Process {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

// ---------------------------------------------------------------------------
// Convenience: preflight + spawn in one call
// ---------------------------------------------------------------------------

/// Composes [`preflight_pcsx2_launch`] and [`spawn_pcsx2`] - the single call
/// a future GUI Launch button would make. Kept as two separate public
/// functions above so preflight-only rejection scenarios can be tested
/// without ever spawning a real process.
pub fn preflight_and_launch_pcsx2(
    request: &Pcsx2LaunchRequest,
    roots: &Pcsx2ProfileDiscoveryRoots,
    firmware_evidence: &[FirmwareIdentityRecord],
) -> Result<LaunchedPcsx2Process, Pcsx2LaunchExecutionError> {
    let command = preflight_pcsx2_launch(request, roots, firmware_evidence)?;
    Ok(spawn_pcsx2(command)?)
}

#[cfg(test)]
mod tests;
