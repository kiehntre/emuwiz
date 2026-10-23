use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use AmigaCheckKind as K;
use AmigaCheckState::{Blocked, Ready, Warning};

fn profile(emulator: AmigaEmulatorKind, model: Option<&str>) -> AmigaProfileFacts {
    AmigaProfileFacts {
        emulator,
        profile_id: format!("{emulator:?}"),
        eligible: true,
        executable_found: true,
        kickstart: AmigaKickstartState::PresentUnverified,
        machine_model: model.map(str::to_owned),
        chipset: None,
        whdload_support_present: true,
    }
}

fn env(profiles: Vec<AmigaProfileFacts>) -> AmigaEnvironmentFacts {
    AmigaEnvironmentFacts {
        profiles,
        discovery_available: true,
        whdload: None,
    }
}

fn slave(aga: bool) -> WhdloadPackageFacts {
    WhdloadPackageFacts::Slaves(vec![WhdloadSlaveFacts {
        member_path: "Game/Game.Slave".into(),
        requires_aga: aga,
        requires_68020: aga,
    }])
}

fn state(readiness: &AmigaGameReadiness, kind: AmigaCheckKind) -> AmigaCheckState {
    readiness
        .check(kind)
        .unwrap_or_else(|| panic!("{kind:?} row"))
        .state
}

const AMIGA: AmigaCdMachine = AmigaCdMachine::OrdinaryAmiga;

#[test]
fn formats_are_distinct_and_platform_decides_the_machine() {
    let kinds: Vec<_> = [
        "a.adf", "a.adz", "a.dms", "a.ipf", "a.hfe", "a.hdf", "a.lha", "a.cue", "a.zip",
    ]
    .iter()
    .map(|name| AmigaMediaKind::from_path(Path::new(name)))
    .collect();
    let mut unique = kinds.clone();
    unique.dedup();
    assert_eq!(unique.len(), kinds.len());
    assert_eq!(machine_for_platform("Amiga"), Some(AMIGA));
    assert_eq!(
        machine_for_platform("AmigaCD32"),
        Some(AmigaCdMachine::Cd32)
    );
    assert_eq!(
        machine_for_platform("Commodore CDTV"),
        Some(AmigaCdMachine::Cdtv)
    );
    assert_eq!(machine_for_platform("Atari ST"), None);
}

#[test]
fn ready_whdload_package_routes_to_the_existing_launch_card_only() {
    let mut facts = env(vec![profile(AmigaEmulatorKind::FsUae, Some("A1200"))]);
    facts.whdload = Some(slave(true));
    let readiness = assess_amiga_game(AMIGA, Path::new("/g/Game.lha"), false, None, Some(&facts));
    for kind in [
        K::Media,
        K::Emulator,
        K::Machine,
        K::Kickstart,
        K::Whdload,
        K::Play,
    ] {
        assert_eq!(state(&readiness, kind), Ready, "{kind:?}");
    }
    assert!(readiness.launch_route_exists);
    // Trainer choices are never described as active at launch.
    let trainers = readiness.check(K::Trainers).unwrap();
    assert!(trainers.detail.contains("not passed"));
    assert_eq!(trainers.route, Some(AmigaFixRoute::Trainers));
}

#[test]
fn direct_floppy_images_never_get_a_launch_route() {
    let facts = env(vec![profile(AmigaEmulatorKind::FsUae, Some("A500"))]);
    for name in ["a.adf", "a.adz", "a.dms", "a.ipf", "a.hdf"] {
        let readiness = assess_amiga_game(AMIGA, Path::new(name), false, None, Some(&facts));
        assert!(!readiness.launch_route_exists, "{name}");
        let play = readiness.check(K::Play).unwrap();
        assert_eq!(play.state, Blocked, "{name}");
        assert!(play.detail.contains("Only WHDLoad packages"), "{name}");
    }
}

#[test]
fn wrong_machine_is_blocked_with_the_configured_model_named() {
    // A CD32-configured emulator cannot run an ordinary Amiga floppy game.
    let facts = env(vec![profile(AmigaEmulatorKind::Amiberry, Some("CD32"))]);
    let readiness = assess_amiga_game(AMIGA, Path::new("a.adf"), false, None, Some(&facts));
    let machine = readiness.check(K::Machine).unwrap();
    assert_eq!(machine.state, Blocked);
    assert!(machine.detail.contains("Wrong machine"));
    assert!(machine.detail.contains("CD32"));

    // An AGA Slave on an A500 profile is also the wrong machine.
    let mut facts = env(vec![profile(AmigaEmulatorKind::FsUae, Some("A500"))]);
    facts.whdload = Some(slave(true));
    let readiness = assess_amiga_game(AMIGA, Path::new("a.lha"), false, None, Some(&facts));
    assert_eq!(state(&readiness, K::Machine), Blocked);
    assert!(!readiness.launch_route_exists);
}

