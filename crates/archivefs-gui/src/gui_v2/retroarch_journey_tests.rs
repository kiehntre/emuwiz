//! The Blue RetroArch launch-time cheat journey on the shared verified-game
//! fixture, through production discovery: Library game → Launch page →
//! readiness → cheat file → tick → review → Play → RetroArch handoff → exit →
//! what the page then says.
//!
//! RetroArch discovery and the launch read the real process environment, so
//! each scenario runs in a child copy of this test binary whose HOME, XDG and
//! PATH point into the fixture root (nothing outside it is read). The fake
//! `retroarch` only records what it was handed: these tests prove what EmuWiz
//! configures and passes to RetroArch, never that an emulator activated a
//! cheat or that gameplay changed.

use super::{App, Route, Section};
use crate::launch_readiness_page::TestProbe;
use archivefs_core::verified_game_fixture::{FakeRetroArch, VerifiedGameFixture};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ROOT: &str = "EMUWIZ_JOURNEY_ROOT";
const SCENARIO: &str = "EMUWIZ_JOURNEY_SCENARIO";
/// Headless viewport for the journey: tall enough that the whole Launch
/// page is painted (egui paints only what is inside the viewport).
const SIZE: [f32; 2] = [1280.0, 2400.0];
/// The real window size the Xvfb picker acceptance clicks at.
const WINDOW: [f32; 2] = [1280.0, 800.0];
const PLAY: &str = "Play — Launch RetroArch";
const PLAY_ONE: &str = "Play — Launch RetroArch with 1 cheat(s)";
const REVIEWED: &str = "I reviewed this cheat for this game";
const NOT_STARTED: &str = "RetroArch was not started.";
const NOT_SEEN: &str = "EmuWiz did not see the cheats work.";
const NOT_CONFIRMED: &str = "cheat activation NOT confirmed";

const CHT: &str = "cheats = 2\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\ncheat0_enable = false\ncheat1_desc = \"Max Ammo\"\ncheat1_code = \"8000BB00\"\ncheat1_enable = false\n";
const CHT_TWO_VERSIONS: &str = "cheats = 2\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\ncheat0_enable = false\ncheat1_desc = \"Infinite Lives\"\ncheat1_code = \"8000AA01\"\ncheat1_enable = false\n";

// ---------------------------------------------------------------- parent --

/// One disposable profile: a verified game, a fake RetroArch with its own
/// argument-recording directory, and the cheat files. Every scenario gets its
/// own, so a recording can never carry over from another scenario.
struct Profile {
    fixture: VerifiedGameFixture,
    fake: FakeRetroArch,
    cheats: PathBuf,
}

impl Profile {
    fn new() -> Self {
        let fixture = VerifiedGameFixture::build().unwrap();
        let fake = fixture.fake_retroarch("mGBA").unwrap();
        let cheats = fixture.root().join("cheats");
        fs::create_dir_all(&cheats).unwrap();
        fs::write(cheats.join("fixture.cht"), CHT).unwrap();
        fs::write(cheats.join("two-versions.cht"), CHT_TWO_VERSIONS).unwrap();
        Self {
            fixture,
            fake,
            cheats,
        }
    }

    /// Runs `scenario` in a child process against this profile only.
    fn run(&self, scenario: &str, path_prefix: Option<&Path>) -> String {
        // Fail closed: nothing may already have been recorded.
        assert!(
            !self.fake.record_dir.join("args.txt").exists(),
            "{scenario}: a RetroArch recording exists before the scenario started"
        );
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "gui_v2::retroarch_journey_tests::child_entry",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            // Private HOME, XDG roots and an empty XDG_DATA_DIRS.
            .envs(self.fixture.child_environment())
            // Production discovery: the fake's settings live under this
            // XDG root and its executable is first on PATH.
            .env("XDG_CONFIG_HOME", self.fake.config_dir())
            // Only the fake's own directory: /usr/bin would expose a host
            // `retroarch` symlink. The fake uses absolute tool paths.
            .env(
                "PATH",
                match path_prefix {
                    Some(prefix) => format!(
                        "{}:{}",
                        prefix.display(),
                        self.fake.executable.parent().unwrap().display()
                    ),
                    None => self.fake.executable.parent().unwrap().display().to_string(),
                },
            )
            .env("EMUWIZ_TEST_REAL_ENVIRONMENT", "1")
            .env(ROOT, self.fixture.root())
            .env(SCENARIO, scenario);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "scenario {scenario} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(text.contains(&format!("journey {scenario}: ok")), "{text}");
        text
    }

