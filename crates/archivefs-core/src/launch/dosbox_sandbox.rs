//! OS-level sandbox for launching DOSBox on an untrusted DOS game folder.
//!
//! Reviewing a game's `dosbox.conf` ([`crate::launch::dosbox_autoexec`]) is not
//! a security boundary: once DOSBox runs, the game and any batch file it runs
//! can issue DOSBox's own `MOUNT`, `IMGMOUNT` and `CONFIG` commands. The
//! boundary is the operating system. DOSBox only ever runs inside
//! [bubblewrap](https://github.com/containers/bubblewrap), which is
//! default-deny for the host filesystem.
//!
//! # What the sandboxed process can see
//!
//! - `/usr` read-only (the emulator, its libraries and its data), and the usual
//!   `/lib`, `/lib64`, `/bin` and `/sbin` links into it;
//! - the loader cache `/etc/ld.so.cache`, read-only;
//! - a private `/proc`, a minimal `/dev`, and a private 64 MiB `/tmp` that
//!   holds a throw-away `HOME`;
//! - the game folder, writable, at its real path. This is the only writable
//!   host path and is how saves work. It is not size-bounded, and a game can
//!   change any file in its own folder, including its next start batch;
//! - only the display, audio and authority sockets or files named in
//!   [`DosBoxHostAccess`], read-only;
//! - the DOSBox executable itself, read-only, when it is outside `/usr`.
//!
//! Everything else is absent: the user's home, other games, saves, databases,
//! `/etc`, `/var`, `/mnt`, other users' files. A `MOUNT`, a batch file, a
//! symlink or a CUE sheet that names such a path finds nothing there.
//!
//! The sandbox also unshares the user, IPC, PID, UTS, cgroup and network
//! namespaces, starts a new session (no terminal injection), forbids nested
//! user namespaces, clears the environment, and runs only `-conf <config>`.
//! Every inherited descriptor above stderr is closed on exec.
//!
//! # Fail closed
//!
//! There is no unsandboxed path. `bwrap` is looked up only at a fixed absolute
//! path, a probe must succeed before anything is spawned, and if `bwrap`
//! cannot set the sandbox up at launch it exits without starting DOSBox.
//!
//! # What this does not provide
//!
//! - **Approval is not security.** The user approving a start command is a
//!   trust decision about the game; the sandbox is what limits the damage.
//! - **X11 is weak.** Any process on the user's X server can read keystrokes
//!   and screen contents of the other clients. Prefer Wayland.
//! - **The game folder is fully exposed to the game**, read and write.
//! - Audio goes through the user's sound server socket; that socket is the
//!   sound server's own attack surface.
//! - DOSBox and the kernel can have vulnerabilities; this is a boundary, not a
//!   proof.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use crate::launch::dosbox_command::DosBoxCommand;
use crate::launch::process_spawn::PreparedProcessCommand;

/// The only place `bwrap` is looked for. Never searched on `PATH`.
pub const BWRAP_PATH: &str = "/usr/bin/bwrap";

/// Size cap of the private `/tmp`.
pub const SANDBOX_TMP_BYTES: u64 = 64 * 1024 * 1024;

/// The explicit, read-only host grants a launch adds on top of the default
/// sandbox, and the only environment DOSBox receives besides `PATH` and `HOME`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DosBoxHostAccess {
    /// Absolute files or sockets bound read-only at their own path, such as a
    /// Wayland socket, an X11 socket, an `XAUTHORITY` file or a sound socket.
    pub read_only_paths: Vec<PathBuf>,
    /// Environment variables DOSBox may see (`DISPLAY`, `WAYLAND_DISPLAY`...).
    pub environment: Vec<(String, String)>,
}

impl DosBoxHostAccess {
    /// No display, no audio, no extra environment.
    pub fn none() -> Self {
        Self::default()
    }

