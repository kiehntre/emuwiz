//! Fresh VICE C64 preflight. No earlier discovery or file identity is trusted
//! at spawn time; this returns direct argv only and never touches VICE config.
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::vice_command::{
    VICE_ATTACH_CRT, VICE_AUTOSTART, VICE_DISABLE_SAVE_RESOURCES, VICE_MONITOR_COMMANDS,
    VICE_SUPPORTED_PLATFORM_ID, ViceCheatLaunch, ViceContentKind, vice_content_kind,
};
use crate::patch_manager::{
    ViceCheatProjection, ViceCheatReadiness, ViceMemoryTarget, ViceProfileDiscoveryRoots,
    discover_vice_profiles, resolve_vice_native_launch_binding,
};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViceLaunchPreflightErrorKind {
    ContentPathNotAbsolute,
    ContentNotFound,
    ContentIsSymlink,
    ContentNotRegularFile,
    ContentFormatUnsupported,
    ContentChangedBeforeSpawn,
    IdentityUnresolved,
    IdentityMismatch,
    ProfileNotFound,
    ProfileIneligible,
    BindingUnavailable,
    BindingDrift,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceLaunchPreflightError {
    pub kind: ViceLaunchPreflightErrorKind,
    pub detail: String,
}
fn fail(kind: ViceLaunchPreflightErrorKind, detail: impl Into<String>) -> ViceLaunchPreflightError {
    ViceLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}
fn file(path: &Path) -> Result<CapturedFileIdentity, ViceLaunchPreflightError> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| fail(ViceLaunchPreflightErrorKind::ContentNotFound, e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentIsSymlink,
            "path is a symlink",
        ));
    }
    if !meta.is_file() {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentNotRegularFile,
            "path is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&meta))
}
pub fn preflight_vice_launch(
    request: &ViceLaunchRequest,
    roots: &ViceProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, ViceLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content must be absolute",
        ));
    }
    let Some(kind) = vice_content_kind(&request.selected_content_path) else {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentFormatUnsupported,
            "not a direct strong C64 VICE content form",
        ));
    };
    if crate::archive_kind(&request.selected_content_path).is_some_and(|k| k.is_mount_input()) {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentFormatUnsupported,
            "outer archive paths are never direct VICE content",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed since authorization",
        ));
    }
    let CanonicalIdentityStatus::Resolved(resolved) = identity else {
        return Err(fail(
            ViceLaunchPreflightErrorKind::IdentityUnresolved,
            "identity is unresolved or conflicting",
        ));
    };
    if resolved.platform_id != VICE_SUPPORTED_PLATFORM_ID
        || resolved.platform_id != request.expected_platform_id
        || resolved.game_key != request.expected_game_key
    {
        return Err(fail(
            ViceLaunchPreflightErrorKind::IdentityMismatch,
            "identity no longer matches authorized C64 content",
        ));
    }
    let profile = discover_vice_profiles(roots)
        .profiles
        .into_iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            fail(
                ViceLaunchPreflightErrorKind::ProfileNotFound,
                "authorized VICE profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "VICE profile is ineligible".into()),
        ));
    }
    let binding = resolve_vice_native_launch_binding(&profile)
        .map_err(|e| fail(ViceLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    if binding.executable != request.expected_executable {
        return Err(fail(
            ViceLaunchPreflightErrorKind::BindingDrift,
            "VICE executable binding changed since authorization",
        ));
    }
    let executable_identity = file(&binding.executable)
        .map_err(|e| fail(ViceLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&binding.executable).is_ok_and(|m| m.permissions().mode() & 0o111 == 0) {
            return Err(fail(
                ViceLaunchPreflightErrorKind::BindingUnavailable,
                "VICE executable no longer has an execute bit",
            ));
        }
    }
    if executable_identity != request.executable_identity {
        return Err(fail(
            ViceLaunchPreflightErrorKind::BindingDrift,
            "VICE executable changed since authorization",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed immediately before spawn",
        ));
    }
    let arguments = match kind {
        ViceContentKind::Autostart => vec![
            VICE_DISABLE_SAVE_RESOURCES.into(),
            VICE_AUTOSTART.into(),
            request.selected_content_path.clone().into_os_string(),
        ],
        ViceContentKind::Cartridge => vec![
            VICE_DISABLE_SAVE_RESOURCES.into(),
            "+cart".into(),
            VICE_ATTACH_CRT.into(),
            request.selected_content_path.clone().into_os_string(),
        ],
    };
    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments,
        working_directory: None,
    })
}
pub fn spawn_vice(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}

