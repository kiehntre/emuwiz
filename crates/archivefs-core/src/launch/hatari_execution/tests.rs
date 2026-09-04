use super::*;
use std::os::unix::fs::PermissionsExt;

fn setup() -> (
    tempfile::TempDir,
    HatariLaunchRequest,
    HatariProfileDiscoveryRoots,
) {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config/hatari/hatari.cfg");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        b"[System]\nnModelType=0\n[ROM]\nszTosImageFileName=tos.img\n",
    )
    .unwrap();
    std::fs::write(config.parent().unwrap().join("tos.img"), b"tos").unwrap();
    let executable = dir.path().join("hatari");
    std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let content = dir.path().join("Game.st");
    std::fs::write(&content, b"disk").unwrap();
    let roots = HatariProfileDiscoveryRoots {
        home: dir.path().join("home"),
        xdg_config_home: dir.path().join("config"),
        xdg_data_home: dir.path().join("data"),
        explicit_config_roots: Vec::new(),
        portable_config_roots: Vec::new(),
        explicit_executables: vec![executable.clone()],
        known_version_outputs: Default::default(),
        appimage_directory: None,
    };
    let request = HatariLaunchRequest {
        selected_content_path: content.clone(),
        expected_platform_id: "AtariST".into(),
        profile_id: format!("hatari:{}", config.display()),
        expected_executable: executable,
        expected_config_path: config.clone(),
        expected_machine_model: HatariMachineModel::St,
        disk_drive: 'A',
        ipf_backend_available: false,
        expected_content_identity: Some(CapturedFileIdentity::capture(
            &std::fs::symlink_metadata(&content).unwrap(),
        )),
        expected_config_identity: Some(CapturedFileIdentity::capture(
            &std::fs::symlink_metadata(&config).unwrap(),
        )),
        tos_references: Vec::new(),
    };
    (dir, request, roots)
}

fn identity() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(crate::launch::planning::ResolvedIdentity {
        platform_id: "AtariST".into(),
        game_key: "Game".into(),
    })
}

#[test]
fn preflight_rechecks_config_content_and_executable() {
    let (_dir, request, roots) = setup();
    let command = preflight_hatari_launch(&request, &roots, &identity()).unwrap();
    assert_eq!(command.arguments[0], "--configfile");
    assert_eq!(command.arguments[2], "--disk-a");
}

#[test]
fn same_path_content_replacement_and_symlink_are_rejected() {
    let (_dir, mut request, roots) = setup();
    std::fs::write(&request.selected_content_path, b"replacement").unwrap();
    assert_eq!(
        preflight_hatari_launch(&request, &roots, &identity())
            .unwrap_err()
            .kind,
        HatariLaunchPreflightErrorKind::ContentChangedBeforeSpawn
    );
    request.expected_content_identity = None;
    let other = request.selected_content_path.with_file_name("other.st");
    std::fs::write(&other, b"other").unwrap();
    std::fs::remove_file(&request.selected_content_path).unwrap();
    std::os::unix::fs::symlink(other, &request.selected_content_path).unwrap();
    assert_eq!(
        preflight_hatari_launch(&request, &roots, &identity())
            .unwrap_err()
            .kind,
        HatariLaunchPreflightErrorKind::ContentIsSymlink
    );
}
