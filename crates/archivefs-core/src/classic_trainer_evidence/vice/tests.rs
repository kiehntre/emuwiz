use super::*;
use crate::classic_trainer_evidence::pinning::{create_pin, preview_pin};
use crate::launch::cheat_runtime::{
    CheatRuntimeRoots, CheatRuntimeWorkspace, LiveLaunchBinding, WorkspaceDisposition,
    scan_cheat_runtime_workspaces,
};
use std::{fs, os::unix::fs::symlink};

fn cartridge() -> Vec<u8> {
    let mut bytes = vec![0; 8272];
    bytes[..16].copy_from_slice(b"C64 CARTRIDGE   ");
    bytes[16..20].copy_from_slice(&64u32.to_be_bytes());
    bytes[20..22].copy_from_slice(&0x100u16.to_be_bytes());
    bytes[25] = 1;
    bytes[64..68].copy_from_slice(b"CHIP");
    bytes[68..72].copy_from_slice(&8208u32.to_be_bytes());
    bytes[76..78].copy_from_slice(&0x8000u16.to_be_bytes());
    bytes[78..80].copy_from_slice(&8192u16.to_be_bytes());
    bytes
}
struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    game: ClassicTrainerGame,
    roots: TrustedRoots,
}
fn fixture(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.crt");
    let bytes = cartridge();
    fs::write(&path, &bytes).unwrap();
    fs::write(path.with_extension("pok"), text).unwrap();
    let game = ClassicTrainerGame {
        platform: "Commodore 64".into(),
        identities: vec![format!("sha256:{}", hex_sha256(&bytes))],
        identity_verified: true,
    };
    let roots = TrustedRoots::from_paths([dir.path()]);
    let pin = preview_pin(&game, &path, &roots).unwrap();
    create_pin(&pin, &game, &roots).unwrap();
    Fixture {
        _dir: dir,
        path,
        game,
        roots,
    }
}
fn prepared(f: &Fixture) -> Prepared {
    let old = preview(&f.game, &f.path, &f.roots).unwrap();
    prepare(&old, &f.game, &f.roots, &[0], &f.game.identities[0]).unwrap()
}

#[test]
fn c64_bundle_uses_existing_parsers_pin_projection_and_runtime_plan() {
    let f = fixture("POKE $C000,$2A\nPOKE 53247,7\n");
    let report = assess_sidecar_pok(&f.game, &f.path).unwrap().unwrap();
    assert_eq!(report.verified().count(), 1);
    assert_eq!(report.options[0].platform, PokePlatform::Commodore64);
    let material = prepared(&f);
    assert_eq!(
        String::from_utf8(material.bytes.clone()).unwrap(),
        "radix H\n> C000 2A\n> CFFF 07\nx\n"
    );
    let exe = f.path.with_file_name("x64sc");
    fs::write(&exe, b"fake").unwrap();
    let (plan, handoff) = material
        .runtime_plan(
            "selected",
            f.path.parent().unwrap(),
            &exe,
            &hex_sha256(b"fake"),
        )
        .unwrap();
    assert_eq!(plan.adapter_id(), "vice");
    assert_eq!(
        plan.extra_arguments(),
        [
            std::ffi::OsString::from("-moncommands"),
            handoff.script_path.clone().into_os_string()
        ]
    );
    let runtime = CheatRuntimeRoots {
        approved_root: f.path.parent().unwrap().into(),
    };
    let binding = LiveLaunchBinding {
        adapter_id: "vice".into(),
        game_identity: f.game.identities[0].clone(),
    };
    let (workspace, _) = CheatRuntimeWorkspace::materialise(&plan, &binding, &runtime).unwrap();
    assert_eq!(fs::read(&handoff.script_path).unwrap(), material.bytes);
    assert!(
        scan_cheat_runtime_workspaces(&runtime, u64::MAX, 0)
            .iter()
            .any(|s| s.disposition == WorkspaceDisposition::OwnedActive)
    );
    workspace.cleanup().unwrap();
    assert!(!plan.root().exists());
}

#[test]
fn guards_other_memory_overlap_and_raw_monitor_text_are_refused() {
    for text in [
        "POKE $C000,1,0",
        "POKE $0001,1",
        "POKE $0400,1",
        "POKE $8000,1",
        "POKE $D800,1",
        "POKE $D000,1",
        "POKE $E000,1",
        "POKE $C000,1\nPOKE $C000,2",
    ] {
        let f = fixture(text);
        let old = preview(&f.game, &f.path, &f.roots).unwrap();
        assert!(old.compatibility(0).is_err(), "{text}");
        assert!(prepare(&old, &f.game, &f.roots, &[0], &f.game.identities[0]).is_err());
    }
    let game = ClassicTrainerGame {
        platform: "C64".into(),
        identities: vec![],
        identity_verified: false,
    };
    for text in [
        "radix H\n> C000 FF\nx",
        "POKE 49152,1: SYS 4096",
        "N Trainer\nZ 8 49152 1 0\nY",
        "POKE $C000,256",
    ] {
        assert!(parse_sidecar_trainers(&game, text).is_err());
    }
}