    /// Grants the current desktop session's display and audio, and nothing
    /// else: the Wayland or X11 socket, the X authority file, the PulseAudio
    /// compatible socket, and the variables that name them.
    pub fn from_session_environment() -> Self {
        let mut access = Self::default();
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute() && dir.is_dir());
        let is_socket = |path: &Path| {
            use std::os::unix::fs::FileTypeExt;
            std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket())
        };
        let plain_name = |name: &str| {
            !name.is_empty() && !name.contains('/') && !name.contains('\0') && name != ".."
        };
        if let (Some(dir), Ok(name)) = (&runtime_dir, std::env::var("WAYLAND_DISPLAY"))
            && plain_name(&name)
            && is_socket(&dir.join(&name))
        {
            access.read_only_paths.push(dir.join(&name));
            access
                .environment
                .push(("XDG_RUNTIME_DIR".into(), dir.display().to_string()));
            access.environment.push(("WAYLAND_DISPLAY".into(), name));
        }
        if let Ok(display) = std::env::var("DISPLAY")
            && let Some(number) = display
                .strip_prefix(':')
                .map(|rest| rest.split('.').next().unwrap_or(""))
                .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        {
            let socket = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
            if is_socket(&socket) {
                access.read_only_paths.push(socket);
                access.environment.push(("DISPLAY".into(), display.clone()));
                if let Some(authority) = std::env::var_os("XAUTHORITY").map(PathBuf::from)
                    && authority.is_absolute()
                    && std::fs::symlink_metadata(&authority).is_ok_and(|m| m.is_file())
                {
                    access.read_only_paths.push(authority.clone());
                    access
                        .environment
                        .push(("XAUTHORITY".into(), authority.display().to_string()));
                }
            }
        }
        if let Some(dir) = &runtime_dir {
            let pulse = dir.join("pulse").join("native");
            if is_socket(&pulse) {
                access
                    .environment
                    .push(("PULSE_SERVER".into(), format!("unix:{}", pulse.display())));
                access.read_only_paths.push(pulse);
            }
        }
        access
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DosBoxSandboxError {
    /// `bwrap` is missing, or it could not create the sandbox here (for
    /// example unprivileged user namespaces are disabled).
    Unavailable(String),
    /// A path or grant was refused before anything was built.
    InvalidGrant(String),
}

impl DosBoxSandboxError {
    pub fn detail(&self) -> &str {
        match self {
            Self::Unavailable(detail) | Self::InvalidGrant(detail) => detail,
        }
    }
}

fn clean_absolute(path: &Path, what: &str) -> Result<(), DosBoxSandboxError> {
    let bad = |why: &str| Err(DosBoxSandboxError::InvalidGrant(format!("{what}: {why}")));
    if !path.is_absolute() {
        return bad("the path is not absolute");
    }
    if path.to_string_lossy().contains('\0') {
        return bad("the path contains a NUL");
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return bad("the path contains `.` or `..`");
    }
    Ok(())
}

/// Arguments shared by the real launch and the availability probe.
fn base_arguments() -> Vec<OsString> {
    fn add(args: &mut Vec<OsString>, items: &[&str]) {
        args.extend(items.iter().map(OsString::from));
    }
    let mut args: Vec<OsString> = Vec::new();
    add(
        &mut args,
        &[
            "--unshare-user",
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-uts",
            "--unshare-cgroup",
            "--disable-userns",
            "--new-session",
            "--clearenv",
        ],
    );
    add(&mut args, &["--ro-bind", "/usr", "/usr"]);
    add(&mut args, &["--symlink", "usr/lib", "/lib"]);
    add(&mut args, &["--symlink", "usr/lib64", "/lib64"]);
    add(&mut args, &["--symlink", "usr/bin", "/bin"]);
    add(&mut args, &["--symlink", "usr/sbin", "/sbin"]);
    add(
        &mut args,
        &["--ro-bind-try", "/etc/ld.so.cache", "/etc/ld.so.cache"],
    );
    add(&mut args, &["--proc", "/proc", "--dev", "/dev"]);
    args.push("--size".into());
    args.push(SANDBOX_TMP_BYTES.to_string().into());
    add(&mut args, &["--tmpfs", "/tmp", "--dir", "/tmp/home"]);
    add(&mut args, &["--setenv", "PATH", "/usr/bin:/bin"]);
    add(&mut args, &["--setenv", "HOME", "/tmp/home"]);
    args
}

