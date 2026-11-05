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
    let adf = base.join("games/amiga/Speedball.adf");
    fs::create_dir_all(adf.parent().unwrap()).unwrap();
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
        azahar: None,
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
        "Amiga",
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
        azahar: None,
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
    let mut database = archivefs_core::Database::open_or_create(lab.database.clone()).unwrap();
    // The click needs the file in the library, as the plan did.
    let base = lab._dir.path();
    archivefs_core::scan_and_persist(
        &mut database,
        &archivefs_core::Config {
            configured_source_roots: None,
            source_folders: vec![base.join("games")],
            mount_root: base.join("mount"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        },
        "test",
    )
    .unwrap();
    // A loaded (but irrelevant) catalogue: the local route refuses without one.
    let mut catalogues = std::collections::HashMap::new();
    catalogues.insert(
        "test".to_string(),
        archivefs_core::dat::configured_catalogues::ConfiguredCatalogue {
            reference: archivefs_core::dat::catalogue_selection::CatalogueRef::Local {
                source_id: "test".into(),
                member: None,
            },
            source_id: "test".into(),
            display_name: "test".into(),
            path: lab.adf.with_extension("dat"),
            kind: archivefs_core::dat::sources::DatSourceKind::File,
            sha256: None,
            revision: None,
        },
    );
    archivefs_core::launch::preflight_fsuae_adf_launch(request, &database, &catalogues)
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
    fs::remove_file(lab._dir.path().join("kick13.rom")).unwrap();
    assert!(preflight_local(&request, &lab).is_err());
}

#[test]
fn only_no_evidence_refusals_permit_the_local_route() {
    use archivefs_core::dat::amiga_adf_identity::AmigaAdfIdentityRefusal as R;
    assert!(R::NoCurrentMatch.permits_local_media());
    // Plan time and click time use this one predicate: no library record, no route.
    assert!(!R::NotInLibrary.permits_local_media());
    assert!(R::NotInLibrary.reason().contains("scan it first"));
    for blocked in [
        R::WeakEvidenceOnly,
        R::NotAdf,
        R::NotAmiga,
        R::FileChanged,
        R::Conflicting(vec!["a".into(), "b".into()]),
        R::CatalogueStale,
        R::InventoryMissing,
        R::InventoryStale,
        R::EntryNotInInventory,
        R::NotSingleRom(2),
        R::DiskNumbered("Game".into()),
        R::CompletenessUnknown,
        R::NumberedSiblings("Game (Disk 2)".into()),
        R::NotStandaloneGame("an add-on".into()),
    ] {
        assert!(!blocked.permits_local_media(), "{blocked:?}");
    }
}

#[test]
fn siblings_of_any_amiga_media_kind_or_an_oversized_folder_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("Game Disk 1.adf");
    fs::write(&main, floppy()).unwrap();
    assert!(local_adf_media(&main).is_err(), "numbered itself");
    let plain = tempfile::tempdir().unwrap();
    let one = plain.path().join("Cool Game.adf");
    fs::write(&one, floppy()).unwrap();
    assert!(local_adf_media(&one).is_ok());
    // A numbered sibling in another media format, and a symlinked one.
    fs::write(plain.path().join("Cool Game Disk 2.ipf"), b"x").unwrap();
    assert!(local_adf_media(&one).is_err(), "ipf sibling");
    fs::remove_file(plain.path().join("Cool Game Disk 2.ipf")).unwrap();
    std::os::unix::fs::symlink(&one, plain.path().join("Cool Game Disk 2.adf")).unwrap();
    assert!(local_adf_media(&one).is_err(), "symlinked sibling");
    // A folder past the scan cap refuses instead of scanning a truncation.
    let big = tempfile::tempdir().unwrap();
    let lone = big.path().join("Lone.adf");
    fs::write(&lone, floppy()).unwrap();
    for i in 0..4100 {
        fs::write(big.path().join(format!("f{i}.txt")), b"").unwrap();
    }
    assert!(local_adf_media(&lone).is_err(), "oversized folder");
}

#[test]
fn unrelated_archives_beside_the_adf_do_not_block_it() {
    let dir = tempfile::tempdir().unwrap();
    let adf = dir.path().join("Cool Game.adf");
    fs::write(&adf, floppy()).unwrap();
    fs::write(dir.path().join("Something Else.zip"), b"x").unwrap();
    fs::write(dir.path().join("Another Title.ipf"), b"x").unwrap();
    assert!(local_adf_media(&adf).is_ok());
}

#[test]
fn the_local_route_refuses_without_a_loaded_catalogue() {
    let lab = lab("amiga_model = A500", true);
    let request = local_request(&lab);
    let database = archivefs_core::Database::open_or_create(lab.database.clone()).unwrap();
    let refused = archivefs_core::launch::preflight_fsuae_adf_launch(
        &request,
        &database,
        &std::collections::HashMap::new(),
    );
    assert!(refused.is_err(), "{refused:?}");
}

#[test]
fn a_disk_two_of_another_variant_beside_the_adf_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let one = dir.path().join("Game (1991)(Pub).adf");
    let two = dir.path().join("Game (1991)(Pub)(de)(Disk 2 of 2).adf");
    fs::write(&one, floppy()).unwrap();
    fs::write(&two, floppy()).unwrap();
    assert!(local_adf_media(&one).is_err());
}

#[test]
fn a_file_that_left_the_library_is_refused_at_click_time() {
    let lab = lab("amiga_model = A500", true);
    let request = local_request(&lab);
    // A database that never saw the file: NotInLibrary at the click.
    let other = tempfile::tempdir().unwrap();
    let database =
        archivefs_core::Database::open_or_create(other.path().join("x.sqlite3")).unwrap();
    let mut catalogues = std::collections::HashMap::new();
    catalogues.insert(
        "test".to_string(),
        archivefs_core::dat::configured_catalogues::ConfiguredCatalogue {
            reference: archivefs_core::dat::catalogue_selection::CatalogueRef::Local {
                source_id: "test".into(),
                member: None,
            },
            source_id: "test".into(),
            display_name: "test".into(),
            path: lab.adf.with_extension("dat"),
            kind: archivefs_core::dat::sources::DatSourceKind::File,
            sha256: None,
            revision: None,
        },
    );
    let refused =
        archivefs_core::launch::preflight_fsuae_adf_launch(&request, &database, &catalogues);
    assert!(refused.is_err(), "{refused:?}");
}

#[test]
fn a_fifo_named_like_an_adf_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("pipe.adf");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(local_adf_media(&fifo).is_err());
}

#[test]
fn the_local_route_needs_an_amiga_library_platform() {
    let lab = lab("amiga_model = A500", true);
    let media = local_adf_media(&lab.adf).unwrap();
    let inputs = local_inputs_for(
        &media,
        "Acorn Archimedes",
        &lab.roots,
        lab.database.clone(),
        lab.database.with_extension("toml"),
    );
    assert!(inputs.requests.is_empty());
}
