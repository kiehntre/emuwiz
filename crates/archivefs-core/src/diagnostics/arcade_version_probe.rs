//! Bounded, read-only version probe for an installed MAME executable.
//!
//! # This is a diagnostic probe, not a launch
//!
//! Everything else in the arcade DAT-compatibility path
//! ([`super::arcade_dat_version`]) is pure string work. This module is the one
//! place that runs a child process, and it does so under strict limits, in the
//! same style as [`crate::scummvm_detection::run_detector`]:
//!
//! - [`std::process::Command`] directly - **never a shell**. There is no
//!   `sh -c`, and the single argument is passed as one `OsStr`, so shell
//!   metacharacters in it are inert.
//! - one fixed argument, `-version`, which MAME documents as "Displays current
//!   MAME version and copyright notice"
//!   (<https://docs.mamedev.org/commandline/commandline-all.html>). MAME prints
//!   it and exits; the emulator UI is never started.
//! - `stdin` is `/dev/null`; `stdout` and `stderr` are read through a reader
//!   that stops at [`MAX_PROBE_OUTPUT`] bytes each.
//! - a [`PROBE_TIMEOUT`] wall-clock cap. On expiry the child is killed and the
//!   probe returns `None`, which the compatibility model reports as
//!   "version unknown" rather than a guess.
//!
//! # Why only MAME
//!
//! FinalBurn Neo's standalone builds document no equivalent non-launching
//! version query, so probing one with an unrecognised flag would risk starting
//! it. FBNeo is therefore left un-probed here: its compatibility stays
//! [`super::arcade_dat_version::ArcadeDatVersionCompatibility::Unknown`] unless
//! a version string is supplied some other way. Detecting the executable is a
//! separate question from being able to read its version, and the model
//! already treats "detected, version unknown" honestly.
//!
//! # Nothing is written
//!
//! `mame -version` creates no configuration file (only `-createconfig` does),
//! reads no ROM, and opens no socket.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::arcade_dat_version::ArcadeEmulator;
use super::profiles::LinuxEmulatorInstallationEvidence;

/// The most stdout (and, separately, stderr) bytes the probe will read from a
/// probed process. A version banner is a few hundred bytes; this is generous
/// headroom while still bounding a misbehaving binary.
pub const MAX_PROBE_OUTPUT: usize = 8 * 1024;

/// How long the probe waits for the process to exit on its own before killing
/// it. `-version` returns effectively instantly; this only matters if the
/// binary ignores the flag and does something else.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

fn read_bounded(mut reader: impl Read, cap: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    while bytes.len() < cap {
        let limit = (cap - bytes.len()).min(buffer.len());
        match reader.read(&mut buffer[..limit]) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
        }
    }
    bytes
}

/// Runs `executable <arg>` under the probe's full sandboxing and returns the
/// captured output (stdout, plus stderr appended when non-empty), trimmed.
///
/// Returns `None` if the process could not be spawned, timed out (it is
/// killed in that case), or produced nothing. The exit status is not required
/// to be success: some builds print their banner to stderr or exit non-zero,
/// and the strict version parser downstream rejects anything that is not a
/// real version anyway.
fn probe_command_output(executable: &Path, arg: &OsStr, timeout: Duration) -> Option<String> {
    let mut child = Command::new(executable)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let stdout_thread = thread::spawn(move || read_bounded(stdout, MAX_PROBE_OUTPUT));
    let stderr_thread = thread::spawn(move || read_bounded(stderr, MAX_PROBE_OUTPUT));

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) if start.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    let mut text = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr);
    if !stderr.trim().is_empty() {
        text.push('\n');
        text.push_str(&stderr);
    }
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Whether this path is a real, directly-executable regular file - not a
/// symlink, and not the `/usr/bin/flatpak` shim a Flatpak installation's
/// evidence records (running `flatpak -version` would report Flatpak's
/// version, not the emulator's).
fn probeable_executable(path: &Path) -> bool {
    if path.file_name().and_then(|name| name.to_str()) == Some("flatpak") {
        return false;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The first probeable executable path discovered for `emulator`, if any.
fn probe_target<'a>(
    installations: &'a [LinuxEmulatorInstallationEvidence],
    emulator: ArcadeEmulator,
) -> Option<&'a Path> {
    let name = match emulator {
        ArcadeEmulator::Mame => "MAME",
        ArcadeEmulator::Fbneo => "FinalBurn Neo",
    };
    installations.iter().find_map(|item| {
        if item.emulator != name {
            return None;
        }
        let encoded = item.executable.as_ref()?;
        // A lossy rendering is not a byte-faithful path; do not run it.
        if encoded.lossy {
            return None;
        }
        let path = Path::new(&encoded.display);
        probeable_executable(path).then_some(path)
    })
}

/// Captures live emulator-version output for the arcade compatibility model,
/// in the exact `(emulator, output)` shape
/// [`super::arcade_dat_version::arcade_dat_version_readiness`] consumes.
///
/// Only MAME is probed (see the module docs). The result is empty when no
/// probeable MAME executable was found or the probe produced nothing - the
/// model then reports MAME as "detected, version unknown" when an executable
/// exists at all.
pub fn probe_arcade_emulator_versions(
    installations: &[LinuxEmulatorInstallationEvidence],
) -> Vec<(ArcadeEmulator, String)> {
    let mut outputs = Vec::new();
    if let Some(path) = probe_target(installations, ArcadeEmulator::Mame) {
        if let Some(output) = probe_command_output(path, OsStr::new("-version"), PROBE_TIMEOUT) {
            outputs.push((ArcadeEmulator::Mame, output));
        }
    }
    outputs
}

#[cfg(test)]
mod tests;
