//! RetroArch adapter tests. Real preflight, real discovery, real planner and a
//! real spawn of a fake `retroarch` script that records what it was handed.
//! The real RetroArch install and the real user config are never involved.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::emulator_environment::HostReadOnlyFilesystem;
use crate::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use crate::game_identity::inspect_catalogued_game_identity;
use crate::launch::cheat_launch_plan::{CheatLaunchFormat, CheatVariant};
use crate::launch::cheat_runtime::{CheatRuntimeStage, CleanupOutcome, RuntimeEffect};
use crate::patch_manager::{
    CheatApplicabilityInput, CheatApplicabilityState, CheatParseEvidence, CheatPlatform,
    CheatSourceReference, assess_cheat_applicability, parse_cht_text,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn unique(label: &str) -> String {
    format!(
        "{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

struct World {
    root: PathBuf,
    request: RetroArchLaunchRequest,
    environment: DiscoveryEnvironment,
}

impl World {
    fn new(label: &str, with_save_dirs: bool) -> Self {
        let root = std::env::temp_dir().join(format!("emuwiz-ra-runtime-{}", unique(label)));
        let _ = fs::remove_dir_all(&root);
        let write = |rel: &str, bytes: &[u8]| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, bytes).unwrap();
            p
        };
        // The fake emulator copies the scratch profile it was pointed at, and
        // its own argv, into a record directory, then exits.
        let script = write(
            "bin/retroarch",
            format!(
                "#!/bin/sh\nrec=\"{rec}\"\nmkdir -p \"$rec\"\nprintf '%s\\n' \"$@\" > \"$rec/args.txt\"\n\
                 while [ $# -gt 0 ]; do if [ \"$1\" = \"--config\" ]; then cp -r \"$(dirname \"$(dirname \"$2\")\")\" \"$rec/profile\"; fi; shift; done\nexit 0\n",
                rec = root.join("record").display()
            )
            .as_bytes(),
        );
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(root.join("saves")).unwrap();
        fs::create_dir_all(root.join("states")).unwrap();
        let mut cfg = format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\nvideo_driver = \"vulkan\"\ninput_player1_a = \"x\"\ncheat_database_path = \"/user/own/cheats\"\nconfig_save_on_exit = \"true\"\n",
            root.join("cores").display(),
            root.join("info").display()
        );
        if with_save_dirs {
            cfg.push_str(&format!(
                "savefile_directory = \"{}\"\nsavestate_directory = \"{}\"\n",
                root.join("saves").display(),
                root.join("states").display()
            ));
        }
        write("config/retroarch/retroarch.cfg", cfg.as_bytes());
        write("cores/genesis_plus_gx_libretro.so", b"stub core");
        write("info/genesis_plus_gx.info", b"systemname = \"megadrive\"\n");
        let content = write("content/game.md", b"synthetic mega drive rom bytes");
        let key = inspect_catalogued_game_identity(&content, Some("MegaDrive"))
            .verified_loose_rom_sha256()
            .expect("verifiable loose rom")
            .to_string();
        let request = RetroArchLaunchRequest {
            selected_content_path: content,
            expected_platform_id: "MegaDrive".into(),
            expected_game_key: key,
            profile: ProfileRef {
                profile_kind: ProfileKind::Native,
                scope: ProfileScope::User,
            },
            core_stem: "genesis_plus_gx".into(),
            expected_appimage_executable: None,
        };
        let environment = DiscoveryEnvironment {
            home: Some(root.clone().into_os_string()),
            xdg_config_home: Some(root.join("config").into_os_string()),
            path: Some(root.join("bin").into_os_string()),
            user_flatpak_root: root.join("user-flatpak"),
            system_flatpak_root: root.join("system-flatpak"),
            app_image_search_roots: Vec::new(),
            desktop_file_roots: Vec::new(),
        };
        Self {
            root,
            request,
            environment,
        }
    }

    fn real_config(&self) -> PathBuf {
        self.root.join("config/retroarch/retroarch.cfg")
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn selection() -> RetroArchCheatSelection {
    let parsed = parse_cht_text(
        "cheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\ncheat0_enable = false\n\
         cheat1_desc = \"Max Ammo\"\ncheat1_code = \"8000BB00\"\ncheat1_enable = false\n",
    )
    .unwrap();
    let mut report = assess_cheat_applicability(&CheatApplicabilityInput {
        game: Default::default(),
        association: Default::default(),
        document: parsed.reconciliation_entries(
            "game",
            true,
            CheatPlatform::GameCube,
            "local",
            "a.cht",
        )[0]
        .document
        .clone(),
        parsing: CheatParseEvidence::Valid,
        native_cht: Some(parsed.entries[0].clone()),
        route: None,
        reconciliation: None,
    });
    report.state = CheatApplicabilityState::ExactGameMatch;
    report.blockers.clear();
    let candidate = |id: &str, entry: usize| CheatCandidate {
        logical_id: id.into(),
        title: id.into(),
        unresolved_conflict: false,
        variants: vec![CheatVariant {
            variant_id: "v1".into(),
            source: CheatSourceReference {
                provider: "local".into(),
                source_id: format!("{id}.cht"),
                source_path: None,
                source_sha256: None,
                entry_index: None,
            },
            format: CheatLaunchFormat::RetroArchCht,
            applicability: report.clone(),
            entry: Some(parsed.entries[entry].clone()),
        }],
    };
    RetroArchCheatSelection {
        candidates: vec![candidate("lives", 0), candidate("ammo", 1)],
        selections: vec![CheatLaunchSelection {
            logical_id: "lives".into(),
            variant_id: None,
            review_acknowledged: false,
        }],
        core_library_name: "Genesis Plus GX".into(),
        effective_overrides: vec![],
    }
}

fn launch(
    world: &World,
    cheats: &RetroArchCheatSelection,
    id: &str,
) -> Result<RetroArchCheatLaunch, RetroArchCheatLaunchError> {
    preflight_and_launch_retroarch_with_cheats(
        &world.request,
        &HostReadOnlyFilesystem,
        &world.environment,
        cheats,
        &CheatRuntimeRoots::default(),
        id,
    )
}

fn workspace(id: &str) -> PathBuf {
    CheatRuntimeRoots::default()
        .approved_root
        .join(workspace_name(id))
}

fn wait_finished(launch: &mut RetroArchCheatLaunch) {
    let RetroArchCheatLaunch::WithCheats { session, .. } = launch else {
        panic!("no session")
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while !session.is_finished() {
        session.poll();
        assert!(Instant::now() < deadline, "fake emulator never finished");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn no_selection_is_the_plain_canonical_launch_with_no_runtime_material() {
    let world = World::new("plain", true);
    let id = unique("plain");
    let mut cheats = selection();
    cheats.selections.clear();
    match launch(&world, &cheats, &id).unwrap() {
        RetroArchCheatLaunch::Plain(mut process) => {
            let deadline = Instant::now() + Duration::from_secs(20);
            while process.poll().is_none() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        RetroArchCheatLaunch::WithCheats { .. } => panic!("no cheats were selected"),
    }
    assert!(!workspace(&id).exists());
    let args = fs::read_to_string(world.root.join("record/args.txt")).unwrap();
    assert!(!args.contains("--config"), "{args}");
}

#[test]
fn a_selected_cheat_launches_retroarch_with_a_prepared_scratch_config_and_cleans_up() {
    let world = World::new("happy", true);
    let id = unique("happy");
    let real_before = fs::read(world.real_config()).unwrap();
    let rom_before = fs::read(&world.request.selected_content_path).unwrap();
    let mut launched = launch(&world, &selection(), &id).unwrap();
    wait_finished(&mut launched);
    let receipt = receipt_of(&launched).unwrap().clone();

    // The canonical argv comes first; the prepared config is appended.
    let args = fs::read_to_string(world.root.join("record/args.txt")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert_eq!(lines[0], "-L");
    let at = lines
        .iter()
        .position(|l| *l == "--config")
        .expect("--config handed over");
    assert!(lines[at + 1].contains(&workspace_name(&id)), "{args}");

    // What RetroArch was pointed at: the user's settings, the generated pins,
    // and only the selected cheat.
    let profile = world.root.join("record/profile");
    let cfg = fs::read_to_string(profile.join("profile/retroarch.cfg")).unwrap();
    assert!(
        cfg.contains("video_driver = \"vulkan\""),
        "user settings are inherited"
    );
    assert!(cfg.contains("input_player1_a = \"x\""));
    assert!(cfg.contains("config_save_on_exit = \"false\""));
    assert!(!cfg.contains("config_save_on_exit = \"true\""));
    assert!(
        !cfg.contains("/user/own/cheats"),
        "the user's cheat path is replaced"
    );
    assert!(cfg.contains(&world.root.join("saves").display().to_string()));
    let mut cht = String::new();
    fn collect(dir: &std::path::Path, out: &mut String) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(&p, out)
            } else if p.extension().is_some_and(|x| x == "cht") {
                out.push_str(&fs::read_to_string(p).unwrap());
            }
        }
    }
    collect(&profile, &mut cht);
    assert!(
        cht.contains("Infinite Lives") && !cht.contains("Max Ammo"),
        "{cht}"
    );

    // Truthful receipt: handed over, effect unknown, cleaned up.
    assert_eq!(receipt.runtime_effect, RuntimeEffect::Unknown);
    assert!(
        receipt
            .stages
            .contains(&CheatRuntimeStage::PassedToEmulator)
    );
    assert!(receipt.stages.contains(&CheatRuntimeStage::ProcessStarted));
    assert!(
        receipt
            .stages
            .contains(&CheatRuntimeStage::RuntimeEffectUnknown)
    );
    assert_eq!(receipt.cleanup, CleanupOutcome::Completed);
    assert!(!workspace(&id).exists());
    assert!(!receipt.has_violation());

    // Nothing of the user's was written.
    assert_eq!(fs::read(world.real_config()).unwrap(), real_before);
    assert_eq!(
        fs::read(&world.request.selected_content_path).unwrap(),
        rom_before
    );
    assert_eq!(fs::read_dir(world.root.join("saves")).unwrap().count(), 0);
    assert_eq!(fs::read_dir(world.root.join("states")).unwrap().count(), 0);
}

#[test]
fn unresolved_save_directories_fail_closed_before_anything_is_created() {
    let world = World::new("nosaves", false);
    let id = unique("nosaves");
    match launch(&world, &selection(), &id) {
        Err(RetroArchCheatLaunchError::Facts(why)) => {
            assert!(why.contains("not resolved"), "{why}")
        }
        other => panic!("{:?}", other.err()),
    }
    assert!(!workspace(&id).exists());
    assert!(!world.root.join("record").exists(), "nothing was started");
}

#[test]
fn the_canonical_preflight_still_gates_cheat_launches() {
    let mut world = World::new("gate", true);
    world.request.expected_game_key = "0".repeat(64);
    let id = unique("gate");
    assert!(matches!(
        launch(&world, &selection(), &id),
        Err(RetroArchCheatLaunchError::Preflight(_))
    ));
    assert!(!workspace(&id).exists());
    assert!(!world.root.join("record").exists());
}

#[test]
fn a_selection_the_planner_blocks_creates_and_starts_nothing() {
    let world = World::new("blocked", true);
    let id = unique("blocked");
    let mut cheats = selection();
    cheats.selections[0].logical_id = "does-not-exist".into();
    assert!(matches!(
        launch(&world, &cheats, &id),
        Err(RetroArchCheatLaunchError::Cheats(_))
    ));
    assert!(!workspace(&id).exists());
    assert!(!world.root.join("record").exists());
}

#[test]
fn an_unusable_launch_id_is_refused() {
    let world = World::new("badid", true);
    assert!(launch(&world, &selection(), "../escape").is_err());
    assert!(!world.root.join("record").exists());
}

#[test]
fn the_scratch_config_keeps_user_settings_but_the_launch_owns_its_keys() {
    let seed = "# comment\n#include \"/elsewhere.cfg\"\nvideo_driver = \"gl\"\n\
                config_save_on_exit = \"true\"\ncheat_database_path = \"/user/cheats\"\n\
                savefile_directory = \"/user/saves\"\napply_cheats_after_load = \"false\"\n";
    let generated = "apply_cheats_after_load = \"true\"\ncheat_database_path = \"/scratch\"\n";
    let (text, kept) = compose_base_config(Some(seed), generated);
    assert!(text.contains("video_driver = \"gl\""));
    assert!(!text.contains("#include") && !text.contains("/elsewhere.cfg"));
    assert!(!text.contains("/user/cheats") && !text.contains("\"false\""));
    assert!(
        text.trim_end()
            .ends_with("cheat_database_path = \"/scratch\"")
    );
    assert_eq!(
        kept, 1,
        "only video_driver survives; the rest is owned by the launch"
    );
    // Deterministic, and no seed is fine.
    assert_eq!(
        compose_base_config(Some(seed), generated),
        compose_base_config(Some(seed), generated)
    );
    assert_eq!(compose_base_config(None, generated).1, 0);
}
