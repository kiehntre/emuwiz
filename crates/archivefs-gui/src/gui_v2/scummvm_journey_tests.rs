//! The Blue ScummVM Play journey through production discovery, detection and
//! the existing Gold launch engine: catalogued ScummVM folder → Launch page →
//! readiness → (optional saved trainer profile) → Play → ScummVM process →
//! running → exit → what the page then says.
//!
//! Detection, readiness and the launch all resolve the ScummVM executable
//! from the process environment, so each scenario runs in a child copy of
//! this test binary. Its HOME is the test binary's private tree, PATH holds
//! only the fixture's `bin`, and the test-only `TEST_EXECUTABLE_ENV` route
//! makes the fake the only candidate, so a host ScummVM in `/usr/games` is
//! never found. Before any click the child also refuses to continue unless
//! the resolved executable lives in the disposable root. The fake only
//! records what it was handed: these tests prove what EmuWiz runs and
//! reports, never that a game played or that a trainer option took effect.

use super::{App, Route, Section};
use archivefs_core::scummvm_detection::TEST_EXECUTABLE_ENV;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ROOT: &str = "EMUWIZ_SCUMMVM_JOURNEY_ROOT";
const SCENARIO: &str = "EMUWIZ_SCUMMVM_JOURNEY_SCENARIO";
const SIZE: [f32; 2] = [1280.0, 2400.0];
const KEY: &str = "hypno:wetlands";
const PLAY: &str = "Launch ScummVM";
const AGAIN: &str = "Launch ScummVM again (recheck)";
const CHECK: &str = "Check saved trainer profile";
const USE: &str = "Use this saved profile for the next launch";
const RUNNING: &str =
    "ScummVM process started; EmuWiz cannot tell whether the game itself is running.";
const RUNNING_TRAINER: &str = "ScummVM process started with the saved trainer profile you chose.";
const CLEAN_EXIT: &str = "ScummVM exited normally; this does not prove the game ran.";
const NOT_STARTED: &str = "ScummVM was not started";

// ---------------------------------------------------------------- parent --

/// One disposable root: a game folder, the fake `scummvm` and its recording.
struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let folder = root.join("Wetlands");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("wetlands.mis"), b"synthetic game bytes").unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("record")).unwrap();
        fs::write(root.join("detect"), KEY).unwrap();
        fs::write(root.join("exit_code"), "0").unwrap();
        let exe = root.join("bin/scummvm");
        let r = root.display();
        // `--version` answers the readiness probe; `-p`/`-c` is a launch that
        // records its argv (and, for `-c`, the configuration it was given),
        // stays up briefly so the page can show it running, then exits with
        // the code in `exit_code`; anything else is detection.
        fs::write(
            &exe,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = --version ] && {{ echo 'ScummVM 2.8.0 (journey fake)'; exit 0; }}; done\nif [ \"$1\" = -p ] || [ \"$1\" = -c ]; then\n  printf '%s\\n' \"$@\" > '{r}/record/argv.tmp'\n  if [ \"$1\" = -c ]; then /bin/cp \"$2\" '{r}/record/config.ini'; fi\n  /bin/sleep 1\n  /bin/mv '{r}/record/argv.tmp' '{r}/record/argv.txt'\n  exit $(/bin/cat '{r}/exit_code')\nfi\nprintf 'Game ID: %s\\n' \"$(/bin/cat '{r}/detect')\"\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        // Wait out a transient ETXTBSY from another test thread's fork.
        for _ in 0..10_000 {
            match Command::new(&exe).arg("--version").output() {
                Ok(_) => break,
                Err(e) if e.raw_os_error() == Some(26) => std::thread::yield_now(),
                Err(e) => panic!("fixture not spawnable: {e}"),
            }
        }
        Self { dir }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn run(&self, scenario: &str) -> String {
        let root = self.root();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "gui_v2::scummvm_journey_tests::child_entry",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env("PATH", root.join("bin"))
            .env(TEST_EXECUTABLE_ENV, root.join("bin/scummvm"))
            .env(ROOT, root)
            .env(SCENARIO, scenario)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            output.status.success(),
            "scenario {scenario} failed:\n{text}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains(&format!("journey {scenario}: ok")), "{text}");
        text
    }

    fn game_bytes(&self) -> Vec<u8> {
        fs::read(self.root().join("Wetlands/wetlands.mis")).unwrap()
    }
}

