//! First supported slice of real native DuckStation launch execution:
//! safely revalidating and spawning exactly one native DuckStation process
//! for one direct, loose, regular PS1 content file.
//!
//! # Scope (first slice)
//!
//! - Native DuckStation profiles only - `FlatpakUser`, `Portable`, and
//!   `Explicit` (and any profile
//!   [`crate::patch_manager::resolve_duckstation_native_launch_binding`]
//!   itself refuses) are never attempted.
//! - `PSX` only - the only platform
//!   [`crate::launch::duckstation_command::DUCKSTATION_SUPPORTED_PLATFORM_ID`]
//!   names in this phase.
//! - One direct loose regular `.iso`, validated complete `.cue`/`.bin`, or
//!   `.chd` file - no archive members, no mounted content. See
//!   `duckstation_command`'s own module doc for the validation contract.
//! - A verified PS1 serial is always required.
//! - Strictly [`LaunchReadiness::Ready`] - `ReadyWithWarnings` and
//!   `Blocked` are both refused, never silently accepted.
//! - Exactly one requested, already-discovered DuckStation profile, matched
//!   by profile id - never a silent substitution of a different profile or
//!   executable the fresh discovery happened to also find.
//!
//! # Exact argv contract, and why `-batch`
//!
//! `[duckstation-qt] -batch -- [content]`. Proven from the current upstream
//! `stenzek/duckstation` source (`src/duckstation-qt/qthost.cpp`'s argument
//! parser - see `duckstation_local.rs`'s own binding doc comment for the
//! full citation): `-batch` "enables batch mode (exits after powering
//! off)". Without it, closing a game returns to DuckStation's own open
//! game-list frontend window rather than exiting the process, so a watcher
//! modeled on "the process exits when the play session ends" (the same
//! model RetroArch/Dolphin/PCSX2 already use) would never observe a normal
//! exit - only a user-initiated quit of the whole frontend. `-batch` does
//! not change anything about the emulated game itself while it is running,
//! only what the frontend does after the console powers off, so this is a
//! safe, non-behavior-altering choice for the watcher's benefit. `--`
//! guarantees the trailing content path is always parsed as the boot
//! filename, never as a flag, even if a future content path happens to
//! start with `-`.
//!
//! # What this module is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never launches Flatpak/Portable/AppImage/`Explicit` DuckStation,
//!   RetroArch, Dolphin, or PCSX2.
//! - It never touches cheats, mods, RomM, DAT, Library View History, ES-DE
//!   writes, or the shared transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/argument list
//!   always comes from
//!   [`crate::launch::duckstation_command::build_duckstation_command_plan`],
//!   rebuilt fresh from a freshly re-validated identity/content/profile/
//!   binding (see [`preflight_duckstation_launch`]'s own doc comment for
//!   exactly why the old readiness a caller may have seen earlier is never
//!   trusted alone).
//! - It never adds an automatic timeout, kill, or relaunch - DuckStation is
//!   a long-running, user-facing process the caller owns; see
//!   [`LaunchedDuckStationProcess`]'s own doc comment.
//! - It never downloads a DAT, fabricates firmware evidence, or treats
//!   missing/filename-only BIOS evidence as `Verified` - firmware evidence
//!   is always an already-loaded `&[FirmwareIdentityRecord]` slice the
//!   caller supplies; see step 8 below.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::dat::firmware_evidence::FirmwareIdentityRecord;
use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
use crate::game_identity::inspect_catalogued_game_identity;
use crate::ingestion::cue_bin::resolve_cue_all_files;
use crate::launch::duckstation_command::{
    DUCKSTATION_SUPPORTED_PLATFORM_ID, DuckStationCommand, build_duckstation_command_plan,
    direct_ps1_extension,
};
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
use crate::patch_manager::{
    DuckStationGameRequest, DuckStationProfileDiscoveryRoots, DuckStationUserDirectoryMode,
    discover_duckstation_profiles, inspect_duckstation_game_with_firmware_evidence,
    resolve_duckstation_native_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native DuckStation launch is being requested. Never an
/// arbitrary command string - every field here only ever *selects* which
/// already-discovered profile/binding to revalidate and launch; none of it
/// is ever passed to a shell or used to build argv directly (see
/// [`preflight_duckstation_launch`]).
///
/// `expected_platform_id`/`expected_game_key` are the caller's already
/// approved [`CanonicalIdentityStatus::Resolved`] fields, re-checked fresh
/// at preflight time. `expected_ps1_serial` is the verified PS1 serial the
/// caller already approved - re-checked fresh at preflight time.
///
/// `expected_executable`/`expected_user_directory_mode` are the exact
/// launch binding facts the user was shown at readiness time. A freshly
/// resolved binding that differs from either is treated as drift and
/// refused rather than silently substituted - see step 7 of
/// [`preflight_duckstation_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationLaunchRequest {
    /// The exact, direct content file the user selected - never an outer
    /// archive path and never a mount point.
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub expected_ps1_serial: String,
    /// Which discovered [`crate::patch_manager::DuckStationProfile::profile_id`]
    /// the binding must belong to.
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub expected_user_directory_mode: DuckStationUserDirectoryMode,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckStationLaunchPreflightErrorKind {
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
    /// The requested content path is not a direct `.iso`/`.chd` file.
    ContentFormatUnsupported,
    /// Fresh re-inspection produced `Unknown` or `Conflicting` identity -
    /// never resolved to one trustworthy answer.
    IdentityUnresolved,
    /// Fresh identity resolved, but its platform or game key differs from
    /// what the request expected - the content at this path is not the
    /// game the user approved, or it is not `PSX`.
    IdentityMismatch,
    /// Fresh re-inspection found no verified PS1 serial for this content.
    Ps1SerialUnavailable,
    /// Fresh re-inspection found a verified PS1 serial, but it differs from
    /// `request.expected_ps1_serial`.
    Ps1SerialMismatch,
    /// Fresh DuckStation profile discovery itself failed (e.g. no home
    /// directory could be determined).
    DiscoveryFailed,
    /// No discovered DuckStation profile matches
    /// [`DuckStationLaunchRequest::profile_id`] - never substituted with a
    /// different profile.
    ProfileNotFound,
    /// [`resolve_duckstation_native_launch_binding`] itself refused to
    /// produce a binding for the matched profile.
    BindingUnavailable,
    /// The freshly resolved binding's executable/user-directory mode no
    /// longer matches what the request expected - the binding drifted
    /// between readiness time and this click, so it is never silently
    /// substituted.
    BindingDrift,
    /// No candidate in the freshly rebuilt plan matches the requested
    /// DuckStation profile.
    RequestedCandidateNotFound,
    /// The matched candidate's own readiness is not exactly
    /// [`LaunchReadiness::Ready`] - covers both `ReadyWithWarnings` and
    /// `Blocked`. This is also what a not-`Verified` BIOS surfaces as: the
    /// existing readiness projection (`duckstation_firmware_readiness`)
    /// only ever reports `FirmwareReadiness::Verified` when
    /// [`crate::patch_manager::DuckStationBiosVerificationOutcome::Verified`]
    /// was genuinely produced, and any other firmware state keeps the
    /// candidate from reaching strict `Ready` - see step 8's own doc
    /// comment.
    CandidateNotReady,
    /// The matched candidate's content is not the narrow, direct,
    /// non-mounted plain-file shape this phase supports, even though its
    /// readiness reported `Ready` - defense in depth against a future
    /// planner change silently widening what counts as "Ready".
    CandidateContentUnsupported,
    /// [`build_duckstation_command_plan`] itself reported blockers.
    CommandBlocked,
    /// [`build_duckstation_command_plan`] reported no blockers but also no
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
pub struct DuckStationLaunchPreflightError {
    pub kind: DuckStationLaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: DuckStationLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> DuckStationLaunchPreflightError {
    DuckStationLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum DuckStationLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum DuckStationLaunchExecutionError {
    Preflight(DuckStationLaunchPreflightError),
    Spawn(DuckStationLaunchSpawnError),
}

impl From<DuckStationLaunchPreflightError> for DuckStationLaunchExecutionError {
    fn from(error: DuckStationLaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<DuckStationLaunchSpawnError> for DuckStationLaunchExecutionError {
    fn from(error: DuckStationLaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`DuckStationCommand`] safe to spawn - or refuses with a
/// [`DuckStationLaunchPreflightError`] naming exactly why.
///
/// # Why nothing from before this call is trusted
///
/// See [`crate::launch::execution::preflight_retroarch_launch`]'s own doc
/// comment for the general rationale; the same reasoning applies here - the
/// content file, the DuckStation installation, and even the profile's user
/// directory can all have changed between whenever the user was shown
/// "Ready" and the moment they click Launch.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, be a regular file, not be
///    an outer archive/mount-input path, and have a `.iso`, `.cue`, or `.chd`
///    extension.
/// 3. A [`CapturedFileIdentity`] is captured from the content's current
///    metadata.
/// 4. The content is freshly re-identified via
///    [`inspect_catalogued_game_identity`] (never a caller-supplied old
///    report); the result must resolve to exactly
///    [`DUCKSTATION_SUPPORTED_PLATFORM_ID`]/`request.expected_game_key`,
///    and a verified PS1 serial matching `request.expected_ps1_serial`
///    must be present among the fresh evidence.
/// 5. DuckStation profiles are freshly rediscovered via
///    [`discover_duckstation_profiles`] - never a caller's cached
///    discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_duckstation_native_launch_binding`] is called fresh
///    against that profile; its executable and user-directory mode must
///    exactly equal
///    `request.expected_executable`/`expected_user_directory_mode` -
///    otherwise this is `BindingDrift`, which also naturally catches a
///    `portable.txt`/`settings.ini` file having appeared beside the
///    executable since readiness time (the resolver itself would then
///    refuse to bind at all, surfacing as `BindingUnavailable` instead).
/// 8. The standalone launch plan is rebuilt via the existing
///    [`build_launch_plan_from_results`] integration entry point (a single
///    [`DiscoveredStandaloneProfile::duckstation`] projected from the
///    matched profile and a fresh
///    [`inspect_duckstation_game_with_firmware_evidence`] call, so BIOS
///    readiness is genuinely `Verified` whenever the selected BIOS matches
///    `firmware_evidence`, not merely present - see
///    `crate::patch_manager::duckstation_firmware`'s own module doc
///    comment). The resulting candidate must be exactly
///    [`LaunchReadiness::Ready`] (which is unreachable unless the BIOS
///    genuinely verified, per the existing `duckstation_firmware_readiness`
///    projection) and still the narrow direct-plain-file shape this phase
///    supports.
/// 9. [`build_duckstation_command_plan`] is rebuilt from the fresh
///    identity/serial/candidate/binding; it must report no blockers and a
///    command.
/// 10. Immediately before returning: the executable is re-checked to still
///     exist, not be a symlink, be a regular file, and be marked
///     executable; the content is re-inspected once more and its
///     [`CapturedFileIdentity`] must still equal the one captured in step
///     3.
pub fn preflight_duckstation_launch(
    request: &DuckStationLaunchRequest,
    roots: &DuckStationProfileDiscoveryRoots,
    firmware_evidence: &[FirmwareIdentityRecord],
) -> Result<DuckStationCommand, DuckStationLaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: fresh identity re-inspection + verified PS1 serial ---
    let (identity_status, facts, verified_serial) = fresh_identity_status(content_path, request)?;

    // A CUE is runnable only after every structurally referenced companion is
    // present and safe. The identity reinspection above separately validates
    // the unambiguous PS1 data track; together these checks establish a
    // complete release before the pure command planner sees CueBin.
    let cue_bin_release = content_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"));
    if cue_bin_release {
        resolve_cue_all_files(content_path).map_err(|error| {
            preflight_error(
                DuckStationLaunchPreflightErrorKind::ContentFormatUnsupported,
                format!("CUE/BIN release is incomplete or unsafe: {error}"),
            )
        })?;
    }

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_duckstation_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                DuckStationLaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered DuckStation profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_duckstation_native_launch_binding(profile, roots);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            DuckStationLaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable
        || binding.user_directory_mode != request.expected_user_directory_mode
    {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized \
             executable/user-directory mode",
        ));
    }

    // --- 8: rebuild the plan via the existing integration entry point ---
    let content_ref = LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(if cue_bin_release {
            LaunchContainerKind::CueBin
        } else {
            LaunchContainerKind::PlainFile
        }),
        resolved_path: Some(content_path.clone()),
        requires_mount: false,
        provenance: "duckstation launch execution preflight: revalidated direct regular file"
            .to_string(),
    };
    let inspection_with_firmware = inspect_duckstation_game_with_firmware_evidence(
        profile,
        &DuckStationGameRequest {
            verified_ps1_serial: Some(verified_serial.clone()),
            emulator_serial: None,
            disc_contexts: Vec::new(),
            playlist_path: None,
        },
        firmware_evidence,
    );
    let standalone_profiles = [DiscoveredStandaloneProfile::duckstation(
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
            } => *adapter_id == "duckstation" && *profile_id == request.profile_id,
            LaunchTarget::RetroArchCore { .. } => false,
        })
        .ok_or_else(|| {
            preflight_error(
                DuckStationLaunchPreflightErrorKind::RequestedCandidateNotFound,
                "no candidate in the freshly rebuilt plan matches the requested DuckStation \
                 profile",
            )
        })?;

    // --- strict readiness + content-shape gate ---
    if candidate.readiness != LaunchReadiness::Ready {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::CandidateNotReady,
            format!(
                "requested candidate readiness is {:?}, not exactly Ready",
                candidate.readiness
            ),
        ));
    }
    if !matches!(
        candidate.content.container,
        Some(LaunchContainerKind::PlainFile | LaunchContainerKind::CueBin)
    ) || candidate.content.requires_mount
        || !candidate.content.has_runnable_path()
    {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::CandidateContentUnsupported,
            "candidate content is not a direct, non-mounted plain file",
        ));
    }

    // --- 9: rebuild the command plan ---
    let command_plan = build_duckstation_command_plan(
        &identity_status,
        Some(&verified_serial),
        candidate,
        &binding_result,
    );
    if !command_plan.blockers.is_empty() {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::CommandBlocked,
            format!(
                "command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    let command = command_plan.command.ok_or_else(|| {
        preflight_error(
            DuckStationLaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content file changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, DuckStationLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            DuckStationLaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::ContentNotRegularFile,
            "content path is not a regular file",
        ));
    }
    if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::ContentRequiresMount,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    if !direct_ps1_extension(path) {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::ContentFormatUnsupported,
            "only a direct .iso, .cue, or .chd file is supported by this native DuckStation launch \
             slice",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity_status(
    content_path: &Path,
    request: &DuckStationLaunchRequest,
) -> Result<
    (CanonicalIdentityStatus, Vec<VerifiedIdentityFact>, String),
    DuckStationLaunchPreflightError,
> {
    // `inspect_catalogued_game_identity`, not the plain `inspect_game_identity` -
    // this request only ever exists because the game was already
    // catalogued/identified earlier; the platform hint here is fixed to
    // this slice's own supported platform, never derived from this path's
    // name or extension.
    let report =
        inspect_catalogued_game_identity(content_path, Some(DUCKSTATION_SUPPORTED_PLATFORM_ID));
    let (identity_status, facts) = canonical_identity_from_game_report(&report);
    match &identity_status {
        CanonicalIdentityStatus::Resolved(resolved) => {
            if resolved.platform_id != DUCKSTATION_SUPPORTED_PLATFORM_ID
                || resolved.platform_id != request.expected_platform_id
                || resolved.game_key != request.expected_game_key
            {
                return Err(preflight_error(
                    DuckStationLaunchPreflightErrorKind::IdentityMismatch,
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
                DuckStationLaunchPreflightErrorKind::IdentityUnresolved,
                format!("fresh identity re-inspection produced {identity_status:?}"),
            ));
        }
    }
    let serial = facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::Ps1Serial(value) => Some(value.clone()),
        _ => None,
    });
    let Some(serial) = serial else {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::Ps1SerialUnavailable,
            "fresh identity re-inspection found no verified PS1 serial for this content",
        ));
    };
    if serial != request.expected_ps1_serial {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::Ps1SerialMismatch,
            format!(
                "resolved PS1 serial {serial} does not match expected {}",
                request.expected_ps1_serial
            ),
        ));
    }
    Ok((identity_status, facts, serial))
}

