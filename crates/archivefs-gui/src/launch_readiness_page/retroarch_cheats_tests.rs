//! RetroArch launch-time cheats from the Playing Library: the chooser model and
//! the whole launch worker against a fake RetroArch and real (compiled) fake
//! cores, through the real isolated probe. No real RetroArch, ROM or user file
//! is involved.

use super::retroarch_cheats::{Choice, launch, unavailable_reason};
use archivefs_core::emulator_environment::retroarch::{
    DiscoveryEnvironment, ProfileKind, ProfileRef, ProfileScope,
};
use archivefs_core::game_identity::inspect_catalogued_game_identity;
use archivefs_core::launch::RetroArchLaunchRequest;
use archivefs_core::launch::cheat_runtime::CleanupOutcome;
use archivefs_core::launch::cheat_runtime_retroarch::RetroArchCheatLaunch;
use archivefs_core::launch::libretro_probe::{ProbeLauncher, run_probe_child};
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

const CHT: &str = "cheats = 2\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\ncheat0_enable = false\ncheat1_desc = \"Max Ammo\"\ncheat1_code = \"8000BB00\"\ncheat1_enable = false\n";

/// Re-runs this test binary as the isolated probe child.
pub(crate) struct TestProbe;

impl ProbeLauncher for TestProbe {
    fn child(&self) -> io::Result<(PathBuf, Vec<String>, Vec<(String, String)>)> {
        Ok((
            std::env::current_exe()?,
            vec![
                "--exact".into(),
                "launch_readiness_page::retroarch_cheats_tests::probe_child_entry".into(),
                "--nocapture".into(),
                "--test-threads=1".into(),
            ],
            vec![("EMUWIZ_PROBE_CHILD".into(), "1".into())],
        ))
    }
}

/// Not a test normally; the probe child when started by [`TestProbe`].
#[test]
fn probe_child_entry() {
    if std::env::var_os("EMUWIZ_PROBE_CHILD").is_none() {
        return;
    }
    let arguments: Vec<String> = std::env::args()
        .rev()
        .take(9)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    run_probe_child(&arguments);
}

fn compile_core(path: &Path, source: &str) {
    let c = path.with_extension("c");
    fs::write(&c, source).unwrap();
    let status = Command::new("cc")
        .args(["-shared", "-fPIC", "-O0", "-o"])
        .arg(path)
        .arg(&c)
        .status()
        .expect("a C compiler (cc) is needed for the fake cores");
    assert!(status.success());
}

fn core_source(library_name: &str) -> String {
    format!(
        "struct info {{ const char *n, *v, *e; _Bool a, b; }};\nvoid retro_get_system_info(struct info *i) {{ i->n = \"{library_name}\"; i->v = \"1\"; i->e = \"md\"; }}\n"
    )
}

struct World {
    root: PathBuf,
    request: RetroArchLaunchRequest,
    environment: DiscoveryEnvironment,
}

impl World {
    fn new(library_name: &str) -> Self {
        Self::with_core(
            library_name,
            "genesis_plus_gx",
            "megadrive",
            None,
            "MegaDrive",
        )
    }

