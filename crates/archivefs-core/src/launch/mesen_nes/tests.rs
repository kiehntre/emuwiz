use super::*;
use crate::patch_manager::*;
use std::collections::BTreeMap;
use tempfile::tempdir;
fn profile(root: &Path) -> MesenProfile {
    let config_path = root.join("settings.json");
    MesenProfile {
        profile_id: "fixture".into(),
        installation_type: MesenInstallationType::Explicit,
        configuration_path: root.into(),
        config_path: config_path.clone(),
        config: MesenConfigInspection {
            path: config_path,
            exists: true,
            readable: true,
            oversized: false,
        },
        eligible: true,
        blocker: None,
        executable_candidates: vec![],
    }
}
#[test]
fn filenames_and_size_never_verify_fds_firmware() {
    let d = tempdir().unwrap();
    let p = profile(d.path());
    let fds = d.path().join("game.fds");
    assert_eq!(
        inspect_mesen_nes_firmware(&p, &fds).firmware,
        FirmwareReadiness::Missing
    );
    std::fs::create_dir(d.path().join("Firmware")).unwrap();
    for name in ["disksys.rom", "FdsBios.bin"] {
        std::fs::write(d.path().join("Firmware").join(name), vec![0; 8192]).unwrap();
    }
    let state = inspect_mesen_nes_firmware(&p, &fds);
    assert_eq!(state.firmware, FirmwareReadiness::PresentUnverified);
    assert!(state.firmware_sha256.is_none());
    assert!(state.detail.contains("disksys.rom"));
    std::fs::remove_file(d.path().join("Firmware/disksys.rom")).unwrap();
    assert!(
        inspect_mesen_nes_firmware(&p, &fds)
            .detail
            .contains("FdsBios.bin")
    );
    assert_eq!(
        inspect_mesen_nes_firmware(&p, &d.path().join("game.nes")).firmware,
        FirmwareReadiness::NotRequired
    );
}
#[test]
fn configuration_binding_and_forwarding_are_checked() {
    let d = tempdir().unwrap();
    let root = d.path().join("config/Mesen2");
    std::fs::create_dir_all(&root).unwrap();
    let p = profile(&root);
    let exe = d.path().join("Mesen");
    let roots = MesenProfileDiscoveryRoots {
        home: d.path().into(),
        xdg_config_home: d.path().join("config"),
        explicit_configuration_roots: vec![],
        explicit_executables: vec![],
        known_version_outputs: BTreeMap::new(),
    };
    for bytes in [
        "{",
        "null",
        "{}",
        r#"{"Preferences":{"SingleInstance":true}}"#,
        r#"{"Preferences":{"SingleInstance":"false"}}"#,
    ] {
        std::fs::write(&p.config_path, bytes).unwrap();
        assert!(validate_mesen_profile_binding(&p, &roots, &exe).is_err());
    }
    std::fs::write(
        &p.config_path,
        r#"{"Preferences":{"SingleInstance":false}}"#,
    )
    .unwrap();
    assert!(validate_mesen_profile_binding(&p, &roots, &exe).is_ok());
    std::fs::write(d.path().join("settings.json"), "{}").unwrap();
    assert!(
        validate_mesen_profile_binding(&p, &roots, &exe)
            .unwrap_err()
            .contains("different settings")
    );
}
#[cfg(unix)]
#[test]
fn firmware_symlinks_are_refused() {
    let d = tempdir().unwrap();
    let p = profile(d.path());
    std::fs::create_dir(d.path().join("Firmware")).unwrap();
    let data = d.path().join("bytes");
    std::fs::write(&data, vec![0; 8192]).unwrap();
    std::os::unix::fs::symlink(data, d.path().join("Firmware/disksys.rom")).unwrap();
    assert_eq!(
        inspect_mesen_nes_firmware(&p, &d.path().join("game.fds")).firmware,
        FirmwareReadiness::Unknown
    );
}
#[test]
fn suffixes_do_not_establish_media_support() {
    let d = tempdir().unwrap();
    for name in ["game.fds", "game.nes", "game.unf", "game.zip"] {
        let file = d.path().join(name);
        std::fs::write(&file, b"arbitrary bytes").unwrap();
        assert!(validate_mesen_nes_media(&file).is_err());
    }
}
#[test]
fn syntax_valid_but_uncertified_typed_settings_are_refused() {
    let d = tempdir().unwrap();
    let p = profile(d.path());
    std::fs::write(
        &p.config_path,
        r#"{"Preferences":{"SingleInstance":false},"Nes":42}"#,
    )
    .unwrap();
    let roots = MesenProfileDiscoveryRoots {
        home: d.path().into(),
        xdg_config_home: d.path().into(),
        explicit_configuration_roots: vec![],
        explicit_executables: vec![],
        known_version_outputs: BTreeMap::new(),
    };
    assert!(
        validate_mesen_profile_binding(&p, &roots, &d.path().join("Mesen"))
            .unwrap_err()
            .contains("typed settings schema")
    );
}

#[cfg(unix)]
#[test]
fn broken_firmware_directory_is_unknown_instead_of_proven_missing() {
    let d = tempdir().unwrap();
    let p = profile(d.path());
    std::os::unix::fs::symlink(
        d.path().join("missing-directory"),
        d.path().join("Firmware"),
    )
    .unwrap();
    assert_eq!(
        inspect_mesen_nes_firmware(&p, &d.path().join("game.fds")).firmware,
        FirmwareReadiness::Unknown
    );
}