#[test]
fn an_unnamed_model_is_a_warning_not_a_claim() {
    let facts = env(vec![profile(AmigaEmulatorKind::FsUae, None)]);
    let readiness = assess_amiga_game(AMIGA, Path::new("a.adf"), false, None, Some(&facts));
    assert_eq!(state(&readiness, K::Machine), Warning);
}

#[test]
fn missing_or_unconfigured_kickstart_blocks_and_links_to_firmware() {
    for kickstart in [
        AmigaKickstartState::Missing,
        AmigaKickstartState::NotConfigured,
        AmigaKickstartState::Unreadable,
    ] {
        let mut fsuae = profile(AmigaEmulatorKind::FsUae, Some("A1200"));
        fsuae.kickstart = kickstart;
        let mut facts = env(vec![fsuae]);
        facts.whdload = Some(slave(false));
        let readiness = assess_amiga_game(AMIGA, Path::new("a.lha"), false, None, Some(&facts));
        let row = readiness.check(K::Kickstart).unwrap();
        assert_eq!(row.state, Blocked, "{kickstart:?}");
        assert_eq!(row.route, Some(AmigaFixRoute::Firmware));
        assert_eq!(state(&readiness, K::Play), Blocked);
        assert!(!readiness.launch_route_exists);
    }
}

#[test]
fn no_emulator_routes_to_setup_and_skips_machine_rows() {
    let readiness = assess_amiga_game(AMIGA, Path::new("a.adf"), false, None, Some(&env(vec![])));
    let row = readiness.check(K::Emulator).unwrap();
    assert_eq!(row.state, Blocked);
    assert_eq!(row.route, Some(AmigaFixRoute::EmulatorSetup));
    assert!(readiness.check(K::Machine).is_none());

    let mut missing_binary = profile(AmigaEmulatorKind::FsUae, Some("A500"));
    missing_binary.executable_found = false;
    let readiness = assess_amiga_game(
        AMIGA,
        Path::new("a.adf"),
        false,
        None,
        Some(&env(vec![missing_binary])),
    );
    assert_eq!(state(&readiness, K::Emulator), Blocked);
}

#[test]
fn incomplete_or_unverified_disk_sets_are_reported_honestly() {
    let facts = env(vec![profile(AmigaEmulatorKind::FsUae, Some("A500"))]);
    let incomplete = AmigaDiskSetFacts {
        count: 3,
        order_verified: false,
        problems: 1,
    };
    let readiness = assess_amiga_game(
        AMIGA,
        Path::new("d1.adf"),
        false,
        Some(incomplete),
        Some(&facts),
    );
    let row = readiness.check(K::Disks).unwrap();
    assert_eq!(row.state, Blocked);
    assert_eq!(row.route, Some(AmigaFixRoute::MultiDisk));
    assert_eq!(readiness.disk_count, Some(3));

    let unverified = AmigaDiskSetFacts {
        problems: 0,
        ..incomplete
    };
    let readiness = assess_amiga_game(
        AMIGA,
        Path::new("d1.adf"),
        false,
        Some(unverified),
        Some(&facts),
    );
    assert_eq!(state(&readiness, K::Disks), Warning);

    let verified = AmigaDiskSetFacts {
        order_verified: true,
        ..unverified
    };
    let readiness = assess_amiga_game(
        AMIGA,
        Path::new("d1.adf"),
        false,
        Some(verified),
        Some(&facts),
    );
    let row = readiness.check(K::Disks).unwrap();
    assert_eq!(row.state, Ready);
    assert!(row.detail.contains("does not swap disks"));
    // A verified set is still not a launch route for direct floppies.
    assert!(!readiness.launch_route_exists);
}