    /// The same fake RetroArch and compiled stub core, launching `content`
    /// (a ROM from elsewhere, e.g. the shared verified-game fixture) when
    /// given, otherwise a Mega Drive ROM of its own.
    fn with_core(
        library_name: &str,
        core_stem: &str,
        system: &str,
        content: Option<&Path>,
        platform: &str,
    ) -> Self {
        let root = std::env::temp_dir().join(format!(
            "emuwiz-gui-ra-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let write = |rel: &str, bytes: &[u8]| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, bytes).unwrap();
            p
        };
        // Records its arguments and the temporary profile it was pointed at.
        let script = write(
            "bin/retroarch",
            format!(
                "#!/bin/sh\\nrec=\"{rec}\"\\nmkdir -p \"$rec\"\\nprintf '%s\\\\n' \"$@\" > \"$rec/args.txt\"\\n\
                 while [ $# -gt 0 ]; do if [ \"$1\" = \"--config\" ]; then cp -r \"$(dirname \"$(dirname \"$2\")\")\" \"$rec/profile\"; fi; shift; done\\nexit 0\\n",
                rec = root.join("record").display()
            )
            .replace("\\\\n", "\n")
            .replace("\\n", "\n")
            .as_bytes(),
        );
        archivefs_core::verified_game_fixture::compile_native_test_executable(
            &script,
            &fs::read_to_string(&script).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(root.join("saves")).unwrap();
        fs::create_dir_all(root.join("states")).unwrap();
        fs::create_dir_all(root.join("cores")).unwrap();
        let cfg = format!(
            "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\nvideo_driver = \"vulkan\"\ninput_player1_a = \"x\"\ncheat_database_path = \"/user/own/cheats\"\nconfig_save_on_exit = \"true\"\nsavefile_directory = \"{}\"\nsavestate_directory = \"{}\"\n",
            root.join("cores").display(),
            root.join("info").display(),
            root.join("saves").display(),
            root.join("states").display()
        );
        write("config/retroarch/retroarch.cfg", cfg.as_bytes());
        compile_core(
            &root.join("cores").join(format!("{core_stem}_libretro.so")),
            &core_source(library_name),
        );
        write(
            &format!("info/{core_stem}.info"),
            format!("systemname = \"{system}\"\n").as_bytes(),
        );
        let content = match content {
            Some(content) => content.to_path_buf(),
            None => write("content/game.md", b"synthetic mega drive rom bytes"),
        };
        let key = inspect_catalogued_game_identity(&content, Some(platform))
            .verified_loose_rom_sha256()
            .expect("verifiable loose rom")
            .to_string();
        let request = RetroArchLaunchRequest {
            selected_content_path: content,
            expected_platform_id: platform.into(),
            expected_game_key: key,
            profile: ProfileRef {
                profile_kind: ProfileKind::Native,
                scope: ProfileScope::User,
            },
            core_stem: core_stem.into(),
            expected_snapshot_evidence: None,
            expected_appimage_executable: None,
            expected_executable_binding: None,
            expected_configuration_file: None,
        };
        let environment = DiscoveryEnvironment {
            home: Some(root.clone().into_os_string()),
            xdg_config_home: Some(root.join("config").into_os_string()),
            path: Some(root.join("bin").into_os_string()),
            user_flatpak_root: root.join("user-flatpak"),
            system_flatpak_root: root.join("system-flatpak"),
            app_image_search_roots: Vec::new(),
            desktop_file_roots: Vec::new(),
            explicit_executables: Vec::new(),
        };
        Self {
            root,
            request,
            environment,
        }
    }

    fn cheat_file(&self, text: &str) -> PathBuf {
        let path = self.root.join("my-cheats").join("pick.cht");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn choice(&self, text: &str, ticked: &[&str]) -> Choice {
        let file = self.cheat_file(text);
        let mut choice = Choice::load(&self.request, &file).unwrap();
        for row in &mut choice.rows {
            if ticked.contains(&row.candidate.title.as_str()) {
                row.ticked = true;
                row.acknowledged = true;
            }
        }
        choice
    }

    fn launch(&self, choice: &Choice) -> Result<RetroArchCheatLaunch, String> {
        launch(&self.request, choice, &TestProbe, &self.environment)
    }

    fn args(&self) -> PathBuf {
        self.root.join("record/args.txt")
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

fn finish(launched: &mut RetroArchCheatLaunch) {
    let RetroArchCheatLaunch::WithCheats { session, .. } = launched else {
        panic!("expected a cheat launch");
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while !session.is_finished() {
        session.poll();
        assert!(
            Instant::now() < deadline,
            "the fake RetroArch never finished"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn recorded_cheats(world: &World) -> String {
    fn walk(dir: &Path, out: &mut String) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "cht") {
                out.push_str(&fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let mut text = String::new();
    walk(&world.root.join("record/profile"), &mut text);
    text
}

// ---- Choices ----------------------------------------------------------

#[test]
fn nothing_is_ticked_and_a_weak_match_needs_review_before_it_can_launch() {
    let world = World::new("Genesis Plus GX");
    let mut choice = Choice::load(&world.request, &world.cheat_file(CHT)).unwrap();
    assert_eq!(choice.rows.len(), 2);
    assert_eq!(choice.ticked_count(), 0, "nothing is ticked by default");
    assert!(
        choice
            .selections()
            .unwrap_err()
            .contains("Tick at least one")
    );
    choice.rows[0].ticked = true;
    let error = choice.selections().unwrap_err();
    assert!(
        error.contains("not proven to belong to this game"),
        "{error}"
    );
    choice.rows[0].acknowledged = true;
    let selections = choice.selections().unwrap();
    assert_eq!(selections.len(), 1);
    assert!(selections[0].review_acknowledged);
}

#[test]
fn two_versions_of_a_cheat_require_an_explicit_choice() {
    let world = World::new("Genesis Plus GX");
    let text = "cheats = 2\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\ncheat0_enable = false\ncheat1_desc = \"Infinite Lives\"\ncheat1_code = \"8000CC00\"\ncheat1_enable = false\n";
    let mut choice = Choice::load(&world.request, &world.cheat_file(text)).unwrap();
    let row = choice
        .rows
        .iter_mut()
        .find(|row| row.candidate.title == "Infinite Lives")
        .expect("the duplicated cheat is offered");
    row.ticked = true;
    row.acknowledged = true;
    if row.candidate.variants.len() > 1 || row.candidate.unresolved_conflict {
        let error = choice.selections().unwrap_err();
        assert!(error.contains("more than one version"), "{error}");
        choice.rows[0].variant = Some(0);
        assert!(choice.selections().unwrap()[0].variant_id.is_some());
    }
}

#[test]
fn files_that_are_not_cht_or_have_no_usable_cheat_are_refused() {
    let world = World::new("Genesis Plus GX");
    let other = world.root.join("my-cheats").join("notes.txt");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, CHT).unwrap();
    assert!(
        Choice::load(&world.request, &other)
            .unwrap_err()
            .contains(".cht")
    );
    let broken = world.cheat_file("cheats = 1\ncheat0_desc = \"No Code\"\ncheat0_enable = false\n");
    let loaded = Choice::load(&world.request, &broken);
    assert!(loaded.is_err() || loaded.unwrap().rows.is_empty());
    let linked = world.root.join("my-cheats").join("link.cht");
    symlink(&world.cheat_file(CHT), &linked).unwrap();
    assert!(Choice::load(&world.request, &linked).is_err());
}

#[test]
fn a_cheat_file_that_is_a_link_pipe_device_or_directory_is_refused_without_waiting() {
    let world = World::new("Genesis Plus GX");
    let dir = world.root.join("my-cheats");
    fs::create_dir_all(&dir).unwrap();
    let real = world.cheat_file(CHT);
    let link = dir.join("link.cht");
    symlink(&real, &link).unwrap();
    let pipe = dir.join("pipe.cht");
    assert!(
        Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap()
            .success()
    );
    let device = dir.join("device.cht");
    symlink("/dev/zero", &device).unwrap();
    let folder = dir.join("folder.cht");
    fs::create_dir_all(&folder).unwrap();
    let started = Instant::now();
    for refused in [&link, &pipe, &device, &folder] {
        let error = Choice::load(&world.request, refused).unwrap_err();
        assert!(error.contains("cannot be used"), "{refused:?}: {error}");
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    // A real file still loads.
    assert!(Choice::load(&world.request, &real).is_ok());
}

#[test]
fn a_choice_for_another_game_is_never_applied_and_unsupported_cases_are_explained() {
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    let mut other = world.request.clone();
    other.core_stem = "another_core".into();
    assert!(!choice.matches(&other));
    let error = launch(&other, &choice, &TestProbe, &world.environment)
        .err()
        .unwrap();
    assert!(error.contains("different game"), "{error}");
    assert!(
        unavailable_reason(&world.request, true)
            .unwrap()
            .contains("multi-disc")
    );
    let mut flatpak = world.request.clone();
    flatpak.profile.profile_kind = ProfileKind::Flatpak;
    assert!(
        unavailable_reason(&flatpak, false)
            .unwrap()
            .contains("Flatpak")
    );
    assert!(unavailable_reason(&world.request, false).is_none());
}

// ---- Launching --------------------------------------------------------

#[test]
fn ticked_cheats_reach_retroarch_and_unticked_ones_do_not() {
    let world = World::new("Genesis Plus GX");
    let source = world.cheat_file(CHT);
    let rom = fs::read(&world.request.selected_content_path).unwrap();
    let config = fs::read(world.real_config()).unwrap();
    let choice = world.choice(CHT, &["Infinite Lives"]);
    let mut launched = world.launch(&choice).expect("the launch should succeed");
    finish(&mut launched);
    let args = fs::read_to_string(world.args()).unwrap();
    assert!(args.contains("--config"), "{args}");
    let cheats = recorded_cheats(&world);
    assert!(cheats.contains("Infinite Lives"), "{cheats}");
    assert!(
        !cheats.contains("Max Ammo"),
        "unticked cheats must not be passed: {cheats}"
    );
    // Nothing the user owns was touched, and the temporary profile is gone.
    assert_eq!(fs::read_to_string(&source).unwrap(), CHT);
    assert_eq!(fs::read(&world.request.selected_content_path).unwrap(), rom);
    assert_eq!(fs::read(world.real_config()).unwrap(), config);
    let RetroArchCheatLaunch::WithCheats {
        session,
        activation,
        ..
    } = &launched
    else {
        unreachable!()
    };
    assert_eq!(session.receipt().cleanup, CleanupOutcome::Completed);
    let workspace = session.receipt().workspace_root.clone().unwrap();
    assert!(!workspace.exists(), "the temporary profile must be removed");
    let lines = super::retroarch_cheats::session_summary(session).join("\n");
    assert!(lines.contains("exited normally"), "{lines}");
    assert!(lines.contains("did not see the cheats work"), "{lines}");
    assert!(activation.label().contains("not verified"));
}

#[test]
fn a_cheat_file_changed_after_it_was_chosen_stops_the_launch_before_retroarch_starts() {
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    // Same size, different bytes: only the fingerprint can notice.
    fs::write(world.cheat_file(CHT), CHT.replace("8000AA00", "8000FF00")).unwrap();
    let error = world.launch(&choice).err().expect("must be refused");
    assert!(error.contains("RetroArch was not started"), "{error}");
    assert!(!world.args().exists(), "RetroArch must not have started");
}

#[test]
fn a_core_whose_name_cannot_be_confirmed_refuses_cheats_but_not_the_game() {
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    let core = world.root.join("cores").join("genesis_plus_gx_libretro.so");
    // Not a libretro core at all.
    compile_core(&core, "int unrelated(void) { return 1; }\n");
    let error = world.launch(&choice).err().unwrap();
    assert!(
        error.contains("could not confirm this core's library name"),
        "{error}"
    );
    assert!(error.contains("still launches normally"), "{error}");
    // An unsafe name is refused too.
    compile_core(&core, &core_source("../escape"));
    assert!(
        world
            .launch(&choice)
            .err()
            .unwrap()
            .contains("could not confirm")
    );
    // And a missing core is a refused launch check.
    fs::remove_file(&core).unwrap();
    let error = world.launch(&choice).err().unwrap();
    assert!(
        error.contains("changed since it was last checked"),
        "{error}"
    );
    assert!(!world.args().exists());
}

#[test]
fn the_library_name_comes_from_the_core_and_names_the_cheat_folder() {
    let world = World::new("dolphin-emu");
    let choice = world.choice(CHT, &["Max Ammo"]);
    let mut launched = world.launch(&choice).unwrap();
    finish(&mut launched);
    let found = fs::read_dir(world.root.join("record/profile"))
        .map(|_| {
            fn find(dir: &Path) -> bool {
                fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
                    let p = e.path();
                    (p.is_dir() && (p.file_name().is_some_and(|n| n == "dolphin-emu") || find(&p)))
                })
            }
            find(&world.root.join("record/profile"))
        })
        .unwrap_or(false);
    assert!(
        found,
        "the cheat folder is named after the core's own library name"
    );
}

#[test]
fn an_override_that_defeats_a_pinned_setting_or_cannot_be_read_blocks_cheats() {
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    let overrides = world.root.join("config/retroarch/config/Genesis Plus GX");
    fs::create_dir_all(&overrides).unwrap();
    let file = overrides.join("Genesis Plus GX.cfg");
    fs::write(&file, "savefile_directory = \"/elsewhere\"\n").unwrap();
    let error = world.launch(&choice).err().unwrap();
    assert!(error.contains("RetroArch was not started"), "{error}");
    assert!(!world.args().exists());
    // A linked override file is never followed and blocks cheats.
    fs::remove_file(&file).unwrap();
    let real = world.root.join("elsewhere.cfg");
    fs::write(&real, "video_scale = \"3\"\n").unwrap();
    symlink(&real, &file).unwrap();
    let error = world.launch(&choice).err().unwrap();
    assert!(error.contains("could not be checked safely"), "{error}");
    assert!(!world.args().exists());
}

#[test]
fn a_symlinked_settings_file_is_refused_naming_the_link_not_an_override() {
    // RetroArch discovery does not follow a symlinked retroarch.cfg (GNU Stow and
    // similar), for plain launches as well. The refusal must say so plainly.
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    let target = world.root.join("dotfiles").join("retroarch.cfg");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::rename(world.real_config(), &target).unwrap();
    symlink(&target, world.real_config()).unwrap();
    for case in ["valid link inside home", "broken link", "link leaving home"] {
        match case {
            "broken link" => {
                fs::remove_file(world.real_config()).unwrap();
                symlink(world.root.join("nowhere.cfg"), world.real_config()).unwrap();
            }
            "link leaving home" => {
                let outside = tempfile::tempdir().unwrap();
                let escaped = outside.path().join("retroarch.cfg");
                fs::copy(&target, &escaped).unwrap();
                fs::remove_file(world.real_config()).unwrap();
                symlink(&escaped, world.real_config()).unwrap();
                let error = world.launch(&choice).err().expect(case);
                assert!(error.contains("symbolic link"), "{case}: {error}");
                continue;
            }
            _ => {}
        }
        let error = world.launch(&choice).err().expect(case);
        assert!(error.contains("symbolic link"), "{case}: {error}");
        assert!(error.contains("regular copy"), "{case}: {error}");
        assert!(!error.contains("override"), "{case}: {error}");
        assert!(
            !world.args().exists(),
            "{case}: RetroArch must not have started"
        );
    }
}

#[test]
fn an_absent_settings_file_says_how_to_create_or_restore_it() {
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    fs::remove_file(world.real_config()).unwrap();
    let error = world.launch(&choice).err().expect("must refuse");
    assert!(error.contains("RetroArch was not started"), "{error}");
    assert!(error.contains("retroarch.cfg"), "{error}");
    assert!(error.contains("not found"), "{error}");
    assert!(error.contains("restore a backup"), "{error}");
    assert!(error.contains("open RetroArch once"), "{error}");
    assert!(!error.contains("Preflight"), "no debug dump: {error}");
    assert!(!world.args().exists(), "RetroArch must not have started");
}

#[test]
fn a_missing_or_replaced_core_is_refused_in_plain_words_before_anything_starts() {
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    fs::remove_file(world.root.join("cores/genesis_plus_gx_libretro.so")).unwrap();
    let error = world.launch(&choice).err().expect("must refuse");
    assert!(error.contains("RetroArch was not started"), "{error}");
    assert!(!error.contains("({"), "no raw debug struct: {error}");
    assert!(
        error.contains("no longer available") || error.contains("changed since"),
        "{error}"
    );
    assert!(!world.args().exists(), "RetroArch must not have started");
}

#[test]
fn a_running_cheat_launch_says_the_process_started_but_cheats_are_not_confirmed() {
    let line = super::retroarch_cheats::activation_label();
    assert!(line.contains("Process started"), "{line}");
    assert!(line.contains("NOT confirmed"), "{line}");
}

#[test]
fn a_retroarch_that_cannot_start_is_reported_as_a_failure_and_leaves_nothing_behind() {
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    let exe = world.root.join("bin/retroarch");
    archivefs_core::verified_game_fixture::compile_unstartable_native_test_executable(&exe)
        .unwrap();
    let error = world.launch(&choice).err().expect("must fail");
    assert!(error.contains("RetroArch was not started"), "{error}");
    assert!(error.contains("could not be started"), "{error}");
}

// ---- The shared launch state machine ------------------------------------

use super::{RetroArchLaunchDisplay, RetroArchLaunchState};
use std::sync::Arc;

fn state_for(world: &World, choice: Choice) -> RetroArchLaunchState {
    RetroArchLaunchState {
        cheats: Some(choice),
        probe: Some(Arc::new(TestProbe)),
        environment: Some(world.environment.clone()),
        ..Default::default()
    }
}

fn run_state(state: &mut RetroArchLaunchState) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        state.poll();
        if !state.is_active() {
            return;
        }
        assert!(Instant::now() < deadline, "the launch never finished");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_cheat_launch_runs_through_the_state_machine_and_reports_honestly() {
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    let mut state = state_for(&world, choice);
    state.start_with_cheats(world.request.clone());
    assert!(state.is_active());
    run_state(&mut state);
    match state.display_for(&world.request) {
        RetroArchLaunchDisplay::Exited { success, .. } => assert!(success),
        other => panic!(
            "expected an exited launch, got a different display: {}",
            match other {
                RetroArchLaunchDisplay::Failed { detail, .. } => detail,
                _ => "other".into(),
            }
        ),
    }
    let lines = state.cheat_lines(&world.request).join("\n");
    assert!(lines.contains("did not see the cheats work"), "{lines}");
    assert!(
        !lines.to_lowercase().contains("cheats are active"),
        "{lines}"
    );
    assert!(recorded_cheats(&world).contains("Infinite Lives"));
    // Another game's request never shows this launch's lines.
    let mut other = world.request.clone();
    other.core_stem = "someone_else".into();
    assert!(state.cheat_lines(&other).is_empty());
}

#[test]
fn a_refused_cheat_launch_is_shown_as_not_started_with_the_reason() {
    let world = World::new("Genesis Plus GX");
    let choice = world.choice(CHT, &["Infinite Lives"]);
    fs::write(world.cheat_file(CHT), CHT.replace("8000AA00", "8000FF00")).unwrap();
    let mut state = state_for(&world, choice);
    state.start_with_cheats(world.request.clone());
    run_state(&mut state);
    match state.display_for(&world.request) {
        RetroArchLaunchDisplay::Failed { message, detail } => {
            assert!(message.contains("not started"), "{message}");
            assert!(detail.contains("RetroArch was not started"), "{detail}");
        }
        _ => panic!("a refused cheat launch must be shown as a failure"),
    }
    assert!(state.failure_detail().is_some());
    assert!(!world.args().exists(), "RetroArch must not have started");
}

// ---- Shared verified-game fixture ---------------------------------------

/// The fixture's ROM - discovered, scanned and DAT-verified through the
/// production pipeline - is handed to RetroArch with the selected cheat.
/// The fake RetroArch records the exact arguments and the temporary
/// profile it was given.
#[test]
fn a_verified_fixture_game_reaches_retroarch_with_core_config_and_selected_cheat() {
    let fixture = crate::gui_v2::verified_game_fixture::VerifiedGame::new();
    let platform = fixture
        .verified_platform()
        .expect("a verified game with a scanned platform");
    let world = World::with_core(
        "Snes9x",
        "snes9x",
        "Super Nintendo Entertainment System",
        Some(&fixture.rom),
        &platform,
    );
    let rom = fs::read(&fixture.rom).unwrap();
    let choice = world.choice(CHT, &["Infinite Lives"]);
    let cheat_path = world.root.join("my-cheats").join("pick.cht");
    // The selection the launch is bound to: this ROM, this core, this file.
    assert_eq!(choice.file, cheat_path);
    assert_eq!(choice.content, fixture.rom);
    assert_eq!(choice.core_stem, "snes9x");
    let mut launched = world.launch(&choice).expect("the launch should start");
    finish(&mut launched);

    let args: Vec<String> = fs::read_to_string(world.args())
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let core = world.root.join("cores").join("snes9x_libretro.so");
    let at = |flag: &str| args.iter().position(|arg| arg == flag);
    let core_at = at("-L").expect("a core argument");
    assert_eq!(Path::new(&args[core_at + 1]), core, "{args:?}");
    let config_at = at("--config").expect("a config argument");
    let config = Path::new(&args[config_at + 1]);
    assert!(
        !config.starts_with(world.root.join("config")),
        "RetroArch must get a temporary profile, not the user's: {args:?}"
    );
    // Exactly: -L <core> <content> --config <temporary profile>.
    assert_eq!(args.len(), 5, "{args:?}");
    assert_eq!(
        Path::new(&args[core_at + 2]),
        fixture.rom.as_path(),
        "the verified ROM is the content: {args:?}"
    );
    // The temporary profile RetroArch saw carries the ticked cheat only.
    let cheats = recorded_cheats(&world);
    assert!(cheats.contains("Infinite Lives"), "{cheats}");
    assert!(!cheats.contains("Max Ammo"), "{cheats}");
    let RetroArchCheatLaunch::WithCheats { session, .. } = &launched else {
        unreachable!()
    };
    assert_eq!(session.receipt().cleanup, CleanupOutcome::Completed);
    // Nothing the user owns changed.
    assert_eq!(fs::read(&fixture.rom).unwrap(), rom);
    assert_eq!(fs::read_to_string(&cheat_path).unwrap(), CHT);
}