// ---------------------------------------------------------------------
// Cheats: a launch-owned monitor command file passed with `-moncommands`
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceCheatLaunchError {
    pub detail: String,
}
fn cheat_error(detail: impl Into<String>) -> ViceCheatLaunchError {
    ViceCheatLaunchError {
        detail: detail.into(),
    }
}

/// Largest monitor command file EmuWiz will write or accept.
const MAX_MONITOR_SCRIPT_BYTES: usize = 64 * 1024;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// An EmuWiz-owned, private temporary directory holding the one monitor
/// command file for one launch. It is removed when dropped, so keep it alive
/// for as long as VICE may need the file (hold it in the same
/// [`ViceCheatSession`] as the process). No VICE resource file, media image or
/// global configuration is ever touched.
pub struct ViceMonitorScript {
    dir: Option<tempfile::TempDir>,
    path: PathBuf,
}

impl ViceMonitorScript {
    /// Writes the command file for a launch-reviewed projection.
    ///
    /// Refused: anything not `ReadyForLaunchReview` (weak identity, bank-
    /// ambiguous or unsupported writes), no commands, and any write outside
    /// normal RAM or colour RAM - I/O and ROM/cartridge-banked targets are
    /// never applied automatically. The file is regenerated from the typed
    /// commands, not copied from the projection's text.
    pub fn create(
        scratch_root: &Path,
        projection: &ViceCheatProjection,
        game_key: &str,
    ) -> Result<(Self, ViceCheatLaunch), ViceCheatLaunchError> {
        if game_key.trim().is_empty() {
            return Err(cheat_error("cheats need a verified game key"));
        }
        if projection.readiness != ViceCheatReadiness::ReadyForLaunchReview
            || projection.commands.is_empty()
        {
            return Err(cheat_error(
                "these cheats are preview-only and cannot be used for a launch",
            ));
        }
        let mut text = String::from("radix H\n");
        for command in &projection.commands {
            if !matches!(
                command.memory,
                ViceMemoryTarget::Ram | ViceMemoryTarget::ColourRam
            ) {
                return Err(cheat_error(
                    "only normal RAM and colour RAM writes can be applied at launch",
                ));
            }
            text.push_str(&format!(
                "> {:04X} {:02X}\n",
                command.address, command.value
            ));
        }
        text.push_str("x\n");
        if text.len() > MAX_MONITOR_SCRIPT_BYTES {
            return Err(cheat_error("too many cheat commands for one launch"));
        }
        if !scratch_root.is_absolute() {
            return Err(cheat_error("the launch scratch directory must be absolute"));
        }
        let dir = tempfile::Builder::new()
            .prefix("emuwiz-vice-cheat-")
            .tempdir_in(scratch_root)
            .map_err(|e| {
                cheat_error(format!(
                    "could not create the launch scratch directory: {e}"
                ))
            })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).map_err(|e| {
                cheat_error(format!("could not make the scratch directory private: {e}"))
            })?;
        }
        let path = dir.path().join("commands.txt");
        {
            use std::io::Write;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&path)
                .map_err(|e| cheat_error(format!("could not write the cheat command file: {e}")))?;
            file.write_all(text.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|e| cheat_error(format!("could not write the cheat command file: {e}")))?;
        }
        let launch = ViceCheatLaunch {
            game_key: game_key.to_string(),
            script_path: path.clone(),
            script_sha256: sha256_hex(text.as_bytes()),
        };
        Ok((
            Self {
                dir: Some(dir),
                path,
            },
            launch,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Keeps the directory when the owner is dropped while VICE still runs.
    fn persist_if_running(&mut self, running: bool) {
        if running {
            if let Some(dir) = self.dir.take() {
                let _ = dir.keep();
            }
        }
    }
}

/// Like [`preflight_vice_launch`], and when cheats were chosen also proves the
/// command file is still the one written: absolute, a regular non-symlink
/// file, unchanged since it was hashed, and for this very game. Then adds
/// `-moncommands <file>` after `+saveres`. `None` returns exactly the
/// baseline command.
pub fn preflight_vice_launch_with_cheats(
    request: &ViceLaunchRequest,
    roots: &ViceProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
    cheats: Option<&ViceCheatLaunch>,
) -> Result<PreparedProcessCommand, ViceLaunchPreflightError> {
    let mut command = preflight_vice_launch(request, roots, identity)?;
    let Some(cheats) = cheats else {
        return Ok(command);
    };
    let bad = |detail: &str| {
        fail(
            ViceLaunchPreflightErrorKind::IdentityMismatch,
            detail.to_string(),
        )
    };
    if cheats.game_key != request.expected_game_key {
        return Err(bad("the chosen cheats were made for a different game"));
    }
    if !cheats.script_path.is_absolute() {
        return Err(bad("the cheat command file must be an absolute path"));
    }
    let meta = fs::symlink_metadata(&cheats.script_path)
        .map_err(|_| bad("the cheat command file is no longer available"))?;
    if meta.file_type().is_symlink()
        || !meta.is_file()
        || meta.len() as usize > MAX_MONITOR_SCRIPT_BYTES
    {
        return Err(bad("the cheat command file is not a safe regular file"));
    }
    let bytes = fs::read(&cheats.script_path)
        .map_err(|_| bad("the cheat command file could not be read"))?;
    if !sha256_hex(&bytes).eq_ignore_ascii_case(cheats.script_sha256.trim()) {
        return Err(bad("the cheat command file changed since it was written"));
    }
    command.arguments.insert(1, VICE_MONITOR_COMMANDS.into());
    command
        .arguments
        .insert(2, cheats.script_path.clone().into_os_string());
    Ok(command)
}

/// A running VICE with the launch-owned cheat command file it was started with.
pub struct ViceCheatSession {
    process: WatchedProcess,
    script: ViceMonitorScript,
}

impl ViceCheatSession {
    pub fn pid(&self) -> u32 {
        self.process.pid
    }
    pub fn poll(&mut self) -> Option<&process_spawn::ProcessExitReport> {
        self.process.poll()
    }
    pub fn is_running(&self) -> bool {
        self.process.is_running()
    }
}

impl Drop for ViceCheatSession {
    fn drop(&mut self) {
        // The command file is removed with the session once VICE has exited;
        // a session dropped while VICE still runs leaves it for the OS temp
        // cleaner rather than pulling it out from under the emulator.
        let running = self.process.poll().is_none();
        self.script.persist_if_running(running);
    }
}

pub fn spawn_vice_with_cheats(
    command: &PreparedProcessCommand,
    script: ViceMonitorScript,
) -> std::io::Result<ViceCheatSession> {
    Ok(ViceCheatSession {
        process: process_spawn::spawn_watched_process(command)?,
        script,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::ResolvedIdentity;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn mark_exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut m = fs::metadata(p).unwrap().permissions();
        m.set_mode(0o755);
        fs::set_permissions(p, m).unwrap();
    }

    fn identity(platform: &str, key: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: key.into(),
        })
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        rom: PathBuf,
        exe: PathBuf,
        roots: ViceProfileDiscoveryRoots,
        profile_id: String,
    }

    fn fixture() -> Fixture {
        let dir = tempdir().unwrap();
        let rom = dir.path().join("game.t64");
        fs::write(&rom, b"rom-bytes").unwrap();
        let exe = dir.path().join("x64sc");
        fs::write(&exe, b"exe-bytes").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = ViceProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let profile_id = discover_vice_profiles(&roots).profiles[0]
            .profile_id
            .clone();
        Fixture {
            _dir: dir,
            rom,
            exe,
            roots,
            profile_id,
        }
    }

    fn request(fixture: &Fixture) -> ViceLaunchRequest {
        ViceLaunchRequest {
            selected_content_path: fixture.rom.clone(),
            expected_platform_id: "Commodore 64".into(),
            expected_game_key: "c64sha".into(),
            profile_id: fixture.profile_id.clone(),
            expected_executable: fixture.exe.clone(),
            content_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(&fixture.rom).unwrap(),
            ),
            executable_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(&fixture.exe).unwrap(),
            ),
        }
    }

    #[test]
    fn ready_request_produces_the_exact_argv() {
        let fx = fixture();
        let command = preflight_vice_launch(
            &request(&fx),
            &fx.roots,
            &identity("Commodore 64", "c64sha"),
        )
        .unwrap();
        assert_eq!(command.executable, fx.exe);
        assert_eq!(
            command.arguments,
            vec![
                std::ffi::OsString::from("+saveres"),
                std::ffi::OsString::from("-autostart"),
                fx.rom.clone().into_os_string(),
            ]
        );
        assert!(command.working_directory.is_none());
    }

    #[test]
    fn crt_request_produces_the_exact_cartridge_argv() {
        let fx = fixture();
        let crt = fx.rom.with_extension("crt");
        fs::write(&crt, b"cart-bytes").unwrap();
        let mut req = request(&fx);
        req.selected_content_path = crt.clone();
        req.content_identity = CapturedFileIdentity::capture(&fs::symlink_metadata(&crt).unwrap());
        let command =
            preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha")).unwrap();
        assert_eq!(
            command.arguments,
            vec![
                std::ffi::OsString::from("+saveres"),
                std::ffi::OsString::from("+cart"),
                std::ffi::OsString::from("-cartcrt"),
                crt.into_os_string(),
            ]
        );
    }

    fn ram_projection() -> ViceCheatProjection {
        use crate::patch_manager::CheatOperation;
        use crate::patch_manager::{ViceCheatIdentity, project_vice_c64_pokes};
        project_vice_c64_pokes(
            &[CheatOperation::Write8 {
                address: 0xC000,
                value: 0xFF,
            }],
            &ViceCheatIdentity::VerifiedGame("c64sha".into()),
        )
    }

    #[test]
    fn script_is_regenerated_private_and_removed_with_its_owner() {
        let fx = fixture();
        let scratch = tempdir().unwrap();
        let (script, launch) =
            ViceMonitorScript::create(scratch.path(), &ram_projection(), "c64sha").unwrap();
        assert_eq!(
            fs::read_to_string(script.path()).unwrap(),
            "radix H\n> C000 FF\nx\n"
        );
        assert_eq!(launch.script_path, script.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(script.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let dir = script.path().parent().unwrap();
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let dir = script.path().parent().unwrap().to_path_buf();
        // The game media is never touched by building cheats.
        assert_eq!(fs::read(&fx.rom).unwrap(), b"rom-bytes");
        drop(script);
        assert!(!dir.exists(), "temporary command file must be cleaned up");
    }

    #[test]
    fn only_launch_ready_ram_and_colour_ram_cheats_are_accepted() {
        use crate::patch_manager::CheatOperation;
        use crate::patch_manager::{ViceCheatIdentity, project_vice_c64_pokes};
        let scratch = tempdir().unwrap();
        let strong = ViceCheatIdentity::VerifiedGame("c64sha".into());
        // Weak identity is preview-only.
        let weak = project_vice_c64_pokes(
            &[CheatOperation::Write8 {
                address: 0xC000,
                value: 1,
            }],
            &ViceCheatIdentity::TitleOnly("title".into()),
        );
        assert!(ViceMonitorScript::create(scratch.path(), &weak, "c64sha").is_err());
        // I/O and ROM-mapped targets are never applied automatically.
        for address in [0xD020_u64, 0xE000, 0x8000] {
            let projection =
                project_vice_c64_pokes(&[CheatOperation::Write8 { address, value: 1 }], &strong);
            assert!(
                ViceMonitorScript::create(scratch.path(), &projection, "c64sha").is_err(),
                "{address:#x} must be refused"
            );
        }
        // Colour RAM is allowed; an empty selection and a blank game key are not.
        let colour = project_vice_c64_pokes(
            &[CheatOperation::Write8 {
                address: 0xD800,
                value: 1,
            }],
            &strong,
        );
        assert!(ViceMonitorScript::create(scratch.path(), &colour, "c64sha").is_ok());
        let none = project_vice_c64_pokes(&[], &strong);
        assert!(ViceMonitorScript::create(scratch.path(), &none, "c64sha").is_err());
        assert!(ViceMonitorScript::create(scratch.path(), &ram_projection(), " ").is_err());
    }

    #[test]
    fn preflight_with_cheats_adds_moncommands_and_still_runs_the_canonical_checks() {
        let fx = fixture();
        let scratch = tempdir().unwrap();
        let (script, launch) =
            ViceMonitorScript::create(scratch.path(), &ram_projection(), "c64sha").unwrap();
        let command = preflight_vice_launch_with_cheats(
            &request(&fx),
            &fx.roots,
            &identity("Commodore 64", "c64sha"),
            Some(&launch),
        )
        .unwrap();
        assert_eq!(
            command.arguments,
            vec![
                std::ffi::OsString::from("+saveres"),
                std::ffi::OsString::from("-moncommands"),
                script.path().as_os_str().to_owned(),
                std::ffi::OsString::from("-autostart"),
                fx.rom.clone().into_os_string(),
            ]
        );
        // Without cheats the command is exactly the baseline one.
        let plain = preflight_vice_launch(
            &request(&fx),
            &fx.roots,
            &identity("Commodore 64", "c64sha"),
        )
        .unwrap();
        assert_eq!(
            preflight_vice_launch_with_cheats(
                &request(&fx),
                &fx.roots,
                &identity("Commodore 64", "c64sha"),
                None
            )
            .unwrap(),
            plain
        );
        // The canonical preflight still refuses a changed game even with cheats.
        let authorised = request(&fx);
        fs::write(&fx.rom, b"changed").unwrap();
        assert_eq!(
            preflight_vice_launch_with_cheats(
                &authorised,
                &fx.roots,
                &identity("Commodore 64", "c64sha"),
                Some(&launch),
            )
            .unwrap_err()
            .kind,
            ViceLaunchPreflightErrorKind::ContentChangedBeforeSpawn
        );
    }

    #[test]
    fn stale_wrong_game_or_tampered_cheats_are_refused() {
        let fx = fixture();
        let scratch = tempdir().unwrap();
        let (script, launch) =
            ViceMonitorScript::create(scratch.path(), &ram_projection(), "c64sha").unwrap();
        let go = |launch: &ViceCheatLaunch| {
            preflight_vice_launch_with_cheats(
                &request(&fx),
                &fx.roots,
                &identity("Commodore 64", "c64sha"),
                Some(launch),
            )
        };
        let mut other = launch.clone();
        other.game_key = "other".into();
        assert!(go(&other).is_err());
        fs::write(script.path(), "radix H\n> C000 00\nx\n").unwrap();
        assert!(go(&launch).unwrap_err().detail.contains("changed"));
        fs::remove_file(script.path()).unwrap();
        assert!(go(&launch).unwrap_err().detail.contains("no longer"));
        #[cfg(unix)]
        {
            let target = scratch.path().join("elsewhere.txt");
            fs::write(&target, "radix H\n> C000 FF\nx\n").unwrap();
            std::os::unix::fs::symlink(&target, script.path()).unwrap();
            assert!(
                go(&launch)
                    .unwrap_err()
                    .detail
                    .contains("not a safe regular")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn session_keeps_the_file_while_running_and_cleans_up_after_exit() {
        let scratch = tempdir().unwrap();
        let (script, _launch) =
            ViceMonitorScript::create(scratch.path(), &ram_projection(), "c64sha").unwrap();
        let dir = script.path().parent().unwrap().to_path_buf();
        let command = PreparedProcessCommand {
            executable: "/bin/sleep".into(),
            arguments: vec!["0.3".into()],
            working_directory: None,
        };
        let mut session = spawn_vice_with_cheats(&command, script).unwrap();
        assert!(session.is_running());
        assert!(dir.exists(), "file must stay while the process runs");
        for _ in 0..100 {
            if session.poll().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!session.is_running());
        drop(session);
        assert!(!dir.exists(), "file is removed once the process has exited");
    }
    #[test]
    fn non_c64_platform_is_refused() {
        let fx = fixture();
        let mut req = request(&fx);
        req.expected_platform_id = "Commodore 128".into();
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 128", "c64sha"))
            .unwrap_err();
        assert_eq!(err.kind, ViceLaunchPreflightErrorKind::IdentityMismatch);
    }

    #[test]
    fn unsupported_extension_is_refused() {
        let fx = fixture();
        let bin = fx.rom.with_extension("d64");
        fs::write(&bin, b"disk").unwrap();
        let mut req = request(&fx);
        req.selected_content_path = bin.clone();
        req.content_identity = CapturedFileIdentity::capture(&fs::symlink_metadata(&bin).unwrap());
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha"))
            .unwrap_err();
        assert_eq!(
            err.kind,
            ViceLaunchPreflightErrorKind::ContentFormatUnsupported
        );
    }

    #[test]
    fn missing_executable_is_refused() {
        let fx = fixture();
        let req = request(&fx);
        fs::remove_file(&fx.exe).unwrap();
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha"))
            .unwrap_err();
        // VICE's own discovery filters non-existent executables out of the
        // profile list entirely (see `discover_vice_profiles`'s
        // `.filter(|(p, _)| p.exists())`), so a removed executable makes the
        // authorized profile un-rediscoverable rather than rediscoverable-
        // but-ineligible.
        assert_eq!(err.kind, ViceLaunchPreflightErrorKind::ProfileNotFound);
    }

    #[test]
    fn stale_executable_swapped_after_authorization_is_refused_at_final_check() {
        let fx = fixture();
        let mut req = request(&fx);
        req.executable_identity.size += 1;
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha"))
            .unwrap_err();
        assert_eq!(err.kind, ViceLaunchPreflightErrorKind::BindingDrift);
    }

    #[test]
    fn content_changed_before_spawn_is_refused() {
        let fx = fixture();
        let mut req = request(&fx);
        req.content_identity.size += 1;
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha"))
            .unwrap_err();
        assert_eq!(
            err.kind,
            ViceLaunchPreflightErrorKind::ContentChangedBeforeSpawn
        );
    }

    #[test]
    fn no_installation_candidate_is_never_substituted_with_retroarch() {
        let fx = fixture();
        let empty_roots = ViceProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let err = preflight_vice_launch(
            &request(&fx),
            &empty_roots,
            &identity("Commodore 64", "c64sha"),
        )
        .unwrap_err();
        assert_eq!(err.kind, ViceLaunchPreflightErrorKind::ProfileNotFound);
        // The error is a structured VICE-specific refusal - nothing here ever
        // falls back to building or returning a RetroArch command instead.
    }

    #[test]
    fn x64_and_x64sc_are_never_silently_substituted() {
        let dir = tempdir().unwrap();
        let x64sc = dir.path().join("x64sc");
        let x64 = dir.path().join("x64");
        fs::write(&x64sc, b"exe-bytes-sc").unwrap();
        fs::write(&x64, b"exe-bytes-fast").unwrap();
        #[cfg(unix)]
        {
            mark_exec(&x64sc);
            mark_exec(&x64);
        }
        let roots = ViceProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_vice_profiles(&roots);
        let x64sc_profile = discovery
            .profiles
            .iter()
            .find(|p| p.profile_id.ends_with("x64sc"))
            .unwrap();
        let x64_profile = discovery
            .profiles
            .iter()
            .find(|p| p.profile_id.ends_with("x64") && !p.profile_id.ends_with("x64sc"))
            .unwrap();
        assert_ne!(x64sc_profile.profile_id, x64_profile.profile_id);
        let rom = dir.path().join("game.t64");
        fs::write(&rom, b"rom-bytes").unwrap();
        let req = ViceLaunchRequest {
            selected_content_path: rom.clone(),
            expected_platform_id: "Commodore 64".into(),
            expected_game_key: "c64sha".into(),
            profile_id: x64sc_profile.profile_id.clone(),
            expected_executable: x64sc.clone(),
            content_identity: CapturedFileIdentity::capture(&fs::symlink_metadata(&rom).unwrap()),
            executable_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(&x64sc).unwrap(),
            ),
        };
        let command =
            preflight_vice_launch(&req, &roots, &identity("Commodore 64", "c64sha")).unwrap();
        // The exact x64sc binary is used - never silently swapped for x64.
        assert_eq!(command.executable, x64sc);
    }
}
