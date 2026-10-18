use super::*;
use archivefs_core::launch::*;
use std::collections::BTreeMap;
use tempfile::tempdir;
#[cfg(unix)]
#[test]
fn valid_fds_surfaces_bios_requirement_but_never_native_authority() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempdir().unwrap();
    let profile = d.path().join("config/Mesen2");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("settings.json"), "{}").unwrap();
    let exe = d.path().join("Mesen");
    std::fs::write(&exe, b"test executable - never run").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let fds = d.path().join("game.fds");
    let mut side = vec![0; 65500];
    side[0] = 1;
    side[1..15].copy_from_slice(b"*NINTENDO-HVC*");
    side[0x10..0x14].copy_from_slice(b"TST ");
    side[56] = 2;
    side[57] = 1;
    side[58] = 3;
    side[60] = 1;
    side[61..69].copy_from_slice(b"TEST    ");
    side[69..71].copy_from_slice(&0x8000_u16.to_le_bytes());
    side[71..73].copy_from_slice(&4_u16.to_le_bytes());
    side[75] = 4;
    std::fs::write(&fds, side).unwrap();
    assert!(mesen_nes::validate_mesen_nes_media(&fds).is_ok());
    let identity = canonical_identity_from_game_report(
        &archivefs_core::game_identity::inspect_catalogued_game_identity(&fds, Some("NES")),
    )
    .0;
    let selection = Selection {
        path: fds.clone(),
        identity: identity.clone(),
    };
    let roots = MesenProfileDiscoveryRoots {
        home: d.path().into(),
        xdg_config_home: d.path().join("config"),
        explicit_configuration_roots: vec![profile.clone()],
        explicit_executables: vec![exe],
        known_version_outputs: BTreeMap::new(),
    };
    let snapshot = gather(&selection, roots.clone());
    let inputs = snapshot.inputs().collect::<Vec<_>>();
    assert!(
        inputs
            .iter()
            .any(|p| p.firmware == FirmwareReadiness::Missing)
    );
    let content = LaunchContentRef {
        kind: None,
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(fds),
        requires_mount: false,
        provenance: "synthetic".into(),
    };
    let mut plan = build_launch_plan(
        &identity,
        &content,
        &inputs,
        &archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![],
            diagnostics: vec![],
        },
        &[],
    );
    snapshot.apply(&mut plan);
    assert!(!plan.candidates.is_empty());
    assert!(
        plan.candidates
            .iter()
            .all(|c| c.readiness == LaunchReadiness::Blocked)
    );
    assert!(plan.candidates.iter().any(|c| {
        c.blockers
            .iter()
            .any(|b| b.detail.contains("typed settings"))
    }));
    let input = crate::launch_readiness_page::LaunchReadinessInput::Plan {
        plan: plan.clone(),
        retroarch: None,
        retroarch_scanned: false,
        standalone_scans_complete: true,
        dolphin: None,
        pcsx2: None,
        duckstation: None,
        ppsspp: None,
        rpcs3: None,
        xemu: None,
        xenia: None,
        cemu: None,
        amiga_whdload: None,
        #[cfg(target_os = "linux")]
        shadps4: None,
    };
    let crate::launch_readiness_page::GamerPlayAction::BlockedTyped(blocker) =
        crate::launch_readiness_page::gamer_play_action(&input)
    else {
        panic!("native launch must stay disabled");
    };
    assert_eq!(blocker.emulator.as_deref(), Some("Mesen"));
    assert!(blocker.detail.contains("typed settings"));
    std::fs::create_dir(profile.join("Firmware")).unwrap();
    std::fs::write(profile.join("Firmware/disksys.rom"), vec![0; 8192]).unwrap();
    let snapshot = gather(&selection, roots);
    assert!(snapshot.inputs().all(|p| !p.eligible));
    assert!(
        snapshot
            .inputs()
            .any(|p| p.firmware == FirmwareReadiness::PresentUnverified)
    );
}
#[test]
fn unavailable_is_honest_and_blocked() {
    let snapshot = Snapshot::unavailable("No native Mesen installation");
    let input = snapshot.inputs().next().unwrap();
    assert!(!input.eligible);
    assert_eq!(input.firmware, FirmwareReadiness::Unknown);
}