    fn snapshot(&self) -> Vec<Vec<u8>> {
        snapshot(&self.fixture, &self.fake, &self.cheats)
    }
}

/// Executable leakage regression, entirely inside the root. The host leak is
/// a PATH entry: on this machine `/usr/bin/retroarch` is a symlink to
/// `/usr/games/retroarch`, so a child PATH containing /usr/bin exposed a real
/// emulator. A planted symlink on an extra PATH entry models that; the
/// isolated run must list the fake alone.
#[test]
fn a_host_retroarch_on_path_is_not_discovered_in_the_disposable_profile() {
    let profile = Profile::new();
    let leak = profile.fixture.root().join("leak");
    fs::create_dir_all(leak.join("bin")).unwrap();
    let leaked_executable = leak.join("retroarch");
    fs::write(&leaked_executable, "#!/bin/sh\nexit 0\n").unwrap();
    archivefs_core::verified_game_fixture::compile_native_test_executable(
        &leaked_executable,
        "exit 0",
    )
    .unwrap();
    std::os::unix::fs::symlink(&leaked_executable, leak.join("bin/retroarch")).unwrap();
    let isolated = profile.run("discovery", None);
    assert!(
        isolated.contains(&profile.fake.executable.display().to_string()),
        "{isolated}"
    );
    assert!(
        !isolated.contains(&leaked_executable.display().to_string()),
        "{isolated}"
    );
    let via_path = profile.run("discovery", Some(&leak.join("bin")));
    assert!(
        via_path.contains(&leaked_executable.display().to_string()),
        "{via_path}"
    );
}

/// Every Play scenario in its own profile and recording directory.
#[test]
fn retroarch_cheat_journey_in_blue_on_the_verified_fixture() {
    for scenario in ["plain", "cheats", "stale", "none-ticked", "two-versions"] {
        let profile = Profile::new();
        let before = profile.snapshot();
        profile.run(scenario, None);
        let recorded = profile.fake.record_dir.join("args.txt").exists();
        // Only the two scenarios that launch may leave a recording; the
        // refusals must have started nothing.
        assert_eq!(
            recorded,
            matches!(scenario, "plain" | "cheats"),
            "{scenario}: unexpected RetroArch recording state"
        );
        assert_eq!(
            profile.snapshot(),
            before,
            "{scenario}: the ROM, the user's RetroArch settings and the cheat files are byte-identical"
        );
        let launches = profile.fixture.root().join("tmp/emuwiz/retroarch-launches");
        assert!(
            !launches.exists() || fs::read_dir(&launches).unwrap().next().is_none(),
            "{scenario}: no temporary cheat profile is left behind"
        );
    }
}

/// The real setup worker saves an explicit choice, then a fresh Blue opens
/// with both native executables on PATH and launches only the chosen one.
#[test]
fn installation_picker_choice_is_reloaded_by_blue_play_cheats_and_save_scope() {
    for scenario in ["selected-plain", "selected-cheats"] {
        let profile = Profile::new();
        let second_dir = profile.fixture.root().join("second");
        fs::create_dir_all(&second_dir).unwrap();
        fs::copy(&profile.fake.executable, second_dir.join("retroarch")).unwrap();
        let before = profile.snapshot();
        profile.run(scenario, Some(&second_dir));
        assert!(profile.fake.record_dir.join("args.txt").is_file());
        assert_eq!(profile.snapshot(), before);
    }
}