#[test]
fn no_selection_unknown_duplicate_and_changed_evidence_are_refused() {
    let f = fixture("POKE $C000,1");
    let old = preview(&f.game, &f.path, &f.roots).unwrap();
    for entries in [vec![], vec![1], vec![0, 0]] {
        assert!(prepare(&old, &f.game, &f.roots, &entries, &f.game.identities[0]).is_err());
    }
    assert!(prepare(&old, &f.game, &f.roots, &[0], "other-game").is_err());
    fs::write(f.path.with_extension("pok"), "POKE $C000,2").unwrap();
    assert!(prepare(&old, &f.game, &f.roots, &[0], &f.game.identities[0]).is_err());
    let f = fixture("POKE $C000,1");
    let old = preview(&f.game, &f.path, &f.roots).unwrap();
    fs::write(
        trainer_target_path(&f.path),
        "trainer-sha256 bad\ngame-sha256 bad\n",
    )
    .unwrap();
    assert!(prepare(&old, &f.game, &f.roots, &[0], &f.game.identities[0]).is_err());
    let f = fixture("POKE $C000,1");
    let mut changed = f.game.clone();
    changed.identity_verified = false;
    assert!(preview(&changed, &f.path, &f.roots).is_err());
    assert!(preview(&f.game, &f.path, &TrustedRoots::none()).is_err());
}

#[test]
fn ambiguous_sidecars_symlinks_and_broader_media_do_not_launch() {
    let f = fixture("POKE $C000,1");
    fs::write(f.path.with_extension("POK"), "POKE $C000,2").unwrap();
    assert!(preview(&f.game, &f.path, &f.roots).is_err());
    assert!(
        assess_sidecar_pok(&f.game, &f.path)
            .unwrap()
            .unwrap()
            .options
            .is_empty()
    );
    let f = fixture("POKE $C000,1");
    let path = f.path.with_extension("pok");
    fs::remove_file(&path).unwrap();
    symlink(&f.path, &path).unwrap();
    assert!(preview(&f.game, &f.path, &f.roots).is_err());
    let f = fixture("POKE $C000,1");
    let other = f.path.with_extension("t64");
    fs::copy(&f.path, &other).unwrap();
    assert!(preview(&f.game, &other, &f.roots).is_err());
}

#[test]
fn imported_projection_text_never_enters_monitor_script() {
    let mut projection = project_vice_c64_pokes(
        &[CheatOperation::Write8 {
            address: 0xc000,
            value: 7,
        }],
        &ViceCheatIdentity::VerifiedGame("verified".into()),
    );
    projection.script = "quit\nload evil\n".into();
    projection.commands[0].text = "arbitrary".into();
    assert_eq!(
        ViceMonitorScript::render(&projection, "verified").unwrap(),
        b"radix H\n> C000 07\nx\n"
    );
}

#[test]
fn abandoned_vice_material_uses_the_existing_stale_workspace_recovery() {
    use crate::launch::cheat_runtime::{
        WORKSPACE_MARKER_NAME, WorkspaceMarker, cleanup_stale_workspace,
    };
    let f = fixture("POKE $C000,7");
    let material = prepared(&f);
    let exe = f.path.with_file_name("x64sc");
    fs::write(&exe, b"fake").unwrap();
    let (plan, _) = material
        .runtime_plan(
            "abandoned-vice",
            f.path.parent().unwrap(),
            &exe,
            &hex_sha256(b"fake"),
        )
        .unwrap();
    let roots = CheatRuntimeRoots {
        approved_root: f.path.parent().unwrap().into(),
    };
    let binding = LiveLaunchBinding {
        adapter_id: "vice".into(),
        game_identity: f.game.identities[0].clone(),
    };
    let (workspace, _) = CheatRuntimeWorkspace::materialise(&plan, &binding, &roots).unwrap();
    drop(workspace);
    // Only this test-owned marker is changed to model a GUI owner that died
    // before attempting launch. The public recovery path rechecks it itself.
    let marker_path = plan.root().join(WORKSPACE_MARKER_NAME);
    let mut marker: WorkspaceMarker =
        serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
    marker.owner_pid = u32::MAX;
    marker.owner_start_ticks = None;
    marker.created_unix = 0;
    fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
    let entries = scan_cheat_runtime_workspaces(&roots, u64::MAX, 0);
    assert!(
        entries
            .iter()
            .any(|e| e.path == plan.root() && e.disposition == WorkspaceDisposition::OwnedStale)
    );
    cleanup_stale_workspace(&roots, plan.root(), u64::MAX, 0).unwrap();
    assert!(!plan.root().exists());
    assert!(f.path.is_file());
    assert!(f.path.with_extension("pok").is_file());
}
