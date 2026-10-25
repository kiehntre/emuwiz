//! First supported slice of real native SameBoy launch execution: safely
//! revalidating and spawning exactly one native SameBoy process for one
//! already-selected direct `.gb`/`.gbc` file.
//!
//! # Scope (first slice)
//!
//! - Native SameBoy profiles only - any profile
//!   [`crate::patch_manager::resolve_sameboy_native_launch_binding`] itself
//!   refuses is never attempted.
//! - `Game Boy`/`Game Boy Color` only.
//! - A direct `.gb`/`.gbc` file whose cartridge header (freshly re-parsed
//!   every call, never cached) proves a genuine Game Boy/Game Boy Color
//!   cartridge and does not contradict the requested platform.
//! - Exactly one requested, already-discovered SameBoy profile, matched by
//!   profile id - never a silent substitution of a different profile,
//!   executable, or emulator. In particular, SameBoy unavailable never
//!   falls back to mGBA, RetroArch, or any other Game Boy emulator - see
//!   the module doc comment on [`crate::launch::sameboy_command`].
//!
//! # What this is not
//!
//! - It never builds a GUI Launch button and is not wired to one yet.
//! - It never touches mGBA's own discovery, command, readiness, or
//!   preflight - the two adapters are completely independent.
//! - It never interprets a shell: every process is spawned via
//!   [`std::process::Command::new`] plus [`std::process::Command::args`]
//!   (see [`crate::launch::process_spawn::spawn_watched_process`]), never
//!   `sh -c` and never one concatenated command string.
//! - It never re-derives argv itself - the exact executable/argument list
//!   always comes from
//!   [`crate::launch::sameboy_command::build_sameboy_command_plan`], rebuilt
//!   fresh from freshly re-gathered evidence every single call.
//! - It never mutates `prefs.bin`, a boot ROM, or any ROM/save file.
//! - It never adds an automatic timeout, kill, or relaunch - SameBoy is a
//!   long-running, user-facing process the caller owns.

use std::fs;
use std::path::{Path, PathBuf};

