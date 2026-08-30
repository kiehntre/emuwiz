use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;
use crate::emulator_environment::EncodedPath;

fn bin(name: &str) -> Option<PathBuf> {
    for dir in ["/bin", "/usr/bin"] {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Writes an executable script to `path` via a temp name + rename, so the
/// final path is never held open for writing while a sibling test exec's it
/// (avoids a cross-test `ETXTBSY` race under full-suite parallelism).
#[cfg(unix)]
fn write_exec(path: &Path, body: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    let staging = path.with_extension("staging");
    std::fs::write(&staging, body).unwrap();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&staging, path).unwrap();
}

fn install(emulator: &str, executable: Option<&str>) -> LinuxEmulatorInstallationEvidence {
    LinuxEmulatorInstallationEvidence {
        emulator: emulator.to_string(),
        installation_form: "Native/PATH".to_string(),
        executable: executable.map(|path| EncodedPath::from_path(Path::new(path))),
        profile: None,
        detail: "found".to_string(),
    }
}

#[test]
fn probe_passes_the_argument_literally_never_through_a_shell() {
    let Some(echo) = bin("echo") else {
        return;
    };
    // If this were handled by a shell, the command substitution would run and
    // the backticks / `$(...)` would not survive verbatim.
    let payload = "1.2.3; rm -rf / `id` $(whoami)";
    let output = probe_command_output(&echo, std::ffi::OsStr::new(payload), Duration::from_secs(5))
        .expect("echo should produce output");
    assert_eq!(
        output, payload,
        "the argument must reach the process unmodified"
    );
}

#[test]
fn probe_output_is_capped() {
    let Some(echo) = bin("echo") else {
        return;
    };
    let huge = "x".repeat(MAX_PROBE_OUTPUT * 4);
    let output = probe_command_output(&echo, std::ffi::OsStr::new(&huge), Duration::from_secs(5))
        .expect("echo should produce output");
    assert!(
        output.len() <= MAX_PROBE_OUTPUT,
        "captured {} bytes, cap is {MAX_PROBE_OUTPUT}",
        output.len()
    );
}

#[test]
fn probe_times_out_and_returns_none_without_waiting_for_the_process() {
    let Some(sleep) = bin("sleep") else {
        return;
    };
    let started = Instant::now();
    let output = probe_command_output(
        &sleep,
        std::ffi::OsStr::new("30"),
        Duration::from_millis(150),
    );
    assert!(output.is_none(), "a timed-out probe yields no version");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the probe must not block for the full sleep",
    );
}

#[test]
fn probe_of_a_missing_executable_is_none() {
    let output = probe_command_output(
        Path::new("/nonexistent/archivefs-not-a-real-binary"),
        std::ffi::OsStr::new("-version"),
        Duration::from_secs(5),
    );
    assert!(output.is_none());
}

#[test]
fn flatpak_shim_is_never_a_probe_target() {
    let installations = [install("MAME", Some("/usr/bin/flatpak"))];
    assert!(probe_target(&installations, ArcadeEmulator::Mame).is_none());
}

#[cfg(unix)]
#[test]
fn a_symlinked_executable_is_not_probed() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("mame-real");
    write_exec(&real, b"#!/bin/sh\n");
    let link = dir.path().join("mame");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let installations = [install("MAME", Some(link.to_str().unwrap()))];
    assert!(probe_target(&installations, ArcadeEmulator::Mame).is_none());
}

#[cfg(unix)]
#[test]
fn probe_target_finds_a_real_mame_binary_and_ignores_fbneo() {
    let dir = tempfile::tempdir().unwrap();
    let mame = dir.path().join("mame");
    write_exec(&mame, b"#!/bin/sh\necho '0.216 (mame0216)'\n");
    let installations = [
        install("MAME", Some(mame.to_str().unwrap())),
        install("FinalBurn Neo", Some(mame.to_str().unwrap())),
    ];
    assert_eq!(
        probe_target(&installations, ArcadeEmulator::Mame),
        Some(mame.as_path())
    );
    // FBNeo is deliberately never probed by this module.
    assert!(
        probe_arcade_emulator_versions(&installations)
            .iter()
            .all(|(emulator, _)| *emulator == ArcadeEmulator::Mame)
    );
}

#[cfg(unix)]
#[test]
fn probe_arcade_emulator_versions_reads_a_stub_mame_version() {
    let dir = tempfile::tempdir().unwrap();
    let mame = dir.path().join("mame");
    // A stub that behaves like `mame -version`: prints the banner, exits 0.
    write_exec(
        &mame,
        b"#!/bin/sh\nif [ \"$1\" = \"-version\" ]; then echo '0.270 (mame0270)'; exit 0; fi\nexit 1\n",
    );
    let installations = [install("MAME", Some(mame.to_str().unwrap()))];
    let outputs = probe_arcade_emulator_versions(&installations);
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].0, ArcadeEmulator::Mame);
    assert!(outputs[0].1.contains("0.270"));
}