fn recheck_executable(path: &Path) -> Result<(), DuckStationLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            DuckStationLaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            DuckStationLaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                DuckStationLaunchPreflightErrorKind::ExecutableNotExecutable,
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
pub struct DuckStationLaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub user_directory_mode: DuckStationUserDirectoryMode,
    pub platform_id: String,
    pub verified_ps1_serial: String,
    pub content_path: PathBuf,
}

fn command_facts(command: &DuckStationCommand) -> DuckStationLaunchCommandFacts {
    DuckStationLaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        user_directory_mode: command.selection.user_directory_mode,
        platform_id: command.selection.platform_id.clone(),
        verified_ps1_serial: command.selection.verified_ps1_serial.clone(),
        content_path: command.selection.content_path.clone(),
    }
}

/// What the background watcher thread reports once the process has exited.
pub use crate::launch::process_spawn::ProcessExitReport as DuckStationLaunchExitReport;

/// A spawned, still-owned DuckStation process. Never automatically killed,
/// timed out, or relaunched by this module - the DuckStation Qt process is
/// a long-running, user-facing program the caller (a future GUI) owns for
/// as long as the user wants it running. `-batch` (see the module doc
/// comment) means a normal play session ending is itself what causes this
/// process to exit; [`Self::poll`] is the narrow, non-blocking way to
/// notice that has happened.
pub struct LaunchedDuckStationProcess {
    pub pid: u32,
    pub command_facts: DuckStationLaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedDuckStationProcess {
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
/// `command` must already have passed [`preflight_duckstation_launch`];
/// this function performs no further validation of its own beyond what
/// [`crate::launch::process_spawn::spawn_watched_process`] itself requires
/// to spawn. See that function's own doc comment for the exact lifecycle
/// policy (stdin/stdout/stderr, environment, no timeout/kill).
pub fn spawn_duckstation(
    command: DuckStationCommand,
) -> Result<LaunchedDuckStationProcess, DuckStationLaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched = process_spawn::spawn_watched_process(&prepared)
        .map_err(DuckStationLaunchSpawnError::Spawn)?;
    Ok(LaunchedDuckStationProcess {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

// ---------------------------------------------------------------------------
// Convenience: preflight + spawn in one call
// ---------------------------------------------------------------------------

/// Composes [`preflight_duckstation_launch`] and [`spawn_duckstation`] - the
/// single call a future GUI Launch button would make. Kept as two separate
/// public functions above so preflight-only rejection scenarios can be
/// tested without ever spawning a real process.
pub fn preflight_and_launch_duckstation(
    request: &DuckStationLaunchRequest,
    roots: &DuckStationProfileDiscoveryRoots,
    firmware_evidence: &[FirmwareIdentityRecord],
) -> Result<LaunchedDuckStationProcess, DuckStationLaunchExecutionError> {
    let command = preflight_duckstation_launch(request, roots, firmware_evidence)?;
    Ok(spawn_duckstation(command)?)
}

#[cfg(test)]
mod tests;
