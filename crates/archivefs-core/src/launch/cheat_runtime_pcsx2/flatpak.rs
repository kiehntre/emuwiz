//! Exact-deployment capability and sandbox-aware lifetime. The canonical
//! process watcher owns each child; this adapter never trusts launcher exit
//! alone as permission to remove files used by a sandbox.
use super::super::process_spawn::{WatchedProcess, spawn_watched_process_with_assets};
use super::*;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

fn scope(kind: Pcsx2InstallationType) -> Result<&'static str, String> {
    match kind {
        Pcsx2InstallationType::FlatpakUser => Ok("--user"),
        Pcsx2InstallationType::FlatpakSystem => Ok("--system"),
        _ => Err("Temporary PCSX2 cheats currently require the tested Flatpak installation. Portable, native and AppImage data-path isolation is not verified; ordinary launch remains available.".into()),
    }
}

fn output(binary: &Path, args: &[&str]) -> Result<String, String> {
    let mut capture = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(capture.try_clone().map_err(|e| e.to_string())?)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Flatpak capability/lifetime query timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return Err(format!("Flatpak query refused ({status})"));
    }
    use std::io::{Seek, SeekFrom};
    capture
        .seek(SeekFrom::Start(0))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    capture
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("Flatpak query exceeded its output bound".into());
    }
    String::from_utf8(bytes).map_err(|_| "Flatpak query returned invalid text".into())
}

pub(super) fn capability(
    binding: &Pcsx2NativeLaunchBinding,
    kind: Pcsx2InstallationType,
) -> Result<String, String> {
    let install_scope = scope(kind)?;
    if binding.installation
        != (LaunchInstallation::Flatpak {
            app_id: APP_ID.into(),
        })
    {
        return Err("PCSX2 installation does not match the tested Flatpak application".into());
    }
    let commit = output(
        &binding.executable,
        &["info", install_scope, "--show-commit", APP_ID],
    )?;
    if commit.trim() != TESTED_DEPLOYMENT {
        return Err(format!(
            "This PCSX2 deployment has not been verified for temporary cheats ({}). Ordinary launch remains available.",
            commit.trim()
        ));
    }
    Ok(commit.trim().into())
}

fn instance(workspace: &Path) -> Result<String, String> {
    let bytes = config::read_source(&workspace.join(INSTANCE_FILE), 64)?;
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| "Invalid Flatpak instance ID")?
        .trim();
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|c| c.is_ascii_digit()) {
        return Err("Flatpak has not reported a valid instance ID".into());
    }
    Ok(value.into())
}

fn instance_info(id: &str) -> PathBuf {
    // Use the real login runtime directory, not a workspace-provided path.
    PathBuf::from(format!("/run/user/{}/.flatpak/{id}/info", unsafe {
        libc::geteuid()
    }))
}

fn listed(binary: &Path, id: &str) -> Result<bool, String> {
    Ok(output(binary, &["ps", "--columns=instance"])?
        .split_whitespace()
        .any(|s| s == id))
}

