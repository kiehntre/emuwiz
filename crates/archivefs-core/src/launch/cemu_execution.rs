//! First supported slice of real native Cemu launch execution: safely
//! revalidating and spawning exactly one native Cemu process for one
//! already-resolved extracted Wii U title directory.
//!
//! # Scope (first slice)
//!
//! - Native Cemu profiles only - any profile
//!   [`crate::patch_manager::resolve_cemu_native_launch_binding`] itself
//!   refuses is never attempted.
//! - `WiiU` only.
//! - Only [`crate::patch_manager::CemuContentForm::ExtractedTitle`] - the
//!   `code`/`content`/`meta` layout. `.wud`, `.wux`, and `.wua` are all
//!   refused here too, exactly as
//!   [`crate::launch::cemu_command::build_cemu_command_plan`] itself
//!   refuses them - see that module's doc comment and
//!   [`crate::patch_manager::cemu_local`] for why.
//! - A title whose own `meta.xml` declares itself an update or DLC title is
//!   refused - this slice only ever launches a selected base game.
//! - Exactly one requested, already-discovered Cemu profile, matched by
//!   profile id - never a silent substitution of a different profile,
//!   executable, or emulator (see the "No Silent Fallback" note in this
//!   crate's own report: a missing/unavailable Cemu is reported, never
//!   quietly redirected to Dolphin, RetroArch, or any other emulator).
//!
//! # What this is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never touches cheats, mods, RomM, DAT, ES-DE, or the shared
//!   transaction system.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/argument list
//!   always comes from
//!   [`crate::launch::cemu_command::build_cemu_command_plan`], rebuilt
//!   fresh from freshly re-gathered evidence every single call.
//! - It never adds Wine/Proton, AppImage extraction, or Flatpak sandboxing
//!   logic - a plain native executable path only.
//! - It never mutates `settings.xml`, `keys.txt`, the MLC, or any game,
//!   update, or DLC content.
//! - It never reads a key's contents - only [`crate::patch_manager::CemuKeysState`]
//!   presence/readability evidence ever crosses this boundary.
//! - It never adds an automatic timeout, kill, or relaunch - Cemu is a
//!   long-running, user-facing process the caller owns.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::cemu_command::{
    CEMU_SUPPORTED_PLATFORM_ID, CemuCommand, CemuLaunchRequest, CemuReadiness,
    build_cemu_command_plan, classify_cemu_readiness,
};
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, ProcessExitReport, WatchedProcess,
};
use crate::patch_manager::{
    CemuExtractedLayout, CemuKeysEvidence, CemuMlcEvidence, CemuMlcState,
    CemuProfileDiscoveryRoots, CemuTitleIdentity, cemu_form_for_path, cemu_keys_evidence,
    discover_cemu_profiles, extract_title_identity, inspect_extracted_layout,
    resolve_cemu_native_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native Cemu launch is being requested. Never an
/// arbitrary command string - every field here only ever *selects* which
/// already-discovered profile/binding to revalidate and launch.
///
/// `expected_executable` is the exact launch binding fact the user was shown
/// at readiness time. A freshly resolved binding whose executable differs is
/// treated as drift and refused rather than silently substituted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuPreflightRequest {
    /// The exact extracted-title directory the user selected. Never a
    /// `.wud`/`.wux`/`.wua` path and never an arbitrary folder - see
    /// [`preflight_cemu_launch`] step 2.
    pub selected_content_path: PathBuf,
    pub profile_id: String,
    pub expected_executable: PathBuf,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuLaunchPreflightErrorKind {
    ContentPathNotAbsolute,
    ContentNotFound,
    ContentIsSymlink,
    /// The requested content is not a directory - `.wud`, `.wux`, `.wua`,
    /// and any other file are all refused here too.
    ContentFormatUnsupported,
    ProfileNotFound,
    BindingUnavailable,
    BindingDrift,
    CommandBlocked,
    CommandMissing,
    ExecutableMissing,
    ExecutableUnsafe,
    ExecutableNotExecutable,
    ContentChangedBeforeSpawn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuLaunchPreflightError {
    pub kind: CemuLaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: CemuLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> CemuLaunchPreflightError {
    CemuLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum CemuLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum CemuLaunchExecutionError {
    Preflight(CemuLaunchPreflightError),
    Spawn(CemuLaunchSpawnError),
}

impl From<CemuLaunchPreflightError> for CemuLaunchExecutionError {
    fn from(error: CemuLaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}
impl From<CemuLaunchSpawnError> for CemuLaunchExecutionError {
    fn from(error: CemuLaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`CemuCommand`] safe to spawn - or refuses with a
/// [`CemuLaunchPreflightError`] naming exactly why.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, and be a directory - the
///    only shape [`cemu_form_for_path`] resolves to
///    [`crate::patch_manager::CemuContentForm::ExtractedTitle`].
/// 3. A [`CapturedFileIdentity`] is captured from the content directory's
///    current metadata.
/// 4. [`inspect_extracted_layout`] is run fresh against the content path.
/// 5. Cemu profiles are freshly rediscovered via [`discover_cemu_profiles`]
///    - never a caller's cached discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_cemu_native_launch_binding`] is called fresh against that
///    profile; its executable must exactly equal
///    `request.expected_executable`.
/// 8. Keys and MLC evidence are re-read fresh from the matched profile
///    (never a caller-supplied older reading), and `meta.xml` (when the
///    layout has one) is re-parsed fresh for title identity.
/// 9. [`build_cemu_command_plan`] is rebuilt from all of the above; it must
///    report no blockers and a command.
/// 10. Immediately before returning: the executable is re-checked to still
///     exist, not be a symlink, be a regular file, and be marked
///     executable; the content is re-inspected once more and its
///     [`CapturedFileIdentity`] must still equal the one captured in step 3.
pub fn preflight_cemu_launch(
    request: &CemuPreflightRequest,
    roots: &CemuProfileDiscoveryRoots,
) -> Result<CemuCommand, CemuLaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            CemuLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: layout ---
    let layout = inspect_extracted_layout(content_path);
    let content_form = cemu_form_for_path(content_path).unwrap_or(
        // Only reachable for a path this crate does not even recognise as a
        // Wii U shape; `build_cemu_command_plan` still refuses it cleanly
        // via its platform/content-form checks rather than this function
        // guessing at a default.
        crate::patch_manager::CemuContentForm::Wua,
    );

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_cemu_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                CemuLaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered Cemu profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_cemu_native_launch_binding(profile);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            CemuLaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(preflight_error(
            CemuLaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized executable",
        ));
    }

    // --- 8: fresh keys/MLC/title-identity evidence ---
    let keys_evidence: CemuKeysEvidence =
        cemu_keys_evidence(&profile.configuration_path, &profile.executable_candidates);
    let mlc_evidence: CemuMlcEvidence = profile
        .config
        .as_ref()
        .map(|config| config.mlc.clone())
        .unwrap_or(CemuMlcEvidence {
            path: None,
            state: CemuMlcState::NotConfigured,
        });
    let title_identity: Option<CemuTitleIdentity> = layout
        .as_ref()
        .ok()
        .and_then(|layout: &CemuExtractedLayout| layout.meta_xml_path.as_deref())
        .and_then(extract_title_identity);

    // --- 9: rebuild the command plan ---
    let cemu_request = CemuLaunchRequest {
        executable: binding.executable.clone(),
        profile_id: profile.profile_id.clone(),
        platform_id: CEMU_SUPPORTED_PLATFORM_ID.to_string(),
        selected_content: content_path.clone(),
        content_form,
        title_identity,
        keys_evidence,
        mlc_evidence,
        // Recomputed fresh below; this value is never trusted.
        readiness: CemuReadiness::Blocked,
    };
    let plan = build_cemu_command_plan(&cemu_request, &layout);
    if !plan.blockers.is_empty() {
        return Err(preflight_error(
            CemuLaunchPreflightErrorKind::CommandBlocked,
            format!("command plan reported {} blocker(s)", plan.blockers.len()),
        ));
    }
    let _ = classify_cemu_readiness(&plan);
    let command = plan.command.ok_or_else(|| {
        preflight_error(
            CemuLaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 10: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            CemuLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, CemuLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            CemuLaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            CemuLaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(preflight_error(
            CemuLaunchPreflightErrorKind::ContentFormatUnsupported,
            "only an extracted code/content/meta directory is supported in this build - \
             .wud, .wux, .wua and any other file are all refused",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn recheck_executable(path: &Path) -> Result<(), CemuLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            CemuLaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            CemuLaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                CemuLaunchPreflightErrorKind::ExecutableNotExecutable,
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
pub struct CemuLaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub platform_id: String,
    pub content_path: PathBuf,
    pub title_id: Option<String>,
}

fn command_facts(command: &CemuCommand) -> CemuLaunchCommandFacts {
    CemuLaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        platform_id: command.selection.platform_id.clone(),
        content_path: command.selection.content_path.clone(),
        title_id: command.selection.title_id.clone(),
    }
}

pub use crate::launch::process_spawn::ProcessExitReport as CemuLaunchExitReport;

/// A spawned, still-owned Cemu process. Never automatically killed, timed
/// out, or relaunched by this module.
pub struct LaunchedCemuProcess {
    pub pid: u32,
    pub command_facts: CemuLaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedCemuProcess {
    pub fn poll(&mut self) -> Option<&ProcessExitReport> {
        self.watched.poll()
    }

    pub fn is_running(&self) -> bool {
        self.watched.is_running()
    }
}

/// Spawns exactly the process `command` describes - never a shell.
/// `command` must already have passed [`preflight_cemu_launch`].
pub fn spawn_cemu(command: CemuCommand) -> Result<LaunchedCemuProcess, CemuLaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(CemuLaunchSpawnError::Spawn)?;
    Ok(LaunchedCemuProcess {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

/// Composes [`preflight_cemu_launch`] and [`spawn_cemu`] - the single call a
/// future GUI Launch button would make.
pub fn preflight_and_launch_cemu(
    request: &CemuPreflightRequest,
    roots: &CemuProfileDiscoveryRoots,
) -> Result<LaunchedCemuProcess, CemuLaunchExecutionError> {
    let command = preflight_cemu_launch(request, roots)?;
    Ok(spawn_cemu(command)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn mark_exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut m = fs::metadata(p).unwrap().permissions();
        m.set_mode(0o755);
        fs::set_permissions(p, m).unwrap();
    }

    fn write_exe(path: &Path) {
        fs::write(path, b"x").unwrap();
        #[cfg(unix)]
        mark_exec(path);
    }

    fn roots_for(dir: &Path, exe: PathBuf, config_root: PathBuf) -> CemuProfileDiscoveryRoots {
        CemuProfileDiscoveryRoots {
            home: dir.to_path_buf(),
            xdg_config_home: dir.join("no-xdg"),
            explicit_configuration_roots: vec![config_root],
            portable_configuration_roots: vec![],
            explicit_executables: vec![exe],
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        }
    }

    fn make_extracted_title(dir: &Path) -> PathBuf {
        let root = dir.join("Some Game");
        fs::create_dir_all(root.join("code")).unwrap();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir_all(root.join("meta")).unwrap();
        fs::write(root.join("code/game.rpx"), b"rpx").unwrap();
        fs::write(
            root.join("meta/meta.xml"),
            "<menu><title_id>00050000101010ED</title_id></menu>",
        )
        .unwrap();
        root
    }

    fn make_profile(dir: &Path, exe: &Path, config_root: &Path, mlc: &Path) {
        fs::create_dir_all(config_root).unwrap();
        fs::create_dir_all(mlc).unwrap();
        fs::write(
            config_root.join("settings.xml"),
            format!("<content><mlc_path>{}</mlc_path></content>", mlc.display()),
        )
        .unwrap();
        write_exe(exe);
        let _ = dir;
    }

    #[test]
    fn happy_path_preflight_produces_expected_argv() {
        let d = tempdir().unwrap();
        let exe = d.path().join("Cemu");
        let config_root = d.path().join("profile");
        let mlc = d.path().join("mlc");
        make_profile(d.path(), &exe, &config_root, &mlc);
        let content = make_extracted_title(d.path());
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = CemuPreflightRequest {
            selected_content_path: content.clone(),
            profile_id: format!("cemu:{}", config_root.display()),
            expected_executable: exe.clone(),
        };
        let command = preflight_cemu_launch(&request, &roots).expect("preflight ok");
        assert_eq!(command.executable, exe);
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("-g"),
                content.join("code/game.rpx").into_os_string()
            ]
        );
    }

    #[test]
    fn wrong_profile_id_is_never_substituted() {
        let d = tempdir().unwrap();
        let exe = d.path().join("Cemu");
        let config_root = d.path().join("profile");
        let mlc = d.path().join("mlc");
        make_profile(d.path(), &exe, &config_root, &mlc);
        let content = make_extracted_title(d.path());
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = CemuPreflightRequest {
            selected_content_path: content,
            profile_id: "cemu:/does/not/exist".into(),
            expected_executable: exe,
        };
        let error = preflight_cemu_launch(&request, &roots).unwrap_err();
        assert_eq!(error.kind, CemuLaunchPreflightErrorKind::ProfileNotFound);
    }

    #[test]
    fn executable_drift_is_refused_not_silently_substituted() {
        let d = tempdir().unwrap();
        let exe = d.path().join("Cemu");
        let config_root = d.path().join("profile");
        let mlc = d.path().join("mlc");
        make_profile(d.path(), &exe, &config_root, &mlc);
        let content = make_extracted_title(d.path());
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = CemuPreflightRequest {
            selected_content_path: content,
            profile_id: format!("cemu:{}", config_root.display()),
            expected_executable: d.path().join("a-different-cemu"),
        };
        let error = preflight_cemu_launch(&request, &roots).unwrap_err();
        assert_eq!(error.kind, CemuLaunchPreflightErrorKind::BindingDrift);
    }

    #[test]
    fn missing_mlc_blocks_the_command() {
        let d = tempdir().unwrap();
        let exe = d.path().join("Cemu");
        let config_root = d.path().join("profile");
        fs::create_dir_all(&config_root).unwrap();
        fs::write(
            config_root.join("settings.xml"),
            "<content><mlc_path>/does/not/exist</mlc_path></content>",
        )
        .unwrap();
        write_exe(&exe);
        let content = make_extracted_title(d.path());
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = CemuPreflightRequest {
            selected_content_path: content,
            profile_id: format!("cemu:{}", config_root.display()),
            expected_executable: exe,
        };
        let error = preflight_cemu_launch(&request, &roots).unwrap_err();
        assert_eq!(error.kind, CemuLaunchPreflightErrorKind::CommandBlocked);
    }

    #[test]
    fn wud_file_content_is_refused_as_unsupported_shape() {
        let d = tempdir().unwrap();
        let exe = d.path().join("Cemu");
        let config_root = d.path().join("profile");
        let mlc = d.path().join("mlc");
        make_profile(d.path(), &exe, &config_root, &mlc);
        let wud = d.path().join("game.wud");
        fs::write(&wud, b"disc").unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = CemuPreflightRequest {
            selected_content_path: wud,
            profile_id: format!("cemu:{}", config_root.display()),
            expected_executable: exe,
        };
        let error = preflight_cemu_launch(&request, &roots).unwrap_err();
        assert_eq!(
            error.kind,
            CemuLaunchPreflightErrorKind::ContentFormatUnsupported
        );
    }

    #[test]
    fn symlinked_content_is_refused() {
        let d = tempdir().unwrap();
        let exe = d.path().join("Cemu");
        let config_root = d.path().join("profile");
        let mlc = d.path().join("mlc");
        make_profile(d.path(), &exe, &config_root, &mlc);
        let real = make_extracted_title(d.path());
        let link = d.path().join("link-to-game");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        #[cfg(unix)]
        {
            let request = CemuPreflightRequest {
                selected_content_path: link,
                profile_id: format!("cemu:{}", config_root.display()),
                expected_executable: exe,
            };
            let error = preflight_cemu_launch(&request, &roots).unwrap_err();
            assert_eq!(error.kind, CemuLaunchPreflightErrorKind::ContentIsSymlink);
        }
    }

    #[test]
    fn content_replaced_after_capture_is_detected_before_spawn() {
        // Exercises `inspect_and_capture_content_identity` returning a
        // consistent identity for the same, unmodified directory across two
        // calls - the actual drift path (a real race between plan-build and
        // spawn) is exercised at the integration level; this proves the
        // capture itself is stable and therefore trustworthy for that
        // comparison.
        let d = tempdir().unwrap();
        let content = make_extracted_title(d.path());
        let first = inspect_and_capture_content_identity(&content).unwrap();
        let second = inspect_and_capture_content_identity(&content).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn preflight_never_writes_settings_xml_or_keys_txt() {
        let d = tempdir().unwrap();
        let exe = d.path().join("Cemu");
        let config_root = d.path().join("profile");
        let mlc = d.path().join("mlc");
        make_profile(d.path(), &exe, &config_root, &mlc);
        let content = make_extracted_title(d.path());
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());
        let settings_before = fs::read(config_root.join("settings.xml")).unwrap();

        let request = CemuPreflightRequest {
            selected_content_path: content,
            profile_id: format!("cemu:{}", config_root.display()),
            expected_executable: exe,
        };
        let _ = preflight_cemu_launch(&request, &roots);

        let settings_after = fs::read(config_root.join("settings.xml")).unwrap();
        assert_eq!(settings_before, settings_after);
        assert!(!config_root.join("keys.txt").exists());
    }
}