#[test]
fn scummvm_play_journey_in_blue() {
    for scenario in [
        "details-play",
        "plain",
        "failure",
        "profile",
        "no-profile",
        "untick",
        "no-saves",
        "drift",
    ] {
        let lab = Lab::new();
        let before = lab.game_bytes();
        lab.run(scenario);
        assert_eq!(
            lab.root().join("record/argv.txt").exists(),
            matches!(scenario, "plain" | "failure" | "profile" | "untick"),
            "{scenario}: unexpected ScummVM recording state"
        );
        assert_eq!(lab.game_bytes(), before, "{scenario}: game files unchanged");
        let names: Vec<_> = fs::read_dir(lab.root().join("Wetlands"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            names.len(),
            1,
            "{scenario}: nothing added to the game folder"
        );
    }
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
    let folder = root.join("Wetlands");
    let argv = root.join("record/argv.txt");
    let launch = Route::Task {
        section: Section::Launch,
        game: 9,
    };
    if scenario == "details-play" {
        // Game Details: the readiness card's Play routes to the Launch page,
        // which then offers the ScummVM launch; nothing starts on the way.
        let mut blue = Blue::open(&folder, Route::Game(9));
        blue.wait_until(|b| b.shown("Play"), 45);
        blue.click("Play");
        assert_eq!(blue.app.router.current, launch);
        blue.wait_for_readiness();
        assert!(!argv.exists());
        println!("journey {scenario}: ok");
        return;
    }
    let mut blue = Blue::open(&folder, launch);
    blue.wait_for_readiness();
    match scenario.as_str() {
        "plain" => {
            blue.guard_executable(&root);
            blue.click(PLAY);
            blue.wait_until(|b| b.shown("Running"), 20);
            assert!(blue.shown(RUNNING), "{:?}", blue.last);
            let args = blue.wait_for_exit(&argv);
            assert_eq!(args.first().map(String::as_str), Some("-p"), "{args:?}");
            assert!(args.iter().any(|a| Path::new(a) == folder), "{args:?}");
            assert!(args.iter().any(|a| a.ends_with("wetlands")), "{args:?}");
            assert!(blue.shown("Process exited"), "{:?}", blue.last);
            assert!(blue.shown(CLEAN_EXIT), "{:?}", blue.last);
            // A plain launch never claims a trainer.
            assert!(!blue.saw("saved trainer profile you chose"));
            assert!(blue.shown(AGAIN), "{:?}", blue.last);
        }
        "failure" => {
            fs::write(root.join("exit_code"), "3").unwrap();
            blue.guard_executable(&root);
            blue.click(PLAY);
            blue.wait_for_exit(&argv);
            assert!(
                blue.shown("ScummVM failed: exit status: 3"),
                "{:?}",
                blue.last
            );
        }
        "profile" => {
            let profile = write_profile(&folder);
            let saved = fs::read(&profile).unwrap();
            // A trainer launch needs an established save location (the
            // private HOME's default one here) and must leave it untouched.
            let saves =
                PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/share/scummvm/saves");
            fs::create_dir_all(&saves).unwrap();
            fs::write(saves.join("wetlands.000"), b"valuable save").unwrap();
            blue.click(CHECK);
            assert!(blue.shown("Profile fingerprint"), "{:?}", blue.last);
            assert!(blue.shown("infiniteHealth: On"), "{:?}", blue.last);
            blue.click(USE);
            blue.guard_executable(&root);
            blue.click(PLAY);
            blue.wait_until(|b| b.saw(RUNNING_TRAINER), 20);
            let args = blue.wait_for_exit(&argv);
            assert_eq!(args.first().map(String::as_str), Some("-c"), "{args:?}");
            // ScummVM ran on a private copy, never the saved profile itself.
            assert!(!args.iter().any(|a| Path::new(a) == profile), "{args:?}");
            let seen = fs::read_to_string(root.join("record/config.ini")).unwrap();
            assert!(seen.contains("infiniteHealth=true"), "{seen}");
            assert_eq!(
                fs::read(&profile).unwrap(),
                saved,
                "saved profile unchanged"
            );
            assert_eq!(
                fs::read(saves.join("wetlands.000")).unwrap(),
                b"valuable save"
            );
            assert_eq!(fs::read_dir(&saves).unwrap().count(), 1, "no save added");
            assert!(blue.shown(CLEAN_EXIT), "{:?}", blue.last);
            assert!(
                blue.shown("EmuWiz did not observe whether any trainer option took effect."),
                "{:?}",
                blue.last
            );
            // One tick, one launch: the choice is spent.
            assert!(!blue.shown(USE), "{:?}", blue.last);
        }
        "no-profile" => {
            blue.click(CHECK);
            assert!(
                blue.shown("No saved trainer profile exists for this game"),
                "{:?}",
                blue.last
            );
            assert!(!blue.shown(USE), "{:?}", blue.last);
        }
        "untick" => {
            // Checked and Ready but never ticked: an ordinary launch.
            write_profile(&folder);
            blue.click(CHECK);
            assert!(blue.shown(USE), "{:?}", blue.last);
            blue.guard_executable(&root);
            blue.click(PLAY);
            let args = blue.wait_for_exit(&argv);
            assert_eq!(args.first().map(String::as_str), Some("-p"), "{args:?}");
            assert!(!blue.saw(RUNNING_TRAINER));
        }
        "no-saves" => {
            // A ticked profile with no established save location is refused
            // before ScummVM starts, and says why.
            write_profile(&folder);
            blue.click(CHECK);
            blue.click(USE);
            blue.guard_executable(&root);
            blue.click(PLAY);
            blue.wait_until(|b| b.shown("Launch failed"), 20);
            assert!(
                blue.shown("save location could not be established"),
                "{:?}",
                blue.last
            );
        }
        "drift" => {
            // The folder now detects as a different game: core's fresh
            // detection at click time refuses before anything starts.
            fs::write(root.join("detect"), "hypno:spiderman").unwrap();
            blue.guard_executable(&root);
            blue.click(PLAY);
            blue.wait_until(|b| b.shown("Launch failed"), 20);
            assert!(blue.saw(NOT_STARTED), "{:?}", blue.last);
            assert!(blue.saw("IdentityMismatch"), "{:?}", blue.last);
        }
        other => panic!("unknown scenario {other}"),
    }
    println!("journey {scenario}: ok");
}

/// A saved trainer profile where Cheats & Mods keeps it for this game.
fn write_profile(folder: &Path) -> PathBuf {
    let profile = crate::cheats_mods::scummvm_trainer::owned_profile_path(folder, KEY).unwrap();
    fs::create_dir_all(profile.parent().unwrap()).unwrap();
    fs::write(
        &profile,
        format!(
            "[emuwiz-trainer]\nengineid=hypno\ngameid=wetlands\npath={}\ninfiniteHealth=true\n",
            folder.display()
        ),
    )
    .unwrap();
    profile
}

/// A headless Blue on the Launch page of the catalogued ScummVM folder.
struct Blue {
    context: eframe::egui::Context,
    app: App,
    last: Vec<String>,
    seen: Vec<String>,
}

impl Blue {
    fn open(folder: &Path, route: Route) -> Self {
        let context = eframe::egui::Context::default();
        context.style_mut(|style| style.animation_time = 0.0);
        let mut app = super::tests::fixture(&context);
        app.simple.enabled = true;
        let mut row = super::tests::archive(9, "Wetlands", Some("ScummVM"));
        row.archive_kind = "scummvm_directory".into();
        row.relative_path = "Wetlands".into();
        row.absolute_path = folder.to_path_buf();
        app.library = Arc::new(super::Library::new(vec![row]));
        app.router.current = route;
        let mut blue = Self {
            context,
            app,
            last: Vec::new(),
            seen: Vec::new(),
        };
        blue.paint();
        blue
    }

    fn paint(&mut self) -> eframe::egui::FullOutput {
        self.app.mods.poll();
        if let Some(workflows) = self.app.native_workflows.as_mut() {
            workflows.poll(&self.context, &mut self.app.activity);
        }
        self.app.poll(&self.context);
        let output = self.context.run(input(None), |ctx| self.app.show(ctx));
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
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_for_readiness(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            self.paint();
            if self.shown(PLAY) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "readiness timed out; plan: {}; last frame: {:?}",
                self.app
                    .native_workflows
                    .as_ref()
                    .map(|w| w.launch_plan_for_diagnosis())
                    .unwrap_or_default(),
                self.last
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Fail closed before any Play: the executable production resolution
    /// found must be the fixture inside the disposable root.
    fn guard_executable(&self, root: &Path) {
        let found = archivefs_core::scummvm_detection::resolve_scummvm_executable()
            .expect("a ScummVM executable is resolved");
        assert!(
            found.starts_with(root),
            "refusing to launch: {} is outside the disposable root {}",
            found.display(),
            root.display()
        );
        let plan = self
            .app
            .native_workflows
            .as_ref()
            .unwrap()
            .launch_plan_for_diagnosis();
        assert!(plan.contains(&root.display().to_string()), "{plan}");
        assert!(!plan.contains("/usr/games"), "{plan}");
    }

    fn wait_for_exit(&mut self, argv: &Path) -> Vec<String> {
        self.wait_until(|b| argv.exists() && b.shown("Process exited"), 30);
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(50));
            self.paint();
        }
        fs::read_to_string(argv)
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
        for pressed in [true, false] {
            let _ = self
                .context
                .run(input(Some((point, pressed))), |ctx| self.app.show(ctx));
        }
        self.paint();
        self.paint();
    }
}

fn input(click: Option<(eframe::egui::Pos2, bool)>) -> eframe::egui::RawInput {
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
            eframe::egui::vec2(SIZE[0], SIZE[1]),
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
