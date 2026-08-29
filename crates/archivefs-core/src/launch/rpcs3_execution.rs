//! First supported slice of real native RPCS3 launch execution: safely
//! revalidating and spawning exactly one native RPCS3 process for one
//! direct PS3 ISO or an already-resolved extracted PS3 game folder.
//!
//! # Scope (first slice)
//!
//! - Native RPCS3 profiles only - any profile
//!   [`crate::patch_manager::resolve_rpcs3_native_launch_binding`] itself
//!   refuses is never attempted.
//! - `PS3` only - the only platform
//!   [`crate::launch::rpcs3_command::RPCS3_SUPPORTED_PLATFORM_ID`] names.
//! - A direct `.iso` file or an already-resolved extracted PS3 game folder
//!   (a `PS3_GAME`/`PARAM.SFO` + `USRDIR/EBOOT.BIN` layout) - exactly the
//!   two shapes [`crate::launch::rpcs3_command::build_rpcs3_command_plan`]
//!   itself accepts. No PKG, archive-contained game, CHD, CUE/BIN, or
//!   mounted projection is supported here.
//! - A verified PS3 TITLE_ID is always required.
//! - [`LaunchReadiness::Ready`] **or** [`LaunchReadiness::ReadyWithWarnings`],
//!   never [`LaunchReadiness::Blocked`]. See
//!   [`preflight_rpcs3_launch`]'s own doc comment for exactly why strict
//!   `Ready` alone (the policy every other adapter in this family uses) is
//!   not the right bar for RPCS3 specifically.
//! - Exactly one requested, already-discovered RPCS3 profile, matched by
//!   profile id - never a silent substitution of a different profile or
//!   executable the fresh discovery happened to also find.
//!
//! # What this module is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never launches Flatpak/Portable/Explicit RPCS3, RetroArch, PCSX2,
//!   xemu, PPSSPP, or Dolphin.
//! - It never touches cheats, mods, RomM, DAT, Library View History, ES-DE
//!   writes, or the shared transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/argument list
//!   always comes from
//!   [`crate::launch::rpcs3_command::build_rpcs3_command_plan`], rebuilt
//!   fresh from a freshly re-validated identity/content/profile/binding
//!   (see [`preflight_rpcs3_launch`]'s own doc comment for exactly why the
//!   old readiness a caller may have seen earlier is never trusted alone).
//! - It never adds Wine/Proton - native Linux RPCS3 only.
//! - It never mutates RPCS3 configuration or writes a per-game config file
//!   just to make a launch succeed.
//! - It never adds an automatic timeout, kill, or relaunch - RPCS3 is a
//!   long-running, user-facing process the caller owns; see
//!   [`LaunchedRpcs3Process`]'s own doc comment.

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
use crate::launch::rpcs3_command::{
    RPCS3_SUPPORTED_PLATFORM_ID, Rpcs3Command, build_rpcs3_command_plan,
    direct_ps3_content_is_supported,
};
use crate::patch_manager::{
    Rpcs3GameRequest, Rpcs3ProfileDiscoveryRoots, discover_rpcs3_profiles, inspect_rpcs3_game,
    resolve_rpcs3_native_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native RPCS3 launch is being requested. Never an
/// arbitrary command string - every field here only ever *selects* which
/// already-discovered profile/binding to revalidate and launch; none of it
/// is ever passed to a shell or used to build argv directly (see
/// [`preflight_rpcs3_launch`]).
///
/// `expected_platform_id`/`expected_game_key` are the caller's already
/// approved [`CanonicalIdentityStatus::Resolved`] fields, re-checked fresh at
/// preflight time. `expected_ps3_title_id` is the verified PS3 TITLE_ID the
/// caller already approved.
///
/// `expected_executable` is the exact launch binding fact the user was shown
/// at readiness time. A freshly resolved binding whose executable differs is
/// treated as drift and refused rather than silently substituted - see step
/// 7 of [`preflight_rpcs3_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3LaunchRequest {
    /// The exact, direct content the user selected - a `.iso` file or an
    /// already-resolved extracted PS3 game folder. Never an outer archive
    /// path and never a mount point.
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub expected_ps3_title_id: String,
    /// Which discovered [`crate::patch_manager::Rpcs3Profile::profile_id`]
    /// the binding must belong to.
    pub profile_id: String,
    pub expected_executable: PathBuf,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rpcs3LaunchPreflightErrorKind {
    /// The requested content path is not absolute.
    ContentPathNotAbsolute,
    /// The requested content path does not exist or could not be
    /// inspected.
    ContentNotFound,
    /// The requested content path is a symlink.
    ContentIsSymlink,
    /// The requested content path is not the shape its extension implies -
    /// a `.iso` path that is not a regular file, or an extensionless path
    /// that is not a directory (an extracted PS3 game folder is always
    /// extensionless).
    ContentNotRegularFile,
    /// The requested `.iso` content path is itself a mount-input archive
    /// (zip/7z/rar) - an outer archive path is never a runnable content
    /// path in this module.
    ContentRequiresMount,
    /// The requested content path is neither a direct `.iso` file nor an
    /// extensionless directory - PKG, CHD, CUE/BIN, and every other PS3
    /// representation are all refused here too.
    ContentFormatUnsupported,
    /// Fresh re-inspection produced `Unknown` or `Conflicting` identity -
    /// never resolved to one trustworthy answer.
    IdentityUnresolved,
    /// Fresh identity resolved, but its platform or game key differs from
    /// what the request expected - the content at this path is not the
    /// game the user approved, or it is not `PS3`.
    IdentityMismatch,
    /// Fresh re-inspection found no verified PS3 TITLE_ID for this content
    /// at all - this launch slice always requires one.
    Ps3TitleIdUnavailable,
    /// Fresh re-inspection found a verified PS3 TITLE_ID, but it differs
    /// from `request.expected_ps3_title_id`.
    Ps3TitleIdMismatch,
    /// No discovered RPCS3 profile matches
    /// [`Rpcs3LaunchRequest::profile_id`] - never substituted with a
    /// different profile.
    ProfileNotFound,
    /// [`resolve_rpcs3_native_launch_binding`] itself refused to produce a
    /// binding for the matched profile.
    BindingUnavailable,
    /// The freshly resolved binding's executable no longer matches what the
    /// request expected - the binding drifted between readiness time and
    /// this click, so it is never silently substituted.
    BindingDrift,
    /// No candidate in the freshly rebuilt plan matches the requested
    /// RPCS3 profile.
    RequestedCandidateNotFound,
    /// The matched candidate's own readiness is [`LaunchReadiness::Blocked`].
    CandidateNotReady,
    /// The matched candidate's content is not the narrow, direct,
    /// non-mounted plain-content shape this phase supports, even though its
    /// readiness reported ready - defense in depth against a future
    /// planner change silently widening what counts as ready.
    CandidateContentUnsupported,
    /// [`build_rpcs3_command_plan`] itself reported blockers (including
    /// unavailable firmware).
    CommandBlocked,
    /// [`build_rpcs3_command_plan`] reported no blockers but also no
    /// command - should be unreachable given the checks above, but never
    /// assumed.
    CommandMissing,
    /// The resolved executable no longer exists.
    ExecutableMissing,
    /// The resolved executable is a symlink or not a regular file.
    ExecutableUnsafe,
    /// The resolved executable is not marked executable.
    ExecutableNotExecutable,
    /// The content changed (swapped file/folder, size/mtime drift) between
    /// the initial capture and the final pre-spawn recheck.
    ContentChangedBeforeSpawn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3LaunchPreflightError {
    pub kind: Rpcs3LaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: Rpcs3LaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> Rpcs3LaunchPreflightError {
    Rpcs3LaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum Rpcs3LaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum Rpcs3LaunchExecutionError {
    Preflight(Rpcs3LaunchPreflightError),
    Spawn(Rpcs3LaunchSpawnError),
}

impl From<Rpcs3LaunchPreflightError> for Rpcs3LaunchExecutionError {
    fn from(error: Rpcs3LaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<Rpcs3LaunchSpawnError> for Rpcs3LaunchExecutionError {
    fn from(error: Rpcs3LaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`Rpcs3Command`] safe to spawn - or refuses with a
/// [`Rpcs3LaunchPreflightError`] naming exactly why.
///
/// # Why nothing from before this call is trusted
///
/// See [`crate::launch::execution::preflight_retroarch_launch`]'s own doc
/// comment for the general rationale; the same reasoning applies here - the
/// content, the RPCS3 installation, and even the profile's firmware can all
/// have changed between whenever the user was shown "Ready" and the moment
/// they click Launch.
///
/// # Why `ReadyWithWarnings` is accepted, unlike every sibling adapter
///
/// PCSX2/DuckStation reach a genuinely hash-verified
/// [`crate::launch::readiness::FirmwareReadiness::Verified`] via their own
/// `firmware_evidence`-aware inspection, which is what makes strict
/// [`LaunchReadiness::Ready`] a real, reachable bar for those adapters.
/// RPCS3's own landed firmware model
/// ([`crate::patch_manager::Rpcs3FirmwareStatus`]) never verifies firmware
/// contents at all - it has no `Verified`-equivalent variant, only
/// `Present`/`Missing`/`Unknown` - so a real, correctly-installed RPCS3
/// firmware directory can only ever project to
/// [`crate::launch::readiness::FirmwareReadiness::PresentUnverified`],
/// which the shared planner (`crate::launch::planning::firmware_condition`)
/// always turns into a warning, never strict `Ready`. Requiring strict
/// `Ready` here would make this execution slice permanently unusable for
/// every genuinely working RPCS3 installation - so, deliberately and only
/// for this adapter, [`LaunchReadiness::ReadyWithWarnings`] is accepted
/// too. [`LaunchReadiness::Blocked`] is still always refused, and
/// [`build_rpcs3_command_plan`]'s own stricter
/// [`crate::launch::readiness::FirmwareReadiness::Unknown`] gate still
/// applies unchanged afterward - this does not weaken firmware gating, it
/// only stops requiring a verification level this adapter's landed identity
/// layer does not yet provide.
///
/// # Why identity is re-verified twice
///
/// A direct `.iso` file is a single object: its own
/// [`CapturedFileIdentity`] fully captures a swap between planning and
/// spawn. An extracted PS3 game folder is not: a `PARAM.SFO`/`EBOOT.BIN`
/// edited in place, in most filesystems, never changes the containing
/// directory's own mtime, so a top-level [`CapturedFileIdentity`] compare
/// alone could miss it. Rather than reimplementing the existing
/// `PS3_GAME`/`PARAM.SFO`/`TITLE_ID`/`USRDIR`/`EBOOT.BIN` identity authority
/// here, step 10 below simply re-runs the exact same
/// [`inspect_catalogued_game_identity`]-based check step 4 already used -
/// so any PARAM.SFO/TITLE_ID/EBOOT drift, for either content shape, is
/// caught by the one identity layer that already owns those facts.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, and match its shape: a
///    `.iso` path must be a regular, non-mount-input file; an extensionless
///    path must be a real directory (an extracted PS3 game folder). Any
///    other extension is refused outright (PKG, CHD, CUE/BIN, etc.).
/// 3. A [`CapturedFileIdentity`] is captured from the content's current
///    metadata (works uniformly for a file or a directory).
/// 4. The content is freshly re-identified via
///    [`inspect_catalogued_game_identity`] (never a caller-supplied old
///    report); the result must resolve to exactly
///    [`RPCS3_SUPPORTED_PLATFORM_ID`]/`request.expected_game_key`, and a
///    verified PS3 TITLE_ID matching `request.expected_ps3_title_id` must
///    be present among the fresh evidence.
/// 5. RPCS3 profiles are freshly rediscovered via
///    [`discover_rpcs3_profiles`] - never a caller's cached discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_rpcs3_native_launch_binding`] is called fresh against that
///    profile; its executable must exactly equal
///    `request.expected_executable`.
/// 8. The standalone launch plan is rebuilt via the existing
///    [`build_launch_plan_from_results`] integration entry point (a single
///    [`DiscoveredStandaloneProfile::rpcs3`] projected from the matched
///    profile and a fresh [`inspect_rpcs3_game`] call). The resulting
///    candidate must be [`LaunchReadiness::Ready`] or
///    [`LaunchReadiness::ReadyWithWarnings`] (see above) and still the
///    narrow direct-plain-content shape this phase supports.
/// 9. [`build_rpcs3_command_plan`] is rebuilt from the fresh identity/title
///    id/candidate/binding; it must report no blockers and a command.
/// 10. Immediately before returning: the executable is re-checked to still
///     exist, not be a symlink, be a regular file, and be marked
///     executable; the content is re-inspected once more and its
///     [`CapturedFileIdentity`] must still equal the one captured in step
///     3; and the content is re-identified once more via the same fresh
///     identity check used in step 4, refusing any TITLE_ID/PARAM.SFO/EBOOT
///     drift.
pub fn preflight_rpcs3_launch(
    request: &Rpcs3LaunchRequest,
    roots: &Rpcs3ProfileDiscoveryRoots,
) -> Result<Rpcs3Command, Rpcs3LaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: fresh identity re-inspection + verified PS3 TITLE_ID ---
    let (identity_status, facts, verified_title_id) = fresh_identity_status(content_path, request)?;

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_rpcs3_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                Rpcs3LaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered RPCS3 profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_rpcs3_native_launch_binding(profile);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            Rpcs3LaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized executable",
        ));
    }

    // --- 8: rebuild the plan via the existing integration entry point ---
    let content_ref = LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(content_path.clone()),
        requires_mount: false,
        provenance: "rpcs3 launch execution preflight: revalidated direct content".to_string(),
    };
    let inspection = inspect_rpcs3_game(
        profile,
        &Rpcs3GameRequest {
            verified_ps3_title_id: Some(verified_title_id.clone()),
            ..Default::default()
        },
    );
    let standalone_profiles = [DiscoveredStandaloneProfile::rpcs3(profile, &inspection)];
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
            } => *adapter_id == "rpcs3" && *profile_id == request.profile_id,
            LaunchTarget::RetroArchCore { .. } => false,
        })
        .ok_or_else(|| {
            preflight_error(
                Rpcs3LaunchPreflightErrorKind::RequestedCandidateNotFound,
                "no candidate in the freshly rebuilt plan matches the requested RPCS3 profile",
            )
        })?;

    // --- readiness + content-shape gate (see module doc comment for why
    //     `ReadyWithWarnings` is accepted here, unlike sibling adapters) ---
    if candidate.readiness == LaunchReadiness::Blocked {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::CandidateNotReady,
            "requested candidate readiness is Blocked",
        ));
    }
    if candidate.content.container != Some(LaunchContainerKind::PlainFile)
        || candidate.content.requires_mount
        || !candidate.content.has_runnable_path()
    {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::CandidateContentUnsupported,
            "candidate content is not a direct, non-mounted plain content path",
        ));
    }

    // --- 9: rebuild the command plan ---
    let command_plan = build_rpcs3_command_plan(
        &identity_status,
        Some(&verified_title_id),
        candidate,
        &binding_result,
    );
    if !command_plan.blockers.is_empty() {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::CommandBlocked,
            format!(
                "command plan reported {} blocker(s)",
                command_plan.blockers.len()
            ),
        ));
    }
    let command = command_plan.command.ok_or_else(|| {
        preflight_error(
            Rpcs3LaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed during preflight",
        ));
    }
    // Re-run the same fresh identity re-inspection used in step 4: catches
    // PARAM.SFO/TITLE_ID/EBOOT drift a top-level directory mtime alone would
    // not (see the module doc comment).
    fresh_identity_status(&command.selection.content_path, request)?;

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, Rpcs3LaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            Rpcs3LaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !direct_ps3_content_is_supported(path) {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::ContentFormatUnsupported,
            "only a direct .iso file or an already-resolved extracted PS3 folder is supported - \
             PKG, CHD, CUE/BIN, and archive-contained games are all refused",
        ));
    }
    let has_iso_extension = path.extension().is_some();
    if has_iso_extension {
        if !metadata.is_file() {
            return Err(preflight_error(
                Rpcs3LaunchPreflightErrorKind::ContentNotRegularFile,
                "content path has a .iso extension but is not a regular file",
            ));
        }
        if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
            return Err(preflight_error(
                Rpcs3LaunchPreflightErrorKind::ContentRequiresMount,
                "content path is an outer archive/mount-input path, not direct content",
            ));
        }
    } else if !metadata.is_dir() {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::ContentNotRegularFile,
            "an extensionless content path must be a real directory (an extracted PS3 game \
             folder)",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity_status(
    content_path: &Path,
    request: &Rpcs3LaunchRequest,
) -> Result<(CanonicalIdentityStatus, Vec<VerifiedIdentityFact>, String), Rpcs3LaunchPreflightError>
{
    // `inspect_catalogued_game_identity`, not the plain `inspect_game_identity` -
    // this request only ever exists because the game was already
    // catalogued/identified earlier; the platform hint here is fixed to
    // this slice's own supported platform, never derived from this path's
    // name or extension.
    let report = inspect_catalogued_game_identity(content_path, Some(RPCS3_SUPPORTED_PLATFORM_ID));
    let (identity_status, facts) = canonical_identity_from_game_report(&report);
    match &identity_status {
        CanonicalIdentityStatus::Resolved(resolved) => {
            if resolved.platform_id != RPCS3_SUPPORTED_PLATFORM_ID
                || resolved.platform_id != request.expected_platform_id
                || resolved.game_key != request.expected_game_key
            {
                return Err(preflight_error(
                    Rpcs3LaunchPreflightErrorKind::IdentityMismatch,
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
                Rpcs3LaunchPreflightErrorKind::IdentityUnresolved,
                format!("fresh identity re-inspection produced {identity_status:?}"),
            ));
        }
    }
    let title_id = facts.iter().find_map(|fact| match fact {
        VerifiedIdentityFact::Ps3TitleId(value) => Some(value.clone()),
        _ => None,
    });
    let Some(title_id) = title_id else {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::Ps3TitleIdUnavailable,
            "fresh identity re-inspection found no verified PS3 TITLE_ID for this content",
        ));
    };
    if title_id != request.expected_ps3_title_id {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::Ps3TitleIdMismatch,
            format!(
                "resolved PS3 TITLE_ID {title_id} does not match expected {}",
                request.expected_ps3_title_id
            ),
        ));
    }
    Ok((identity_status, facts, title_id))
}