/// Proves that `bwrap` exists at `bwrap` and can build the sandbox on this
/// machine, by running `true` inside it. Nothing from the game is touched.
pub fn probe_sandbox(bwrap: &Path) -> Result<(), DosBoxSandboxError> {
    let unavailable =
        |why: String| DosBoxSandboxError::Unavailable(format!("{}: {why}", bwrap.display()));
    let metadata = std::fs::symlink_metadata(bwrap)
        .map_err(|error| unavailable(format!("bubblewrap is not installed ({error})")))?;
    if !metadata.is_file() {
        return Err(unavailable("it is not a regular file".into()));
    }
    let mut arguments = base_arguments();
    arguments.push("--".into());
    arguments.push("/usr/bin/true".into());
    let output = Command::new(bwrap)
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| unavailable(error.to_string()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(unavailable(format!(
        "the sandbox could not be created ({}): {}",
        output.status,
        stderr.trim().chars().take(300).collect::<String>()
    )))
}

/// Builds the exact argv that runs `command` inside the sandbox. Pure: nothing
/// is probed or spawned; only metadata is read. Every part is a separate argument; there is no shell.
pub fn sandboxed_command(
    command: &DosBoxCommand,
    access: &DosBoxHostAccess,
    bwrap: &Path,
) -> Result<PreparedProcessCommand, DosBoxSandboxError> {
    let game = command
        .working_directory
        .as_deref()
        .ok_or_else(|| DosBoxSandboxError::InvalidGrant("no game folder to expose".into()))?;
    clean_absolute(game, "game folder")?;
    clean_absolute(&command.executable, "DOSBox executable")?;
    if !std::fs::symlink_metadata(&command.executable).is_ok_and(|m| m.is_file()) {
        return Err(DosBoxSandboxError::InvalidGrant(
            "the DOSBox executable is missing or is not a regular file".into(),
        ));
    }
    if game.components().count() < 3 {
        return Err(DosBoxSandboxError::InvalidGrant(
            "the game folder is too broad to expose".into(),
        ));
    }
    // Never expose a folder that contains the user's home.
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && home.starts_with(game)
    {
        return Err(DosBoxSandboxError::InvalidGrant(
            "the game folder contains the home folder".into(),
        ));
    }
    // The emulator must not come from the game's own writable folder.
    if command.executable.starts_with(game) {
        return Err(DosBoxSandboxError::InvalidGrant(
            "the DOSBox executable is inside the game folder".into(),
        ));
    }
    for path in &access.read_only_paths {
        clean_absolute(path, "granted path")?;
        if path.components().count() < 3 || path.starts_with(game) || game.starts_with(path) {
            return Err(DosBoxSandboxError::InvalidGrant(format!(
                "granted path {} is too broad or overlaps the game folder",
                path.display()
            )));
        }
    }
    for (key, value) in &access.environment {
        if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
            return Err(DosBoxSandboxError::InvalidGrant(
                "an environment grant is malformed".into(),
            ));
        }
        if key == "PATH" || key == "HOME" {
            return Err(DosBoxSandboxError::InvalidGrant(format!(
                "`{key}` is fixed by the sandbox"
            )));
        }
    }

    let mut arguments = base_arguments();
    if !command.executable.starts_with("/usr") {
        arguments.push("--ro-bind".into());
        arguments.push(command.executable.clone().into());
        arguments.push(command.executable.clone().into());
    }
    for path in &access.read_only_paths {
        arguments.push("--ro-bind".into());
        arguments.push(path.clone().into());
        arguments.push(path.clone().into());
    }
    for (key, value) in &access.environment {
        arguments.push("--setenv".into());
        arguments.push(key.into());
        arguments.push(value.into());
    }
    arguments.push("--bind".into());
    arguments.push(game.into());
    arguments.push(game.into());
    arguments.push("--chdir".into());
    arguments.push(game.into());
    arguments.push("--".into());
    arguments.push(command.executable.clone().into());
    arguments.extend(command.arguments.iter().cloned());

    Ok(PreparedProcessCommand {
        executable: bwrap.to_path_buf(),
        arguments,
        working_directory: Some(game.to_path_buf()),
    })
}

#[cfg(test)]
mod tests;