fn snapshot(fixture: &VerifiedGameFixture, fake: &FakeRetroArch, cheats: &Path) -> Vec<Vec<u8>> {
    [
        fixture.rom_path.clone(),
        fake.config.clone(),
        cheats.join("fixture.cht"),
        cheats.join("two-versions.cht"),
    ]
    .iter()
    .map(|path| fs::read(path).unwrap())
    .collect()
}

// ----------------------------------------------------------------- child --

/// Not a test on its own: the scenario runner when started by the parent.
#[test]
fn child_entry() {
    let Some(root) = std::env::var_os(ROOT) else {
        return;
    };
    let scenario = std::env::var(SCENARIO).unwrap();
    let root = PathBuf::from(root);
    let fixture = VerifiedGameFixture::reopen(&root).unwrap();
    let cheats = root.join("cheats");
    let record = root.join("retroarch/record");
    if scenario.starts_with("selected-") {
        let selected = root.join("second/retroarch");
        let context = eframe::egui::Context::default();
        let mut picker = crate::retroarch_installation_setup::InstallationPicker::default();
        picker.start(Some(selected.clone()), context.clone());
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let mut changed = false;
            let _ = context.run(Default::default(), |context| {
                eframe::egui::CentralPanel::default().show(context, |ui| {
                    changed = picker.show(ui, context);
                });
            });
            if changed {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "installation picker did not finish"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let environment = archivefs_core::emulator_environment::retroarch::DiscoveryEnvironment::from_process_environment();
        assert_eq!(environment.explicit_executables, vec![selected]);
    }
    let mut blue = Blue::open(&fixture);
    blue.wait_for_readiness();
    if scenario.starts_with("selected-") {
        assert_eq!(
            blue.discovered_executables(),
            vec![root.join("second/retroarch").display().to_string()]
        );
        let selected_save = blue
            .app
            .native_workflows
            .as_ref()
            .unwrap()
            .save_state_target(blue.game, &fixture.rom_path);
        assert!(
            selected_save.is_some(),
            "the selected launch profile provides save guidance"
        );
    }
    let core = root.join("retroarch/cores/mgba_libretro.so");
    match scenario.as_str() {
        "discovery" => {
            println!(
                "discovery executables: {}",
                blue.discovered_executables().join(" ")
            );
        }
        "plain" | "selected-plain" => {
            blue.guard_executables(&root);
            blue.click(PLAY);
            let args = blue.wait_for_exit(&record);
            assert!(
                args.iter().any(|a| Path::new(a) == fixture.rom_path),
                "the verified ROM is handed to RetroArch: {args:?}"
            );
            assert_core(&args, &core);
            assert!(
                !blue.saw(NOT_CONFIRMED),
                "a plain launch never talks about cheats"
            );
        }
        "cheats" | "selected-cheats" => {
            blue.pick_cheat_file(&cheats.join("fixture.cht"));
            blue.click("Infinite Lives");
            assert!(blue.shown(REVIEWED), "{:?}", blue.last);
            // Review refusals come from the launch worker after Play.
            blue.guard_executables(&root);
            blue.click(PLAY_ONE);
            blue.wait_until(|b| b.shown(NOT_STARTED), 20);
            blue.open_launch_error();
            assert!(
                blue.shown("is not proven to belong to this game"),
                "{:?}",
                blue.last
            );
            assert!(
                !record.join("args.txt").exists(),
                "unreviewed cheats never launch"
            );
            blue.click(REVIEWED);
            assert!(blue.shown(PLAY_ONE), "{:?}", blue.last);
            blue.guard_executables(&root);
            blue.click(PLAY_ONE);
            let args = blue.wait_for_exit(&record);
            assert_core(&args, &core);
            // Configured and passed: a private profile, named after the core.
            let config = args
                .iter()
                .position(|a| a == "--config")
                .map(|i| PathBuf::from(&args[i + 1]))
                .expect("RetroArch is pointed at a private settings file");
            assert!(config.starts_with(root.join("tmp")), "{}", config.display());
            // RetroArch looks up cheats by the content basename, not the
            // name of the source .cht file the user selected.
            let cheat_name = fixture.rom_path.with_extension("cht");
            let cht = find_file(
                &record.join("profile"),
                cheat_name.file_name().unwrap().to_str().unwrap(),
            )
            .expect("the chosen cheats were written into the private profile");
            assert!(cht.parent().unwrap().ends_with("mGBA"), "{}", cht.display());
            let written = fs::read_to_string(&cht).unwrap();
            assert!(written.contains("Infinite Lives") && !written.contains("Max Ammo"));
            assert!(args.iter().any(|a| Path::new(a) == fixture.rom_path));
            // Said honestly: started, never confirmed, never seen to work.
            assert!(blue.saw(NOT_CONFIRMED), "{:?}", blue.seen);
            assert!(blue.shown(NOT_SEEN), "{:?}", blue.last);
            assert!(
                blue.shown("temporary cheat profile: Completed"),
                "{:?}",
                blue.last
            );
        }
        "stale" => {
            blue.pick_cheat_file(&cheats.join("fixture.cht"));
            blue.click("Infinite Lives");
            blue.click(REVIEWED);
            assert!(blue.shown(PLAY_ONE));
            // The file changes after it was reviewed.
            fs::write(
                cheats.join("fixture.cht"),
                CHT.replace("8000AA00", "8000AA99"),
            )
            .unwrap();
            blue.guard_executables(&root);
            blue.click(PLAY_ONE);
            blue.wait_until(|b| b.shown(NOT_STARTED), 20);
            assert!(!record.join("args.txt").exists(), "RetroArch never ran");
            fs::write(cheats.join("fixture.cht"), CHT).unwrap();
        }
        "none-ticked" => {
            blue.pick_cheat_file(&cheats.join("fixture.cht"));
            assert!(blue.shown("Infinite Lives") && blue.shown("Max Ammo"));
            // With nothing ticked the existing page offers an ordinary launch.
            assert!(blue.shown(PLAY), "{:?}", blue.last);
            assert!(!blue.shown(PLAY_ONE));
            assert!(!record.join("args.txt").exists());
        }
        "two-versions" => {
            blue.pick_cheat_file(&cheats.join("two-versions.cht"));
            blue.click("Infinite Lives");
            assert!(blue.shown("Choose a version"), "{:?}", blue.last);
            blue.guard_executables(&root);
            blue.click(PLAY_ONE);
            blue.wait_until(|b| b.shown(NOT_STARTED), 20);
            blue.open_launch_error();
            assert!(blue.shown("has more than one version"), "{:?}", blue.last);
            assert!(!record.join("args.txt").exists());
        }
        // Not a journey: prints where the chooser button and the Play
        // button are painted at 1280x800, for the real-binary Xvfb click.
        "coords" => {
            let layout = blue.paint_at(WINDOW);
            for label in ["Choose a cheat file (.cht)…", PLAY] {
                let rect = bounds(&layout, label)
                    .into_iter()
                    .find(|rect| rect.min.y > 64.0)
                    .unwrap_or_else(|| panic!("{label} not on screen: {:?}", blue.last));
                let centre = rect.center();
                println!("coords {label}\t{:.0}\t{:.0}", centre.x, centre.y);
            }
        }
        other => panic!("unknown scenario {other}"),
    }
    println!("journey {scenario}: ok");
}

/// A headless Blue on the Launch page of the fixture game.
struct Blue {
    context: eframe::egui::Context,
    app: App,
    game: i64,
    last: Vec<String>,
    seen: Vec<String>,
}

impl Blue {
    fn open(fixture: &VerifiedGameFixture) -> Self {
        let blue = Self::open_game(fixture);
        assert_eq!(blue.app.library.games[0].identity_summary(), "Verified");
        blue
    }

    fn open_game(fixture: &VerifiedGameFixture) -> Self {
        let context = eframe::egui::Context::default();
        context.style_mut(|style| style.animation_time = 0.0);
        let mut app = super::tests::fixture(&context);
        app.simple.enabled = true;
        app.library = Arc::new(super::verified_game_fixture_tests::load(fixture));
        let game = app.library.games[0].archive.id;
        app.router.current = Route::Task {
            section: Section::Launch,
            game,
        };
        let mut blue = Self {
            context,
            app,
            game,
            last: Vec::new(),
            seen: Vec::new(),
        };
        blue.paint();
        blue.app
            .native_workflows
            .as_mut()
            .expect("the Launch page created its workflows")
            .app
            .launch_retroarch
            .use_probe(Arc::new(TestProbe));
        blue
    }

    fn state(&mut self) -> &mut crate::launch_readiness_page::RetroArchLaunchState {
        &mut self
            .app
            .native_workflows
            .as_mut()
            .unwrap()
            .app
            .launch_retroarch
    }

    fn pick_cheat_file(&mut self, path: &Path) {
        self.state().pending_cheat_file = Some(path.to_path_buf());
        self.paint();
        self.paint();
    }

    fn paint(&mut self) -> eframe::egui::FullOutput {
        self.paint_at(SIZE)
    }

    fn paint_at(&mut self, size: [f32; 2]) -> eframe::egui::FullOutput {
        // Exactly the live frame's order (`App::ui`): the native workflows'
        // poll collects the evidence and emulator-profile results, then the
        // app poll, then the paint.
        self.app.mods.poll();
        if let Some(workflows) = self.app.native_workflows.as_mut() {
            workflows.poll(&self.context, &mut self.app.activity);
        }
        self.app.poll(&self.context);
        let output = self
            .context
            .run(input(None, size), |ctx| self.app.show(ctx));
        self.last = texts(&output);
        self.seen.extend(self.last.iter().cloned());
        output
    }

    fn shown(&self, needle: &str) -> bool {
        self.last.iter().any(|s| s.contains(needle))
    }

    fn saw(&self, needle: &str) -> bool {
        self.seen.iter().any(|s| s.contains(needle))
    }

    fn wait_until(&mut self, done: impl Fn(&Self) -> bool, seconds: u64) {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            self.paint();
            if done(&*self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out after {seconds}s; last frame: {:?}",
                self.last
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Readiness found the fake RetroArch through the process environment.
    fn wait_for_readiness(&mut self) {
        let game = self.game;
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            self.paint();
            let expected = Route::Task {
                section: Section::Launch,
                game,
            };
            if self.shown(PLAY) && self.app.router.current == expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "readiness timed out; {}; last frame: {:?}",
                self.diagnosis(),
                self.last
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!self.shown("could not be launched"), "{:?}", self.last);
    }

    /// Every RetroArch executable production discovery found.
    fn discovered_executables(&self) -> Vec<String> {
        use crate::emulator_setup::RetroArchProfilesState as P;
        let inner = &self.app.native_workflows.as_ref().unwrap().app;
        match &inner.emulator_readiness.retroarch_profiles {
            P::Ready(discovery) => discovery
                .profiles
                .iter()
                .flat_map(|profile| profile.executable_evidence.iter())
                .map(|path| path.display.clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Fail closed before any Play: a real emulator must never be started
    /// by this test, so every discovered executable has to live inside the
    /// disposable root.
    fn guard_executables(&self, root: &Path) {
        let executables = self.discovered_executables();
        assert!(
            !executables.is_empty(),
            "no RetroArch executable discovered"
        );
        for executable in &executables {
            assert!(
                Path::new(executable).starts_with(root),
                "refusing to launch: {executable} is outside the disposable root {}",
                root.display()
            );
        }
    }

    /// The worker states behind the Launch page, for a failure message.
    fn diagnosis(&self) -> String {
        use crate::emulator_setup::RetroArchProfilesState as P;
        use crate::selected_evidence_page::SelectedEvidenceState as E;
        let inner = &self.app.native_workflows.as_ref().unwrap().app;
        let evidence = match &inner.selected_evidence_ui.selected_evidence {
            E::Idle => "evidence Idle".to_string(),
            E::Loading { .. } => "evidence Loading".to_string(),
            E::Ready { .. } => "evidence Ready".to_string(),
            E::Error { message, .. } => format!("evidence Error: {message}"),
        };
        let profiles = match &inner.emulator_readiness.retroarch_profiles {
            P::NotScanned => "profiles NotScanned".to_string(),
            P::Scanning { .. } => "profiles Scanning".to_string(),
            P::Ready(discovery) => format!("profiles Ready: {discovery:?}"),
            P::Error(message) => format!("profiles Error: {message}"),
        };
        let plan = self
            .app
            .native_workflows
            .as_ref()
            .unwrap()
            .launch_plan_for_diagnosis();
        format!("{evidence}; {profiles}; plan: {plan}")
    }

    /// Returns the recorded arguments once RetroArch ran and the page settled.
    fn wait_for_exit(&mut self, record: &Path) -> Vec<String> {
        self.wait_until(
            |b| record.join("args.txt").exists() && (b.shown("exited") || b.shown("finished")),
            45,
        );
        // A few more frames so the summary after exit is on screen.
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(100));
            self.paint();
        }
        fs::read_to_string(record.join("args.txt"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn click(&mut self, label: &str) {
        let layout = self.paint();
        let point = bounds(&layout, label)
            .into_iter()
            .find(|rect| rect.min.y > 64.0)
            .unwrap_or_else(|| panic!("{label} not on screen: {:?}", self.last))
            .center();
        self.click_at(point);
    }

    fn open_launch_error(&mut self) {
        // The final disclosure belongs to the launch failure, after the
        // candidate's own technical details.
        let layout = self.paint();
        let point = bounds(&layout, "Technical details")
            .into_iter()
            .filter(|rect| rect.min.y > 64.0)
            .last()
            .expect("launch error disclosure is painted")
            .center();
        self.click_at(point);
    }

    fn click_at(&mut self, point: eframe::egui::Pos2) {
        for pressed in [true, false] {
            let _ = self
                .context
                .run(input(Some((point, pressed)), SIZE), |ctx| {
                    self.app.show(ctx)
                });
        }
        self.paint();
        self.paint();
    }
}

fn input(click: Option<(eframe::egui::Pos2, bool)>, size: [f32; 2]) -> eframe::egui::RawInput {
    let mut events = Vec::new();
    if let Some((pos, pressed)) = click {
        events.push(eframe::egui::Event::PointerMoved(pos));
        events.push(eframe::egui::Event::PointerButton {
            pos,
            button: eframe::egui::PointerButton::Primary,
            pressed,
            modifiers: eframe::egui::Modifiers::NONE,
        });
    }
    eframe::egui::RawInput {
        screen_rect: Some(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(size[0], size[1]),
        )),
        events,
        ..Default::default()
    }
}

fn texts(output: &eframe::egui::FullOutput) -> Vec<String> {
    fn gather(shape: &eframe::egui::Shape, out: &mut Vec<String>) {
        match shape {
            eframe::egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            eframe::egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| gather(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|s| gather(&s.shape, &mut out));
    out
}

fn bounds(output: &eframe::egui::FullOutput, wanted: &str) -> Vec<eframe::egui::Rect> {
    fn gather(shape: &eframe::egui::Shape, wanted: &str, out: &mut Vec<eframe::egui::Rect>) {
        match shape {
            eframe::egui::Shape::Text(text) if text.galley.text() == wanted => {
                out.push(eframe::egui::Rect::from_min_size(
                    text.pos,
                    text.galley.size(),
                ));
            }
            eframe::egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| gather(s, wanted, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|s| gather(&s.shape, wanted, &mut out));
    out
}

/// RetroArch was handed exactly the fixture's stub core.
fn assert_core(args: &[String], core: &Path) {
    let position = args
        .iter()
        .position(|a| a == "-L" || a == "--libretro")
        .unwrap_or_else(|| panic!("no core argument: {args:?}"));
    assert_eq!(Path::new(&args[position + 1]), core, "{args:?}");
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|f| f == name) {
            return Some(path);
        }
    }
    None
}

mod cartridge_cue;