#[test]
fn unsupported_damaged_and_packed_media_are_blocked() {
    let facts = env(vec![profile(AmigaEmulatorKind::FsUae, Some("A500"))]);
    for name in ["a.hfe", "a.zip", "a.xyz"] {
        let readiness = assess_amiga_game(AMIGA, Path::new(name), false, None, Some(&facts));
        assert_eq!(state(&readiness, K::Media), Blocked, "{name}");
    }
    let damaged = assess_amiga_game(AMIGA, Path::new("a.adf"), true, None, Some(&facts));
    assert_eq!(state(&damaged, K::Media), Blocked);

    let mut facts = facts;
    facts.whdload = Some(WhdloadPackageFacts::Unreadable("encrypted".into()));
    let readiness = assess_amiga_game(AMIGA, Path::new("a.lha"), false, None, Some(&facts));
    assert_eq!(state(&readiness, K::Whdload), Blocked);
    assert!(!readiness.launch_route_exists);

    facts.whdload = Some(WhdloadPackageFacts::Slaves(vec![
        WhdloadSlaveFacts {
            member_path: "A.Slave".into(),
            requires_aga: false,
            requires_68020: false,
        },
        WhdloadSlaveFacts {
            member_path: "B.Slave".into(),
            requires_aga: false,
            requires_68020: false,
        },
    ]));
    let readiness = assess_amiga_game(AMIGA, Path::new("a.lha"), false, None, Some(&facts));
    assert_eq!(state(&readiness, K::Whdload), Blocked);
}

#[test]
fn cd32_and_cdtv_media_are_never_floppies() {
    let cd32 = AmigaCdMachine::Cd32;
    let facts = env(vec![profile(AmigaEmulatorKind::Amiberry, Some("CD32"))]);
    let readiness = assess_amiga_game(cd32, Path::new("g.cue"), false, None, Some(&facts));
    assert_eq!(state(&readiness, K::Media), Ready);
    assert_eq!(state(&readiness, K::Machine), Ready);
    // The extended ROM is not checked here, so Kickstart is not fully ready.
    assert_eq!(state(&readiness, K::Kickstart), Warning);
    assert!(readiness.check(K::Disks).is_none());
    assert!(!readiness.launch_route_exists);
    assert_eq!(state(&readiness, K::Play), Blocked);

    let readiness = assess_amiga_game(cd32, Path::new("g.adf"), false, None, Some(&facts));
    assert_eq!(state(&readiness, K::Media), Blocked);
    let readiness = assess_amiga_game(AMIGA, Path::new("g.iso"), false, None, Some(&facts));
    assert_eq!(state(&readiness, K::Media), Blocked);
    let readiness = assess_amiga_game(
        AmigaCdMachine::Cdtv,
        Path::new("g.iso"),
        false,
        None,
        Some(&facts),
    );
    assert_eq!(state(&readiness, K::Machine), Blocked);
}

#[test]
fn rows_needing_the_environment_wait_for_it() {
    let readiness = assess_amiga_game(AMIGA, Path::new("a.lha"), false, None, None);
    assert!(readiness.check(K::Emulator).is_none());
    assert!(readiness.check(K::Kickstart).is_none());
    assert_eq!(state(&readiness, K::Whdload), Warning);
}

#[test]
fn observing_reads_profile_files_only_and_changes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let root = config.join("amiberry");
    fs::create_dir_all(&root).unwrap();
    let conf = "amiga_model=A1200\nchipset=AGA\nkickstart_rom_file=kick31.rom\n";
    fs::write(root.join("amiberry.conf"), conf).unwrap();
    fs::write(root.join("kick31.rom"), b"not a verified ROM").unwrap();
    let game = temp.path().join("Game.adf");
    fs::write(&game, vec![0u8; 901_120]).unwrap();
    let roots = AmigaProfileDiscoveryRoots {
        home: temp.path().join("home"),
        xdg_config_home: config,
        xdg_data_home: temp.path().join("data"),
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
    };
    let before = snapshot(temp.path());
    let facts = observe_amiga_environment(Some(&roots), &game, &AtomicBool::new(false));
    assert_eq!(snapshot(temp.path()), before);
    assert!(facts.discovery_available);
    assert!(facts.whdload.is_none());
    let amiberry = facts
        .profiles
        .iter()
        .find(|profile| profile.emulator == AmigaEmulatorKind::Amiberry)
        .expect("Amiberry profile");
    assert_eq!(amiberry.machine(), Some(AMIGA));
    assert_eq!(amiberry.aga(), Some(true));
    assert_eq!(amiberry.kickstart, AmigaKickstartState::PresentUnverified);

    let none = observe_amiga_environment(None, &game, &AtomicBool::new(false));
    let readiness = assess_amiga_game(AMIGA, &game, false, None, Some(&none));
    assert_eq!(state(&readiness, K::Emulator), Blocked);
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}
