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

/// A structurally plausible DD floppy: right size and a `DOS\0` boot block.
fn floppy() -> Vec<u8> {
    let mut bytes = vec![0u8; 901_120];
    bytes[..4].copy_from_slice(b"DOS\0");
    bytes
}

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
    fs::write(&adf, floppy()).unwrap();
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
        vice: None,
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

fn local_play(lab: &Lab) -> (AdfLaunchInputs, GamerPlayAction) {
    let media = local_adf_media(&lab.adf).expect("a plain single floppy");
    let inputs = local_inputs_for(
        &media,
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
        vice: None,
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

fn local_request(lab: &Lab) -> archivefs_core::launch::FsUaeAdfLaunchRequest {
    let (inputs, action) = local_play(lab);
    assert_eq!(inputs.requests.len(), 1);
    let GamerPlayAction::Launch(request) = action else {
        panic!("expected Play for a local single-disk ADF");
    };
    let TypedLaunchRequest::Standalone(StandaloneLaunchRequest::FsUaeAdf(request)) = *request
    else {
        panic!("expected the FS-UAE ADF request");
    };
    *request
}

fn preflight_local(
    request: &archivefs_core::launch::FsUaeAdfLaunchRequest,
    lab: &Lab,
) -> Result<
    archivefs_core::launch::process_spawn::PreparedProcessCommand,
    archivefs_core::launch::FsUaeAdfLaunchError,
> {
    // An empty catalogue: the file is not in the library, so no DAT speaks.
    let database = archivefs_core::Database::open_or_create(lab.database.clone()).unwrap();
    archivefs_core::launch::preflight_fsuae_adf_launch(
        request,
        &database,
        &std::collections::HashMap::new(),
    )
}

#[test]
fn an_unverified_single_floppy_plays_through_the_local_media_route() {
    let lab = lab("amiga_model = A500", true);
    let request = local_request(&lab);
    assert!(matches!(
        &request.route,
        archivefs_core::launch::FsUaeAdfRoute::LocalMedia { sha256 } if sha256.len() == 64
    ));
    // Not a catalogue game: the key is namespaced to the bytes.
    assert!(
        request
            .fsuae
            .expected_game_key
            .starts_with("local-adf-sha256:")
    );
    let before = fs::read(&lab.adf).unwrap();
    let command = preflight_local(&request, &lab).expect("launchable");
    let mut media = std::ffi::OsString::from("--floppy-drive-0=");
    media.push(&lab.adf);
    assert_eq!(command.arguments.len(), 2);
    assert_eq!(command.arguments[1], media);
    assert_eq!(fs::read(&lab.adf).unwrap(), before);
}

#[test]
fn a_changed_floppy_is_refused_at_click_time() {
    let lab = lab("amiga_model = A500", true);
    let request = local_request(&lab);
    // Same size, different bytes.
    let mut bytes = floppy();
    bytes[2000] = 7;
    fs::write(&lab.adf, bytes).unwrap();
    let refused = preflight_local(&request, &lab);
    assert!(refused.is_err(), "{refused:?}");
    // Replaced by something that is no longer a floppy at all.
    fs::write(&lab.adf, b"nothing").unwrap();
    assert!(preflight_local(&request, &lab).is_err());
}

#[test]
fn numbered_or_sibling_disks_are_not_a_single_floppy() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["Game (Disk 1 of 2).adf", "Game Disk 2.adf"] {
        let path = dir.path().join(name);
        fs::write(&path, floppy()).unwrap();
        let refused = local_adf_media(&path);
        assert!(refused.is_err(), "{name}: {refused:?}");
    }
    // A plain name beside a numbered sibling is also refused.
    let single = tempfile::tempdir().unwrap();
    let one = single.path().join("Monkey Island Disk 1.adf");
    let two = single.path().join("Monkey Island Disk 2.adf");
    fs::write(&one, floppy()).unwrap();
    fs::write(&two, floppy()).unwrap();
    assert!(local_adf_media(&one).is_err());
    assert!(local_adf_media(&two).is_err());
    let alone = single.path().join("Other.adf");
    fs::write(&alone, floppy()).unwrap();
    assert!(local_adf_media(&alone).is_ok());
}

#[test]
fn only_a_plausible_amiga_floppy_image_counts_as_media() {
    let dir = tempfile::tempdir().unwrap();
    let wrong_size = dir.path().join("short.adf");
    fs::write(&wrong_size, vec![0u8; 1000]).unwrap();
    let no_boot = dir.path().join("zeros.adf");
    fs::write(&no_boot, vec![0u8; 901_120]).unwrap();
    let not_adf = dir.path().join("game.hdf");
    fs::write(&not_adf, floppy()).unwrap();
    let link = dir.path().join("link.adf");
    let real = dir.path().join("real.adf");
    fs::write(&real, floppy()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    for path in [&wrong_size, &no_boot, &not_adf, &link] {
        assert!(local_adf_media(path).is_err(), "{path:?}");
    }
    assert!(local_adf_media(&real).is_ok());
}

#[test]
fn the_local_route_keeps_kickstart_and_machine_refusals() {
    let no_rom = lab("amiga_model = A500", false);
    let (inputs, action) = local_play(&no_rom);
    assert!(inputs.requests.is_empty());
    assert_eq!(inputs.profiles[0].firmware, FirmwareReadiness::Missing);
    assert!(matches!(action, GamerPlayAction::BlockedTyped(_)));

    let cd32 = lab("amiga_model = CD32", true);
    let (inputs, action) = local_play(&cd32);
    assert!(inputs.requests.is_empty());
    assert!(!inputs.profiles[0].eligible);
    assert!(matches!(action, GamerPlayAction::BlockedTyped(_)));

    // A Kickstart that disappears after planning is refused at the click.
    let lab = lab("amiga_model = A500", true);
    let request = local_request(&lab);
    fs::remove_file(lab.adf.parent().unwrap().join("kick13.rom")).unwrap();
    assert!(preflight_local(&request, &lab).is_err());
}

#[test]
fn catalogue_evidence_against_a_standalone_game_blocks_the_local_route() {
    use archivefs_core::dat::amiga_adf_identity::AmigaAdfIdentityRefusal as R;
    for against in [
        R::Conflicting(vec!["a".into(), "b".into()]),
        R::NotSingleRom(2),
        R::DiskNumbered("Game".into()),
        R::NumberedSiblings("Game (Disk 2)".into()),
        R::NotStandaloneGame("an add-on".into()),
    ] {
        assert!(against.is_evidence_against_standalone(), "{against:?}");
    }
    for open in [
        R::NotInLibrary,
        R::NoCurrentMatch,
        R::CatalogueStale,
        R::InventoryMissing,
        R::CompletenessUnknown,
        R::FileChanged,
    ] {
        assert!(!open.is_evidence_against_standalone(), "{open:?}");
    }
}
