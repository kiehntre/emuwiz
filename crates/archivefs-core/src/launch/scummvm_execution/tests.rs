use super::*;

#[cfg(unix)]
fn executable_fixture(root: &std::path::Path, output: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("scummvm-fixture");
    std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{output}'\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
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