use crate::gb_header_evidence::parse_gb_header;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, ProcessExitReport, WatchedProcess,
};
use crate::launch::sameboy_command::{
    SameBoyCommand, SameBoyLaunchRequest, SameBoyReadiness, build_sameboy_command_plan,
    classify_sameboy_readiness, form_for_path,
};
use crate::patch_manager::{
    SameBoyProfileDiscoveryRoots, discover_sameboy_profiles, resolve_sameboy_native_launch_binding,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// The narrow, immutable set of facts identifying exactly which
/// user-authorized native SameBoy launch is being requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyPreflightRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameBoyLaunchPreflightErrorKind {
    ContentPathNotAbsolute,
    ContentNotFound,
    ContentIsSymlink,
    ContentNotRegularFile,
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
pub struct SameBoyLaunchPreflightError {
    pub kind: SameBoyLaunchPreflightErrorKind,
    pub detail: String,
}

fn preflight_error(
    kind: SameBoyLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> SameBoyLaunchPreflightError {
    SameBoyLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

#[derive(Debug)]
pub enum SameBoyLaunchSpawnError {
    Spawn(std::io::Error),
}

#[derive(Debug)]
pub enum SameBoyLaunchExecutionError {
    Preflight(SameBoyLaunchPreflightError),
    Spawn(SameBoyLaunchSpawnError),
}

impl From<SameBoyLaunchPreflightError> for SameBoyLaunchExecutionError {
    fn from(error: SameBoyLaunchPreflightError) -> Self {
        Self::Preflight(error)
    }
}
impl From<SameBoyLaunchSpawnError> for SameBoyLaunchExecutionError {
    fn from(error: SameBoyLaunchSpawnError) -> Self {
        Self::Spawn(error)
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt [`SameBoyCommand`] safe to spawn - or refuses with a
/// [`SameBoyLaunchPreflightError`] naming exactly why.
///
/// # Sequence
///
/// 1. `request.selected_content_path` must be absolute.
/// 2. The content must exist, not be a symlink, be a regular file, and have
///    a `.gb`/`.gbc` extension.
/// 3. A [`CapturedFileIdentity`] is captured from the content's current
///    metadata.
/// 4. The first [`GB_HEADER_BYTES`] of the content are read fresh and
///    parsed via [`parse_gb_header`] - never a cached header.
/// 5. SameBoy profiles are freshly rediscovered via
///    [`discover_sameboy_profiles`] - never a caller's cached discovery.
/// 6. The profile whose id exactly equals `request.profile_id` is found -
///    never substituted with a different one.
/// 7. [`resolve_sameboy_native_launch_binding`] is called fresh against
///    that profile; its executable must exactly equal
///    `request.expected_executable`.
/// 8. [`build_sameboy_command_plan`] is rebuilt from all of the above; it
///    must report no blockers and a command.
/// 9. Immediately before returning: the executable is re-checked to still
///    exist, not be a symlink, be a regular file, and be marked executable;
///    the content is re-inspected once more and its [`CapturedFileIdentity`]
///    must still equal the one captured in step 3.
pub fn preflight_sameboy_launch(
    request: &SameBoyPreflightRequest,
    roots: &SameBoyProfileDiscoveryRoots,
) -> Result<SameBoyCommand, SameBoyLaunchPreflightError> {
    // --- 1-3: content path facts + identity capture ---
    let content_path = &request.selected_content_path;
    if !content_path.is_absolute() {
        return Err(preflight_error(
            SameBoyLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "requested content path is not absolute",
        ));
    }
    let content_form = form_for_path(content_path).ok_or_else(|| {
        preflight_error(
            SameBoyLaunchPreflightErrorKind::ContentFormatUnsupported,
            "only a direct .gb or .gbc file is supported in this build",
        )
    })?;
    let content_identity = inspect_and_capture_content_identity(content_path)?;

    // --- 4: fresh header read + parse ---
    let header_bytes = fs::read(content_path).map_err(|io_error| {
        preflight_error(
            SameBoyLaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", content_path.display()),
        )
    })?;
    let rom_header = parse_gb_header(&header_bytes);

    // --- 5-6: fresh profile discovery, find the exact requested profile ---
    let discovery = discover_sameboy_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.profile_id)
        .ok_or_else(|| {
            preflight_error(
                SameBoyLaunchPreflightErrorKind::ProfileNotFound,
                "no freshly discovered SameBoy profile matches the requested profile id",
            )
        })?;

    // --- 7: fresh launch binding, refuse silent substitution ---
    let binding_result = resolve_sameboy_native_launch_binding(profile);
    let binding = binding_result.as_ref().map_err(|error| {
        preflight_error(
            SameBoyLaunchPreflightErrorKind::BindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(preflight_error(
            SameBoyLaunchPreflightErrorKind::BindingDrift,
            "the freshly resolved launch binding no longer matches the user-authorized executable",
        ));
    }

    // --- 8: rebuild the command plan ---
    let sameboy_request = SameBoyLaunchRequest {
        executable: binding.executable.clone(),
        profile_id: profile.profile_id.clone(),
        platform_id: request.expected_platform_id.clone(),
        selected_content: content_path.clone(),
        content_form,
        rom_header,
        boot_rom: profile.boot_rom.clone(),
        config: profile.config.clone(),
        // Recomputed fresh below; this value is never trusted.
        readiness: SameBoyReadiness::Blocked,
    };
    let plan = build_sameboy_command_plan(&sameboy_request);
    if !plan.blockers.is_empty() {
        return Err(preflight_error(
            SameBoyLaunchPreflightErrorKind::CommandBlocked,
            format!("command plan reported {} blocker(s)", plan.blockers.len()),
        ));
    }
    let _ = classify_sameboy_readiness(&plan);
    let command = plan.command.ok_or_else(|| {
        preflight_error(
            SameBoyLaunchPreflightErrorKind::CommandMissing,
            "command plan reported no blockers but also no command",
        )
    })?;

    // --- 9: recheck immediately before spawn ---
    recheck_executable(&command.executable)?;
    let current_identity = inspect_and_capture_content_identity(&command.selection.content_path)?;
    if current_identity != content_identity {
        return Err(preflight_error(
            SameBoyLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed during preflight",
        ));
    }

    Ok(command)
}

fn inspect_and_capture_content_identity(
    path: &Path,
) -> Result<CapturedFileIdentity, SameBoyLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            SameBoyLaunchPreflightErrorKind::ContentNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(preflight_error(
            SameBoyLaunchPreflightErrorKind::ContentIsSymlink,
            "content path is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(preflight_error(
            SameBoyLaunchPreflightErrorKind::ContentNotRegularFile,
            "content path is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn recheck_executable(path: &Path) -> Result<(), SameBoyLaunchPreflightError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        preflight_error(
            SameBoyLaunchPreflightErrorKind::ExecutableMissing,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(preflight_error(
            SameBoyLaunchPreflightErrorKind::ExecutableUnsafe,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(preflight_error(
                SameBoyLaunchPreflightErrorKind::ExecutableNotExecutable,
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
pub struct SameBoyLaunchCommandFacts {
    pub executable: PathBuf,
    pub arguments: Vec<std::ffi::OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub platform_id: String,
    pub content_path: PathBuf,
    pub rom_title: Option<String>,
}

fn command_facts(command: &SameBoyCommand) -> SameBoyLaunchCommandFacts {
    SameBoyLaunchCommandFacts {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
        profile_id: command.selection.profile_id.clone(),
        platform_id: command.selection.platform_id.clone(),
        content_path: command.selection.content_path.clone(),
        rom_title: command.selection.rom_title.clone(),
    }
}

pub use crate::launch::process_spawn::ProcessExitReport as SameBoyLaunchExitReport;

/// A spawned, still-owned SameBoy process. Never automatically killed, timed
/// out, or relaunched by this module.
pub struct LaunchedSameBoyProcess {
    pub pid: u32,
    pub command_facts: SameBoyLaunchCommandFacts,
    watched: WatchedProcess,
}

impl LaunchedSameBoyProcess {
    pub fn poll(&mut self) -> Option<&ProcessExitReport> {
        self.watched.poll()
    }

    pub fn is_running(&self) -> bool {
        self.watched.is_running()
    }
}

/// Spawns exactly the process `command` describes - never a shell.
/// `command` must already have passed [`preflight_sameboy_launch`].
pub fn spawn_sameboy(
    command: SameBoyCommand,
) -> Result<LaunchedSameBoyProcess, SameBoyLaunchSpawnError> {
    let facts = command_facts(&command);
    let prepared = PreparedProcessCommand {
        executable: command.executable,
        arguments: command.arguments,
        working_directory: command.working_directory,
    };
    let watched =
        process_spawn::spawn_watched_process(&prepared).map_err(SameBoyLaunchSpawnError::Spawn)?;
    Ok(LaunchedSameBoyProcess {
        pid: watched.pid,
        command_facts: facts,
        watched,
    })
}

/// Composes [`preflight_sameboy_launch`] and [`spawn_sameboy`] - the single
/// call a future GUI Launch button would make.
pub fn preflight_and_launch_sameboy(
    request: &SameBoyPreflightRequest,
    roots: &SameBoyProfileDiscoveryRoots,
) -> Result<LaunchedSameBoyProcess, SameBoyLaunchExecutionError> {
    let command = preflight_sameboy_launch(request, roots)?;
    Ok(spawn_sameboy(command)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gb_header_evidence::GB_HEADER_BYTES;
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

    const NINTENDO_LOGO_OFFSET: usize = 0x104;
    #[rustfmt::skip]
    const NINTENDO_LOGO: [u8; 48] = [
        0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83,
        0x00, 0x0C, 0x00, 0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E,
        0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD, 0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63,
        0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
    ];
    const CGB_FLAG_OFFSET: usize = 0x143;
    const HEADER_CHECKSUM_OFFSET: usize = 0x14D;
    const CHECKSUM_RANGE_START: usize = 0x134;
    const CHECKSUM_RANGE_END_INCLUSIVE: usize = 0x14C;

    fn valid_gb_rom(cgb_flag: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; GB_HEADER_BYTES];
        bytes[NINTENDO_LOGO_OFFSET..NINTENDO_LOGO_OFFSET + NINTENDO_LOGO.len()]
            .copy_from_slice(&NINTENDO_LOGO);
        bytes[CGB_FLAG_OFFSET] = cgb_flag;
        let mut checksum: u8 = 0;
        for &byte in &bytes[CHECKSUM_RANGE_START..=CHECKSUM_RANGE_END_INCLUSIVE] {
            checksum = checksum.wrapping_sub(byte).wrapping_sub(1);
        }
        bytes[HEADER_CHECKSUM_OFFSET] = checksum;
        bytes
    }

    fn roots_for(dir: &Path, exe: PathBuf, config_root: PathBuf) -> SameBoyProfileDiscoveryRoots {
        SameBoyProfileDiscoveryRoots {
            home: dir.to_path_buf(),
            xdg_data_home: dir.join("no-xdg"),
            explicit_configuration_roots: vec![config_root],
            portable_configuration_roots: vec![],
            explicit_executables: vec![exe],
            known_version_outputs: BTreeMap::new(),
        }
    }

    #[test]
    fn happy_path_preflight_produces_expected_argv() {
        let d = tempdir().unwrap();
        let exe = d.path().join("sameboy");
        write_exe(&exe);
        let config_root = d.path().join("profile");
        fs::create_dir_all(&config_root).unwrap();
        let rom = d.path().join("Game With Spaces.gb");
        fs::write(&rom, valid_gb_rom(0x00)).unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = SameBoyPreflightRequest {
            selected_content_path: rom.clone(),
            expected_platform_id: "Game Boy".to_string(),
            profile_id: format!("sameboy:{}", config_root.display()),
            expected_executable: exe.clone(),
        };
        let command = preflight_sameboy_launch(&request, &roots).expect("preflight ok");
        assert_eq!(command.executable, exe);
        assert_eq!(
            command.arguments,
            vec![std::ffi::OsString::from(rom.as_os_str())]
        );
    }

    #[test]
    fn wrong_profile_id_is_never_substituted() {
        let d = tempdir().unwrap();
        let exe = d.path().join("sameboy");
        write_exe(&exe);
        let config_root = d.path().join("profile");
        fs::create_dir_all(&config_root).unwrap();
        let rom = d.path().join("game.gb");
        fs::write(&rom, valid_gb_rom(0x00)).unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = SameBoyPreflightRequest {
            selected_content_path: rom,
            expected_platform_id: "Game Boy".to_string(),
            profile_id: "sameboy:/does/not/exist".to_string(),
            expected_executable: exe,
        };
        let error = preflight_sameboy_launch(&request, &roots).unwrap_err();
        assert_eq!(error.kind, SameBoyLaunchPreflightErrorKind::ProfileNotFound);
    }

    #[test]
    fn executable_drift_is_refused_not_silently_substituted() {
        let d = tempdir().unwrap();
        let exe = d.path().join("sameboy");
        write_exe(&exe);
        let config_root = d.path().join("profile");
        fs::create_dir_all(&config_root).unwrap();
        let rom = d.path().join("game.gb");
        fs::write(&rom, valid_gb_rom(0x00)).unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = SameBoyPreflightRequest {
            selected_content_path: rom,
            expected_platform_id: "Game Boy".to_string(),
            profile_id: format!("sameboy:{}", config_root.display()),
            expected_executable: d.path().join("a-different-sameboy"),
        };
        let error = preflight_sameboy_launch(&request, &roots).unwrap_err();
        assert_eq!(error.kind, SameBoyLaunchPreflightErrorKind::BindingDrift);
    }

    #[test]
    fn cgb_only_rom_requested_as_game_boy_blocks() {
        let d = tempdir().unwrap();
        let exe = d.path().join("sameboy");
        write_exe(&exe);
        let config_root = d.path().join("profile");
        fs::create_dir_all(&config_root).unwrap();
        let rom = d.path().join("game.gb");
        fs::write(&rom, valid_gb_rom(0xC0)).unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = SameBoyPreflightRequest {
            selected_content_path: rom,
            expected_platform_id: "Game Boy".to_string(),
            profile_id: format!("sameboy:{}", config_root.display()),
            expected_executable: exe,
        };
        let error = preflight_sameboy_launch(&request, &roots).unwrap_err();
        assert_eq!(error.kind, SameBoyLaunchPreflightErrorKind::CommandBlocked);
    }

    #[test]
    fn malformed_truncated_rom_blocks() {
        let d = tempdir().unwrap();
        let exe = d.path().join("sameboy");
        write_exe(&exe);
        let config_root = d.path().join("profile");
        fs::create_dir_all(&config_root).unwrap();
        let rom = d.path().join("game.gb");
        fs::write(&rom, b"too short").unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = SameBoyPreflightRequest {
            selected_content_path: rom,
            expected_platform_id: "Game Boy".to_string(),
            profile_id: format!("sameboy:{}", config_root.display()),
            expected_executable: exe,
        };
        let error = preflight_sameboy_launch(&request, &roots).unwrap_err();
        assert_eq!(error.kind, SameBoyLaunchPreflightErrorKind::CommandBlocked);
    }

    #[test]
    fn non_gb_extension_is_refused_as_unsupported_shape() {
        let d = tempdir().unwrap();
        let exe = d.path().join("sameboy");
        write_exe(&exe);
        let config_root = d.path().join("profile");
        fs::create_dir_all(&config_root).unwrap();
        let rom = d.path().join("game.zip");
        fs::write(&rom, valid_gb_rom(0x00)).unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = SameBoyPreflightRequest {
            selected_content_path: rom,
            expected_platform_id: "Game Boy".to_string(),
            profile_id: format!("sameboy:{}", config_root.display()),
            expected_executable: exe,
        };
        let error = preflight_sameboy_launch(&request, &roots).unwrap_err();
        assert_eq!(
            error.kind,
            SameBoyLaunchPreflightErrorKind::ContentFormatUnsupported
        );
    }

    #[test]
    fn symlinked_content_is_refused() {
        let d = tempdir().unwrap();
        let exe = d.path().join("sameboy");
        write_exe(&exe);
        let config_root = d.path().join("profile");
        fs::create_dir_all(&config_root).unwrap();
        let real = d.path().join("real.gb");
        fs::write(&real, valid_gb_rom(0x00)).unwrap();
        let link = d.path().join("link.gb");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        #[cfg(unix)]
        {
            let request = SameBoyPreflightRequest {
                selected_content_path: link,
                expected_platform_id: "Game Boy".to_string(),
                profile_id: format!("sameboy:{}", config_root.display()),
                expected_executable: exe,
            };
            let error = preflight_sameboy_launch(&request, &roots).unwrap_err();
            assert_eq!(
                error.kind,
                SameBoyLaunchPreflightErrorKind::ContentIsSymlink
            );
        }
    }

    #[test]
    fn preflight_never_writes_prefs_or_the_rom() {
        let d = tempdir().unwrap();
        let exe = d.path().join("sameboy");
        write_exe(&exe);
        let config_root = d.path().join("profile");
        fs::create_dir_all(&config_root).unwrap();
        let rom = d.path().join("game.gb");
        let rom_bytes = valid_gb_rom(0x00);
        fs::write(&rom, &rom_bytes).unwrap();
        let roots = roots_for(d.path(), exe.clone(), config_root.clone());

        let request = SameBoyPreflightRequest {
            selected_content_path: rom.clone(),
            expected_platform_id: "Game Boy".to_string(),
            profile_id: format!("sameboy:{}", config_root.display()),
            expected_executable: exe,
        };
        let _ = preflight_sameboy_launch(&request, &roots);

        assert_eq!(fs::read(&rom).unwrap(), rom_bytes);
        assert!(!config_root.join("prefs.bin").exists());
    }
}