fn stopped(binary: &Path, id: &str) -> bool {
    // Both independent lifetime observations must agree. An unreadable
    // query or retained instance metadata is never treated as stopped.
    matches!(listed(binary, id), Ok(false))
        && matches!(std::fs::symlink_metadata(instance_info(id)), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

fn stop_probe(binary: &Path, id: &str) -> bool {
    if stopped(binary, id) {
        return true;
    }
    // The ID came from this launch's inherited private descriptor. Never
    // kill by app ID or by a PID supplied by saved state.
    let _ = output(binary, &["kill", id]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if stopped(binary, id) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

pub(super) fn verify_deployment(ini: &config::Ini) -> Result<(), String> {
    if ini.get("Application", "name") != Some(APP_ID)
        || ini.get("Instance", "app-commit") != Some(TESTED_DEPLOYMENT)
    {
        return Err(
            "The running sandbox is not the approved PCSX2 application and deployment".into(),
        );
    }
    Ok(())
}

pub(super) fn verify_context(id: &str, expected: &[OsString]) -> Result<(), String> {
    let bytes = config::read_source(&instance_info(id), 64 * 1024)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "Invalid Flatpak context")?;
    let ini = config::Ini::parse(text)?;
    verify_deployment(&ini)?;
    let actual: std::collections::BTreeSet<_> = ini
        .get("Context", "filesystems")
        .ok_or("Sandbox did not report filesystem grants")?
        .split(';')
        .filter(|s| !s.is_empty() && !s.starts_with('!'))
        .map(str::to_string)
        .collect();
    let expected: std::collections::BTreeSet<_> = expected
        .iter()
        .map(|s| {
            s.to_str()
                .and_then(|s| s.strip_prefix("--filesystem="))
                .map(str::to_string)
                .ok_or("Invalid visibility option")
        })
        .collect::<Result<_, _>>()?;
    if actual != expected {
        return Err("Sandbox filesystem grants differ from the approved PCSX2 launch".into());
    }
    Ok(())
}

fn command(
    prepared: &Prepared,
    descriptor: i32,
    probe: bool,
) -> Result<PreparedProcessCommand, String> {
    let root = prepared.plan.root();
    let mut args = vec![
        "run".into(),
        scope(prepared.profile.installation_type)?.into(),
        format!("--commit={TESTED_DEPLOYMENT}").into(),
        "--nofilesystem=host:reset".into(),
        format!("--instance-id-fd={descriptor}").into(),
    ];
    if probe {
        args.extend([
            "--die-with-parent".into(),
            "--unshare=network".into(),
            "--nosocket=x11".into(),
            "--nosocket=wayland".into(),
            "--env=QT_QPA_PLATFORM=offscreen".into(),
        ]);
    }
    args.extend(
        prepared
            .visibility
            .flatpak_options()
            .map_err(|e| e.to_string())?,
    );
    // Intrinsic per-app storage is not a filesystem grant. These child-only
    // overrides and the explicit data path keep default settings private.
    for (key, leaf) in [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        args.push(format!("--env={key}={}", root.join(leaf).display()).into());
    }
    args.extend([
        APP_ID.into(),
        "-datapath".into(),
        root.as_os_str().to_owned(),
        "-logfile".into(),
        root.join(if probe {
            "configuration.log"
        } else {
            "activation.log"
        })
        .into_os_string(),
        "-earlyconsolelog".into(),
    ]);
    if probe {
        args.push("-testconfig".into());
    } else {
        args.extend([
            "-batch".into(),
            "--".into(),
            prepared
                .preview
                .request
                .selected_content_path
                .as_os_str()
                .to_owned(),
        ]);
    }
    Ok(PreparedProcessCommand {
        executable: prepared.command.executable.clone(),
        arguments: args,
        working_directory: Some(root.into()),
    })
}

fn open_instance(workspace: &Path) -> Result<File, String> {
    validate_destination_root(workspace).map_err(|e| format!("Unsafe workspace: {e:?}"))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(workspace.join(INSTANCE_FILE))
        .map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Invalid instance descriptor".into());
    }
    file.set_len(0).map_err(|e| e.to_string())?;
    Ok(file)
}

fn spawn(prepared: &Prepared, probe: bool) -> Result<WatchedProcess, String> {
    let descriptor = open_instance(prepared.plan.root())?;
    let command = command(prepared, descriptor.as_raw_fd(), probe)?;
    spawn_watched_process_with_assets(&command, &[&descriptor], |_| {}).map_err(|e| e.to_string())
}

fn wait_id(workspace: &Path, process: &mut WatchedProcess) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(id) = instance(workspace) {
            return Ok(id);
        }
        if process.poll().is_some() || Instant::now() >= deadline {
            return Err("PCSX2 sandbox did not provide lifetime evidence".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn verify_probe_log(prepared: &Prepared) -> Result<(), String> {
    let bytes = config::read_source(&prepared.plan.root().join("configuration.log"), 1024 * 1024)?;
    let text = String::from_utf8_lossy(&bytes);
    if !text.contains(&format!(
        "DataRoot Directory: {}\n",
        prepared.plan.root().join("PCSX2").display()
    )) {
        return Err("PCSX2 did not honor the approved temporary data root".into());
    }
    for (name, path) in &prepared.expected_folders {
        if !text.contains(&format!("{name} Directory: {}\n", path.display())) {
            return Err(format!("PCSX2 did not confirm the pinned {name} directory"));
        }
    }
    Ok(())
}

fn verify_staged_files(prepared: &Prepared) -> Result<(), String> {
    let global = prepared.plan.root().join("PCSX2/inis/PCSX2.ini");
    for file in prepared.plan.files() {
        if file.path == prepared.plan.root().join(INSTANCE_FILE) {
            continue;
        }
        let actual = config::read_source(
            &file.path,
            if file.kind == RuntimeFileKind::VerifiedResource {
                8 * 1024 * 1024
            } else {
                1024 * 1024
            },
        )?;
        if file.path == global {
            let actual = config::Ini::parse(
                std::str::from_utf8(&actual).map_err(|_| "Invalid staged INI")?,
            )?;
            let approved = config::Ini::parse(
                std::str::from_utf8(&file.bytes).map_err(|_| "Invalid approved INI")?,
            )?;
            for (key, _) in &prepared.preview.preserved_folders {
                if actual.get("Folders", key) != approved.get("Folders", key) {
                    return Err(format!("Probe changed pinned {key} routing"));
                }
            }
            for (section, key) in [
                ("Folders", "Cheats"),
                ("EmuCore", "EnableCheats"),
                ("Cheats", "Enable"),
            ] {
                if actual.get(section, key) != approved.get(section, key) {
                    return Err(format!("Probe changed scoped {section}/{key}"));
                }
            }
            for key in [
                "EnablePatches",
                "EnableWideScreenPatches",
                "EnableNoInterlacingPatches",
            ] {
                if approved.get("EmuCore", key).is_some()
                    && actual.get("EmuCore", key) != approved.get("EmuCore", key)
                {
                    return Err(format!("Probe changed {key}"));
                }
            }
        } else if hash(&actual) != file.sha256 {
            return Err("Staged cheat or per-game settings changed before boot".into());
        }
    }
    Ok(())
}

pub(super) fn observe(log: &str, cheat_path: &Path, observation: &mut Pcsx2CheatObservation) {
    observation.file_loaded |= log.lines().any(|line| {
        line.contains("Found ")
            && line.contains(&format!(" cheats in {}.", cheat_path.display()))
            && !line.contains("Found 0 cheats")
    });
    observation.selection_enabled |= log
        .lines()
        .any(|line| line.ends_with(&format!("Enabled patch: {SELECTED_GROUP}")));
}

pub struct Pcsx2RuntimeProcess {
    pid: u32,
    exit: Receiver<ExitSummary>,
    cached: Option<ExitSummary>,
}
impl RuntimeProcess for Pcsx2RuntimeProcess {
    fn pid(&self) -> u32 {
        self.pid
    }
    fn poll_exit(&mut self) -> Option<ExitSummary> {
        if self.cached.is_none() {
            self.cached = self.exit.try_recv().ok();
        }
        self.cached.clone()
    }
}

pub(super) fn probe_and_spawn(
    prepared: &Prepared,
    observation: Arc<Mutex<Pcsx2CheatObservation>>,
) -> Result<Pcsx2RuntimeProcess, String> {
    let mut probe = spawn(prepared, true)?;
    let pid = probe.pid;
    let prepared = prepared.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let workspace = prepared.plan.root();
        let binary = &prepared.binding.executable;
        let mut active_id = None;
        let mut current = None;
        let result = (|| -> Result<ExitSummary, String> {
            let id = wait_id(workspace, &mut probe)?;
            active_id = Some(id.clone());
            verify_context(
                &id,
                &prepared
                    .visibility
                    .flatpak_options()
                    .map_err(|e| e.to_string())?,
            )?;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(exit) = probe.poll() {
                    if !exit.status.as_ref().is_ok_and(|s| s.success()) {
                        return Err("PCSX2 temporary configuration probe failed".into());
                    }
                    break;
                }
                if Instant::now() >= deadline {
                    return Err("PCSX2 temporary configuration probe timed out".into());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if !stop_probe(binary, &id) {
                return Err("The configuration sandbox is still active; workspace retained".into());
            }
            active_id = None;
            verify_probe_log(&prepared)?;
            // Rebuild the complete approval, including formerly absent
            // per-game files. Hashes alone cannot detect a new override.
            let runtime = CheatRuntimeRoots {
                approved_root: workspace
                    .parent()
                    .ok_or("Workspace has no approved parent")?
                    .into(),
            };
            if capture_content(&prepared.preview.request.expected_executable)?
                != prepared.preview.executable
            {
                return Err("The PCSX2 launcher changed before boot".into());
            }
            let fresh = prepare(
                &prepared.preview.request,
                &prepared.roots,
                &prepared.firmware,
                &prepared.preview.source,
                &prepared.preview.launch_id,
                &runtime,
            )?;
            if fresh.preview.digest != prepared.preview.digest
                || fresh.preview.directories != prepared.preview.directories
                || fresh.preview.content != prepared.preview.content
                || fresh.preview.identity != prepared.preview.identity
                || fresh.preview.executable != prepared.preview.executable
                || fresh.preview.deployment != prepared.preview.deployment
            {
                return Err("Game, profile, deployment or cheat evidence changed during the configuration probe".into());
            }
            verify_staged_files(&prepared)?;
            current = Some(spawn(&prepared, false)?);
            observation
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .game_launch_handed_off = true;
            let id = wait_id(workspace, current.as_mut().unwrap())?;
            active_id = Some(id.clone());
            verify_context(
                &id,
                &prepared
                    .visibility
                    .flatpak_options()
                    .map_err(|e| e.to_string())?,
            )?;
            loop {
                if let Ok(bytes) =
                    config::read_source(&workspace.join("activation.log"), 1024 * 1024)
                {
                    observe(
                        &String::from_utf8_lossy(&bytes),
                        &prepared.cheat_path,
                        &mut observation.lock().unwrap_or_else(|e| e.into_inner()),
                    );
                }
                if let Some(exit) = current.as_mut().unwrap().poll()
                    && stopped(binary, &id)
                {
                    return Ok(ExitSummary {
                        success: exit.status.as_ref().is_ok_and(|s| s.success()),
                        code: exit.status.as_ref().ok().and_then(|s| s.code()),
                    });
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        })();
        match result {
            Ok(exit) => {
                let _ = sender.send(exit);
            }
            Err(detail) => {
                observation.lock().unwrap_or_else(|e| e.into_inner()).detail =
                    Some(format!("Launch refused: {detail}"));
                // Failure after starting a sandbox is not permission to
                // delete its files. Stop only this owned probe/failed handoff
                // and retain the workspace whenever lifetime is unknown.
                if active_id.is_none() {
                    active_id = instance(workspace).ok();
                }
                if let Some(id) = active_id {
                    if !stop_probe(binary, &id) {
                        return;
                    }
                } else {
                    // Launcher exit is not proof of sandbox exit when no
                    // valid instance ID was captured. Keep recovery evidence.
                    return;
                }
                let _ = sender.send(ExitSummary {
                    success: false,
                    code: None,
                });
            }
        }
    });
    Ok(Pcsx2RuntimeProcess {
        pid,
        exit: receiver,
        cached: None,
    })
}

pub(super) fn workspace_may_be_active(workspace: &Path) -> bool {
    // Recovery is metadata-only and conservative. Launcher death is not
    // sandbox death. Missing/malformed instance evidence keeps the workspace.
    match instance(workspace) {
        Ok(id) => {
            use super::super::installation_known::{KnownInstallRoots, flatpak_binary};
            let Some(binary) =
                KnownInstallRoots::from_environment().and_then(|r| flatpak_binary(&r))
            else {
                return true;
            };
            !stopped(&binary, &id)
        }
        Err(_) => true,
    }
}
