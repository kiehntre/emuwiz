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
        Self::new_in(
            &std::env::temp_dir(),
            label,
            with_save_dirs,
            b"synthetic mega drive rom bytes",
        )
    }

    /// `base` lets a real-RetroArch probe place the whole fixture somewhere a
    /// Flatpak can see (the host `/tmp` is private to it).
    fn new_in(base: &std::path::Path, label: &str, with_save_dirs: bool, rom: &[u8]) -> Self {
        let root = base.join(format!("emuwiz-ra-runtime-{}", unique(label)));
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
        let content = write("content/game.md", rom);
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

// ---- argv ordering (verified against real RetroArch 1.22.2) ---------------

#[test]
fn the_prepared_config_is_the_final_pair_after_the_content_argument() {
    let world = World::new("argv-order", true);
    let id = unique("argv-order");
    let mut launched = launch(&world, &selection(), &id).unwrap();
    wait_finished(&mut launched);
    let args = fs::read_to_string(world.root.join("record/args.txt")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    let content = world.request.selected_content_path.display().to_string();
    let content_at = lines
        .iter()
        .position(|l| *l == content)
        .expect("content argument");
    assert_eq!(lines[0], "-L");
    assert_eq!(lines[content_at + 1], "--config");
    assert_eq!(
        lines.len(),
        content_at + 3,
        "nothing follows the config: {args}"
    );
}

// ---- Flatpak: every resource the spawned process opens --------------------

const APP: &str = "org.libretro.RetroArch";

/// A world whose fake `retroarch` is classified as `flatpak run <app>` (the
/// real flatpak is never run) and a Flatpak layout rooted inside the world, so
/// "home" is the world itself and `/tmp` reservations do not apply to it.
struct FlatpakWorld {
    world: World,
    host: FlatpakHost,
    roots: CheatRuntimeRoots,
}

impl FlatpakWorld {
    fn new(label: &str, sandbox_line: &str) -> Self {
        let world = World::new(label, true);
        let script = world.root.join("bin/retroarch");
        let body = fs::read_to_string(&script).unwrap();
        let body = body.replacen("#!/bin/sh\n", &format!("#!/bin/sh\n# {sandbox_line}\n"), 1);
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let home = world.root.clone();
        let host = FlatpakHost {
            xdg_data_home: home.join(".local/share"),
            xdg_cache_home: home.join(".cache"),
            xdg_config_home: home.join(".config"),
            system_flatpak_root: home.join("system-flatpak"),
            home,
        };
        // The planner only accepts EmuWiz's own approved roots, so the workspace
        // root is the real EmuWiz data launch root (unique ids, cleaned up).
        let roots = CheatRuntimeRoots {
            approved_root: approved_retroarch_data_launch_root().expect("EmuWiz data dir"),
        };
        Self { world, host, roots }
    }

    /// Grants the workspace root plus `extra` (a `;`-separated list).
    fn grant_data(&self, extra: &str) {
        let data = self.roots.approved_root.display();
        self.grant(&format!("{data};{extra}"));
    }

    fn grant(&self, filesystems: &str) {
        let dir = self
            .host
            .xdg_data_home
            .join("flatpak/app")
            .join(APP)
            .join("current/active");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("metadata"),
            format!("[Application]\nname={APP}\n\n[Context]\nfilesystems={filesystems};\n"),
        )
        .unwrap();
    }

    fn launch(&self, id: &str) -> Result<RetroArchCheatLaunch, RetroArchCheatLaunchError> {
        let command = preflight_retroarch_launch(
            &self.world.request,
            &HostReadOnlyFilesystem,
            &self.world.environment,
        )
        .unwrap();
        launch_prepared_command(
            &self.world.request,
            command,
            &HostReadOnlyFilesystem,
            &self.world.environment,
            &selection(),
            &self.roots,
            id,
            Some(&self.host),
        )
    }

    fn workspace(&self, id: &str) -> PathBuf {
        self.roots.approved_root.join(workspace_name(id))
    }
}

const FLATPAK_RUN: &str = "exec flatpak run org.libretro.RetroArch \"$@\"";

#[test]
fn a_flatpak_cheat_launch_proceeds_only_when_every_resource_is_proven_reachable() {
    let f = FlatpakWorld::new("fp-ok", FLATPAK_RUN);
    f.grant_data("home");
    let id = unique("fp-ok");
    let mut launched = f.launch(&id).expect("every resource is reachable");
    wait_finished(&mut launched);
    let receipt = receipt_of(&launched).unwrap();
    // The workspace was the EmuWiz data root, not the host /tmp.
    assert!(
        receipt
            .workspace_root
            .as_ref()
            .unwrap()
            .starts_with(&f.roots.approved_root)
    );
    assert!(
        !receipt
            .workspace_root
            .as_ref()
            .unwrap()
            .starts_with(std::env::temp_dir().join("emuwiz"))
    );
    assert_eq!(receipt.cleanup, CleanupOutcome::Completed);
    assert!(!f.workspace(&id).exists());
}

#[test]
fn a_flatpak_cheat_launch_names_the_first_resource_it_cannot_prove() {
    let f = FlatpakWorld::new("fp-each", FLATPAK_RUN);
    let r = f.world.root.display().to_string();
    let steps: [(&str, &'static str); 4] = [
        ("", "game content"),
        ("~/content:ro", "emulator core"),
        ("~/content:ro;~/cores:ro", "save directory"),
        ("~/content:ro;~/cores:ro;~/saves", "save-state directory"),
    ];
    for (grants, expected) in steps {
        f.grant_data(grants);
        let id = unique("fp-each");
        match f.launch(&id) {
            Err(RetroArchCheatLaunchError::Sandbox(RetroArchRootError::ResourceNotVisible {
                resource,
                path,
            })) => {
                assert_eq!(resource, expected, "{grants}");
                assert!(path.starts_with(&r), "{path:?}");
            }
            other => panic!("{grants}: {:?}", other.err()),
        }
        assert!(!f.workspace(&id).exists(), "nothing was created");
        assert!(!f.world.root.join("record").exists(), "nothing was started");
    }
    // With the last grant added every resource is reachable.
    f.grant_data("~/content:ro;~/cores:ro;~/saves;~/states");
    let id = unique("fp-each");
    let mut launched = f.launch(&id).expect("now provable");
    wait_finished(&mut launched);
}

#[test]
fn content_in_the_host_tmp_is_refused_even_when_the_app_has_host_access() {
    // Fake home elsewhere, so the world (under the host /tmp) is not "home".
    let home = tempfile::tempdir().unwrap();
    let mut f = FlatpakWorld::new("fp-tmp", FLATPAK_RUN);
    f.host = FlatpakHost {
        xdg_data_home: home.path().join(".local/share"),
        xdg_cache_home: home.path().join(".cache"),
        xdg_config_home: home.path().join(".config"),
        system_flatpak_root: home.path().join("system-flatpak"),
        home: home.path().to_path_buf(),
    };
    f.grant_data("host");
    let id = unique("fp-tmp");
    match f.launch(&id) {
        Err(RetroArchCheatLaunchError::Sandbox(RetroArchRootError::ResourceNotVisible {
            resource,
            ..
        })) => assert_eq!(resource, "game content"),
        other => panic!("{:?}", other.err()),
    }
    // The ROM was neither copied nor moved.
    assert!(f.world.request.selected_content_path.is_file());
    assert!(!f.workspace(&id).exists());
    assert!(!f.world.root.join("record").exists());
}

#[test]
fn unknown_sandboxes_and_unreadable_flatpak_permissions_fail_closed() {
    let unknown = FlatpakWorld::new("fp-unknown", "exec bwrap --dev-bind / / retroarch \"$@\"");
    let id = unique("fp-unknown");
    assert!(matches!(
        unknown.launch(&id),
        Err(RetroArchCheatLaunchError::Sandbox(
            RetroArchRootError::SandboxUnknown
        ))
    ));
    assert!(!unknown.world.root.join("record").exists());

    let no_metadata = FlatpakWorld::new("fp-nometa", FLATPAK_RUN);
    let id = unique("fp-nometa");
    assert!(matches!(
        no_metadata.launch(&id),
        Err(RetroArchCheatLaunchError::Sandbox(
            RetroArchRootError::FlatpakPermissionsUnavailable(_)
        ))
    ));
    assert!(!no_metadata.workspace(&id).exists());
}

#[test]
fn a_flatpak_launch_without_cheats_is_unchanged_and_needs_no_proof() {
    let f = FlatpakWorld::new("fp-plain", FLATPAK_RUN);
    // No Flatpak metadata at all: a plain launch never consults it.
    let command = preflight_retroarch_launch(
        &f.world.request,
        &HostReadOnlyFilesystem,
        &f.world.environment,
    )
    .unwrap();
    let mut none = selection();
    none.selections.clear();
    let launched = launch_prepared_command(
        &f.world.request,
        command,
        &HostReadOnlyFilesystem,
        &f.world.environment,
        &none,
        &f.roots,
        "plain",
        Some(&f.host),
    )
    .unwrap();
    assert!(matches!(launched, RetroArchCheatLaunch::Plain(_)));
    assert!(!f.workspace("plain").exists(), "no workspace was created");
}

// ---- real Flatpak RetroArch (not part of the normal run) -------------------

/// A minimal, synthetic (not copyrighted) Mega Drive image: a valid vector
/// table and header over zero padding, enough for the core to start running.
fn synthetic_mega_drive_rom() -> Vec<u8> {
    let mut rom = vec![0_u8; 128 * 1024];
    rom[0..4].copy_from_slice(&0x00FF_0000_u32.to_be_bytes()); // initial SP
    rom[4..8].copy_from_slice(&0x0000_0200_u32.to_be_bytes()); // initial PC
    rom[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
    rom[0x120..0x130].copy_from_slice(b"EMUWIZ TEST ROM ");
    rom[0x200..0x204].copy_from_slice(&[0x60, 0xFE, 0x4E, 0x71]); // bra.s self
    rom
}

/// `REAL_RETROARCH_CORE=<core.so> REAL_RETROARCH_OUT=<dir>
/// REAL_RETROARCH_FIXTURE_BASE=<dir under $HOME> [REAL_RETROARCH_USER_CFG=<retroarch.cfg>]
/// xvfb-run -a cargo test ... -- --ignored real_flatpak`.
///
/// The production entry point launches the *real* Flatpak RetroArch (a wrapper
/// the classifier sees as `flatpak run`; it only swaps the stub core for the
/// real one and adds logging). Resource visibility is proven against the real
/// installed app's permissions and the real EmuWiz data root; no `TMPDIR`.
#[test]
#[ignore]
fn real_flatpak_retroarch_cheat_launch_end_to_end() {
    use sha2::{Digest, Sha256};
    let core = std::env::var("REAL_RETROARCH_CORE").expect("REAL_RETROARCH_CORE");
    let out = PathBuf::from(std::env::var("REAL_RETROARCH_OUT").expect("REAL_RETROARCH_OUT"));
    let base = PathBuf::from(
        std::env::var("REAL_RETROARCH_FIXTURE_BASE").expect("REAL_RETROARCH_FIXTURE_BASE"),
    );
    fs::create_dir_all(&out).unwrap();
    fs::create_dir_all(&base).unwrap();
    let hash = |path: &std::path::Path| -> String {
        Sha256::digest(fs::read(path).unwrap_or_default())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let user_cfg = std::env::var_os("REAL_RETROARCH_USER_CFG").map(PathBuf::from);
    let user_before = user_cfg.as_deref().map(hash);

    let world = World::new_in(&base, "real-fp", true, &synthetic_mega_drive_rom());
    let log = out.join("retroarch.log");
    let script = world.root.join("bin/retroarch");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\n# flatpak run org.libretro.RetroArch\n\
             prev=\"\"\nfor a in \"$@\"; do shift\n  if [ \"$prev\" = \"-L\" ]; then set -- \"$@\" '{core}'; else set -- \"$@\" \"$a\"; fi\n  prev=\"$a\"\ndone\n\
             printf '%s\\n' \"$@\" > '{argv}'\n\
             exec timeout 20 flatpak run org.libretro.RetroArch --verbose --log-file '{log}' \"$@\"\n",
            argv = out.join("argv").display(),
            log = log.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let content_before = fs::read(&world.request.selected_content_path).unwrap();
    let fixture_cfg_before = fs::read(world.real_config()).unwrap();
    let id = unique("real-fp");
    let mut launched = preflight_and_launch_retroarch_with_cheats(
        &world.request,
        &HostReadOnlyFilesystem,
        &world.environment,
        &selection(),
        &id,
    )
    .expect("the production entry point launches the real Flatpak RetroArch");
    let RetroArchCheatLaunch::WithCheats { session, .. } = &mut launched else {
        panic!("a cheat session")
    };
    let workspace_root = session.receipt().workspace_root.clone().unwrap();
    fs::write(out.join("workspace"), workspace_root.display().to_string()).unwrap();
    let cheat_files: Vec<_> = walk(&workspace_root)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "cht"))
        .collect();
    fs::write(
        out.join("cheat-files"),
        cheat_files
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    assert_eq!(cheat_files.len(), 1, "one selected-only cheat file");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !session.is_finished() {
        session.poll();
        assert!(Instant::now() < deadline, "RetroArch never finished");
        std::thread::sleep(Duration::from_millis(100));
    }
    let receipt = session.receipt().clone();
    fs::write(
        out.join("receipt.json"),
        serde_json::to_string_pretty(&receipt).unwrap(),
    )
    .unwrap();

    let text = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !text.contains("Config not found"),
        "config invisible:\n{text}"
    );
    assert!(
        !text.contains("Could not read content file"),
        "content invisible:\n{text}"
    );
    assert!(
        !text.contains("\"savefile_directory\" is not a directory")
            && !text.contains("\"savestate_directory\" is not a directory"),
        "save/state directories invisible:\n{text}"
    );
    assert!(
        text.contains("Loading content file"),
        "RetroArch never reached the content:\n{text}"
    );
    // L6: RetroArch's own log says it loaded the generated cheat file.
    let loaded = format!(
        "Load game-specific cheatfile: \"{}\"",
        cheat_files[0].display()
    );
    assert!(
        text.contains(&loaded) && text.contains("[Cheats] Applying cheat changes"),
        "RetroArch did not load the generated cheat file:\n{text}"
    );
    assert_eq!(
        fs::read(&world.request.selected_content_path).unwrap(),
        content_before
    );
    assert_eq!(fs::read(world.real_config()).unwrap(), fixture_cfg_before);
    if let (Some(path), Some(before)) = (user_cfg.as_deref(), user_before) {
        assert_eq!(hash(path), before, "the user's retroarch.cfg changed");
    }
    assert_eq!(receipt.cleanup, CleanupOutcome::Completed, "{receipt:?}");
    assert!(!workspace_root.exists(), "the workspace is removed");
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}
