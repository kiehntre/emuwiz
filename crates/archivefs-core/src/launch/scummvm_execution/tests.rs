use super::*;

/// Writes a tiny shell script that prints `output`, and does not return until
/// that script is genuinely spawnable.
///
/// Unlike the other launch fixtures, ScummVM's preflight actually `execve`s
/// this file (through the detector). In a multithreaded test binary a
/// concurrent `fork`+`exec` in another test can briefly leave a writable
/// descriptor to a just-written file alive, so the first `execve` here can
/// transiently fail with `ETXTBSY` and surface as a spurious
/// `ScummVmGameIdUnavailable`. The write is made fully durable and its handle
/// dropped before `chmod`, then spawnability is confirmed with a bounded,
/// yield-only retry (no timed sleep) so every caller sees a ready executable.
#[cfg(unix)]
fn executable_fixture(root: &std::path::Path, output: &str) -> std::path::PathBuf {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    let path = root.join("scummvm-fixture");
    {
        let mut file = std::fs::File::create(&path).unwrap();
        write!(file, "#!/bin/sh\nprintf '%s\\n' '{output}'\n").unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();
    } // handle closed here, before the mode change
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

    const ETXTBSY: i32 = 26;
    for attempt in 0.. {
        match Command::new(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let _ = child.wait();
                break;
            }
            Err(error) if error.raw_os_error() == Some(ETXTBSY) && attempt < 10_000 => {
                std::thread::yield_now();
            }
            Err(error) => panic!("scummvm fixture never became spawnable: {error}"),
        }
    }
    path
}

#[test]
fn malformed_request_is_rejected_before_any_spawn() {
    let result = preflight_scummvm_launch(&ScummVmLaunchRequest {
        selected_game_folder: "relative/game".into(),
        expected_game_key: "scumm:game".into(),
        expected_executable: "/does/not/exist".into(),
    });
    assert_eq!(
        result.unwrap_err().kind,
        ScummVmLaunchPreflightErrorKind::ContentPathNotAbsolute
    );
}

#[cfg(unix)]
#[test]
fn fresh_detector_evidence_builds_a_command_for_a_renamed_folder() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder-name-is-not-used");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("resource.dat"), b"content").unwrap();
    let executable = executable_fixture(root.path(), "Game ID: sci:monkey");
    let command = preflight_scummvm_launch(&ScummVmLaunchRequest {
        selected_game_folder: folder.clone(),
        expected_game_key: "sci:monkey".into(),
        expected_executable: executable,
    })
    .unwrap();
    assert_eq!(command.arguments[0], "-p");
    assert_eq!(command.arguments[1], folder.as_os_str());
    assert_eq!(command.arguments[2], "sci:monkey");
}

#[cfg(unix)]
#[test]
fn fresh_detector_disagreement_refuses_before_command_creation() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let executable = executable_fixture(root.path(), "Game ID: sci:other");
    let error = preflight_scummvm_launch(&ScummVmLaunchRequest {
        selected_game_folder: folder,
        expected_game_key: "sci:monkey".into(),
        expected_executable: executable,
    })
    .unwrap_err();
    assert_eq!(
        error.kind,
        ScummVmLaunchPreflightErrorKind::IdentityMismatch
    );
}
