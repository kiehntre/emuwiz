//! A verified single-disk ADF through the real launch planner to Blue's one
//! Play action, with a fake FS-UAE in a private directory. Identity is built
//! by hand here: core's own tests prove how a DAT produces it.
use super::*;
use crate::launch_readiness_page::{
    AmigaWHDLoadLaunchContext, GamerPlayAction, LaunchReadinessInput, StandaloneLaunchRequest,
    TypedLaunchRequest, gamer_play_action,
};
use archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport;
use archivefs_core::launch::{LaunchContainerKind, LaunchContentRef, build_launch_plan};
use std::fs;

struct Lab {
    _dir: tempfile::TempDir,
    adf: PathBuf,
    roots: AmigaProfileDiscoveryRoots,
    database: PathBuf,
}

fn lab(conf: &str, kickstart: bool) -> Lab {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let adf = base.join("Speedball.adf");
    fs::write(&adf, vec![0u8; 901_120]).unwrap();
    let config_root = base.join("fs-uae");
    fs::create_dir_all(&config_root).unwrap();
    let rom = base.join("kick13.rom");
    if kickstart {
        fs::write(&rom, b"not a verified ROM").unwrap();
    }
    fs::write(
        config_root.join("fs-uae.conf"),
        format!("{conf}\nkickstart_file = {}\n", rom.display()),
    )
    .unwrap();
    let executable = base.join("bin/fs-uae");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    Lab {
        adf,
        roots: AmigaProfileDiscoveryRoots {
            home: base.join("home"),
            xdg_config_home: base.join("xdg-config"),
            xdg_data_home: base.join("xdg-data"),
            explicit_configuration_roots: vec![(AmigaEmulatorKind::FsUae, config_root)],
            portable_configuration_roots: Vec::new(),
            explicit_executables: vec![(AmigaEmulatorKind::FsUae, executable)],
            known_version_outputs: Default::default(),
        },
        // No catalogue exists here, so a click must fail closed.
        database: base.join("absent-library.sqlite3"),
        _dir: dir,
    }
}

fn identity() -> AmigaAdfLaunchIdentity {
    AmigaAdfLaunchIdentity {
        game_key: "adf-sha1:00".into(),
        source_id: "tosec-amiga".into(),
        source_revision: "2026-10-01".into(),
        catalogue_sha256: "0".repeat(64),
        entry_name: "Speedball (1988)(Image Works)(Disk 1 of 1)".into(),
        matched_algorithm: "SHA-1".into(),
        matched_value: "00".into(),
        standalone: archivefs_core::dat::amiga_adf_identity::StandaloneEvidence::TosecSingleMedium,
    }
}

fn play(lab: &Lab) -> (AdfLaunchInputs, GamerPlayAction) {
    let inputs = inputs_for(
        &identity(),
        &lab.adf,
        &lab.roots,
        lab.database.clone(),
        lab.database.with_extension("toml"),
    );
    let content = LaunchContentRef {
        kind: None,
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(lab.adf.clone()),
        requires_mount: false,
        provenance: "test loose ADF".into(),
    };
    let plan = build_launch_plan(
        &inputs.identity,
        &content,
        &inputs.profiles,
        &RetroArchEnvironmentReport {
            format_version: 2,
            profiles: Vec::new(),
            diagnostics: Vec::new(),
        },
        &[],
    );
    let input = LaunchReadinessInput::Plan {
        plan,
        retroarch: None,
        retroarch_scanned: true,
        standalone_scans_complete: true,
        dolphin: None,
        pcsx2: None,
        duckstation: None,
        ppsspp: None,
        rpcs3: None,
        xemu: None,
        xenia: None,
        melonds: None,
        cemu: None,
        flycast: None,
        xroar: None,
        amiga_whdload: Some(AmigaWHDLoadLaunchContext {
            requests: Vec::new(),
            adf: inputs.requests.clone(),
        }),
        #[cfg(target_os = "linux")]
        shadps4: None,
    };
    let action = gamer_play_action(&input);
    (inputs, action)
}

#[test]
fn a_verified_single_disk_adf_offers_play_through_fs_uae_and_a_click_revalidates() {
    let lab = lab("amiga_model = A500", true);
    let (inputs, action) = play(&lab);
    assert_eq!(inputs.requests.len(), 1);
    let GamerPlayAction::Launch(request) = action else {
        panic!("expected Play for a verified single-disk ADF");
    };
    let TypedLaunchRequest::Standalone(StandaloneLaunchRequest::FsUaeAdf(request)) = *request
    else {
        panic!("expected the FS-UAE ADF request");
    };
    assert_eq!(request.fsuae.selected_content_path, lab.adf);
    assert_eq!(request.fsuae.expected_game_key, identity().game_key);
    assert!(request.fsuae.config_path.ends_with("fs-uae.conf"));
    // The click re-derives identity from the catalogue; with none, it refuses.
    let before = fs::read(&lab.adf).unwrap();
    let refused = StandaloneLaunchRequest::FsUaeAdf(request).execute();
    assert!(
        matches!(&refused, Err(reason) if reason.contains("catalogue")),
        "{:?}",
        refused.err()
    );
    assert_eq!(fs::read(&lab.adf).unwrap(), before);
}

#[test]
fn missing_kickstart_or_a_cd32_setup_never_offers_play() {
    let no_rom = lab("amiga_model = A500", false);
    let (inputs, action) = play(&no_rom);
    assert!(inputs.requests.is_empty());
    assert_eq!(inputs.profiles[0].firmware, FirmwareReadiness::Missing);
    assert!(matches!(action, GamerPlayAction::BlockedTyped(_)));

    let cd32 = lab("amiga_model = CD32", true);
    let (inputs, action) = play(&cd32);
    assert!(inputs.requests.is_empty());
    assert!(!inputs.profiles[0].eligible);
    assert!(matches!(action, GamerPlayAction::BlockedTyped(_)));
}

#[test]
fn only_adf_files_reach_the_catalogue_check() {
    assert!(is_amiga_adf(Path::new("/g/a.adf"), Some("Amiga")));
    assert!(is_amiga_adf(Path::new("/g/a.ADF"), None));
    for (path, platform) in [
        ("/g/a.adz", Some("Amiga")),
        ("/g/a.dms", Some("Amiga")),
        ("/g/a.ipf", Some("Amiga")),
        ("/g/a.hdf", Some("Amiga")),
        ("/g/a.adf", Some("AmigaCD32")),
        ("/g/a.adf", Some("Acorn Archimedes")),
    ] {
        assert!(
            !is_amiga_adf(Path::new(path), platform),
            "{path} {platform:?}"
        );
        assert!(launch_inputs(Path::new(path), platform).is_none(), "{path}");
    }
}