fn recheck_executable(path: &Path) -> Result<(), Rpcs3LaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            Rpcs3LaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            Rpcs3LaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                Rpcs3LaunchPreflightErrorKind::ExecutableNotExecutable,
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
pub struct Rpcs3LaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub platform_id: String,
    pub verified_ps3_title_id: String,
    pub content_path: PathBuf,
}

fn command_facts(command: &Rpcs3Command) -> Rpcs3LaunchCommandFacts {
    Rpcs3LaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        platform_id: command.selection.platform_id.clone(),
        verified_ps3_title_id: command.selection.verified_ps3_title_id.clone(),
        content_path: command.selection.content_path.clone(),
    }
}

/// What the background watcher thread reports once the process has exited.
pub use crate::launch::process_spawn::ProcessExitReport as Rpcs3LaunchExitReport;

/// A spawned, still-owned RPCS3 process. Never automatically killed, timed
/// out, or relaunched by this module - RPCS3 is a long-running, user-facing
/// program the caller (a future GUI) owns for as long as the user wants it
/// running. [`Self::poll`] is the narrow, non-blocking way to notice it has
/// exited.
pub struct LaunchedRpcs3Process {
    pub pid: u32,
    pub command_facts: Rpcs3LaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedRpcs3Process {
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
/// must already have passed [`preflight_rpcs3_launch`]; this function
/// performs no further validation of its own beyond what
/// [`crate::launch::process_spawn::spawn_watched_process`] itself requires
/// to spawn. See that function's own doc comment for the exact lifecycle
/// policy (stdin/stdout/stderr, environment, no timeout/kill).
pub fn spawn_rpcs3(command: Rpcs3Command) -> Result<LaunchedRpcs3Process, Rpcs3LaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(Rpcs3LaunchSpawnError::Spawn)?;
    Ok(LaunchedRpcs3Process {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

// ---------------------------------------------------------------------------
// Convenience: preflight + spawn in one call
// ---------------------------------------------------------------------------

/// Composes [`preflight_rpcs3_launch`] and [`spawn_rpcs3`] - the single
/// call a future GUI Launch button would make. Kept as two separate public
/// functions above so preflight-only rejection scenarios can be tested
/// without ever spawning a real process.
pub fn preflight_and_launch_rpcs3(
    request: &Rpcs3LaunchRequest,
    roots: &Rpcs3ProfileDiscoveryRoots,
) -> Result<LaunchedRpcs3Process, Rpcs3LaunchExecutionError> {
    let command = preflight_rpcs3_launch(request, roots)?;
    Ok(spawn_rpcs3(command)?)
}

#[cfg(test)]
mod tests;
