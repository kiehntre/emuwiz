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
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ROOT: &str = "EMUWIZ_JOURNEY_ROOT";
const SCENARIO: &str = "EMUWIZ_JOURNEY_SCENARIO";
const SIZE: [f32; 2] = [1280.0, 800.0];
const PLAY: &str = "Play — Launch RetroArch";
const PLAY_ONE: &str = "Play — Launch RetroArch with 1 cheat(s)";
const REVIEWED: &str = "I reviewed this cheat for this game";
const NOT_STARTED: &str = "RetroArch was not started.";
const NOT_SEEN: &str = "EmuWiz did not see the cheats work.";
const NOT_CONFIRMED: &str = "cheat activation NOT confirmed";

const CHT: &str = "cheats = 2\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\ncheat0_enable = false\ncheat1_desc = \"Max Ammo\"\ncheat1_code = \"8000BB00\"\ncheat1_enable = false\n";
const CHT_TWO_VERSIONS: &str = "cheats = 2\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\ncheat0_enable = false\ncheat1_desc = \"Infinite Lives\"\ncheat1_code = \"8000AA01\"\ncheat1_enable = false\n";

// ---------------------------------------------------------------- parent --

/// Builds one fixture and runs every scenario in a child process against it.
#[test]
fn retroarch_cheat_journey_in_blue_on_the_verified_fixture() {
    let fixture = VerifiedGameFixture::build().unwrap();
    let fake = fixture.fake_retroarch("mGBA").unwrap();
    let cheats = fixture.root().join("cheats");
    fs::create_dir_all(&cheats).unwrap();
    fs::write(cheats.join("fixture.cht"), CHT).unwrap();
    fs::write(cheats.join("two-versions.cht"), CHT_TWO_VERSIONS).unwrap();
    let before = snapshot(&fixture, &fake, &cheats);
    for scenario in ["plain", "cheats", "stale", "none-ticked", "two-versions"] {
        let _ = fs::remove_dir_all(&fake.record_dir);
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "gui_v2::retroarch_journey_tests::child_entry",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .envs(fixture.child_environment())
            // Production discovery: the fake's settings live under this
            // XDG root and its executable is first on PATH.
            .env("XDG_CONFIG_HOME", &fake.config_dir())
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin",
                    fake.executable.parent().unwrap().display()
                ),
            )
            .env("EMUWIZ_TEST_REAL_ENVIRONMENT", "1")
            .env(ROOT, fixture.root())
            .env(SCENARIO, scenario)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "scenario {scenario} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains(&format!("journey {scenario}: ok")), "{text}");
    }
    assert_eq!(
        snapshot(&fixture, &fake, &cheats),
        before,
        "the ROM, the user's RetroArch settings and the cheat files are byte-identical after every scenario"
    );
    assert!(
        !fixture
            .root()
            .join("tmp/emuwiz/retroarch-launches")
            .exists()
            || fs::read_dir(fixture.root().join("tmp/emuwiz/retroarch-launches"))
                .unwrap()
                .next()
                .is_none(),
        "no temporary cheat profile is left behind"
    );
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
    let mut blue = Blue::open(&fixture);
    blue.wait_for_readiness();
    match scenario.as_str() {
        "plain" => {
            blue.click(PLAY);
            let args = blue.wait_for_exit(&record);
            assert!(
                args.iter().any(|a| Path::new(a) == fixture.rom_path),
                "the verified ROM is handed to RetroArch: {args:?}"
            );
            assert!(
                !blue.saw(NOT_CONFIRMED),
                "a plain launch never talks about cheats"
            );
        }
        "cheats" => {
            blue.pick_cheat_file(&cheats.join("fixture.cht"));
            blue.click("Infinite Lives");
            assert!(blue.shown("is not proven to belong to this game"));
            blue.click(REVIEWED);
            assert!(blue.shown(PLAY_ONE), "{:?}", blue.last);
            blue.click(PLAY_ONE);
            let args = blue.wait_for_exit(&record);
            // Configured and passed: a private profile, named after the core.
            let config = args
                .iter()
                .position(|a| a == "--config")
                .map(|i| PathBuf::from(&args[i + 1]))
                .expect("RetroArch is pointed at a private settings file");
            assert!(config.starts_with(root.join("tmp")), "{}", config.display());
            let cht = find_file(&record.join("profile"), "fixture.cht")
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
            blue.click(PLAY_ONE);
            blue.wait_until(|b| b.shown(NOT_STARTED), 20);
            assert!(!record.join("args.txt").exists(), "RetroArch never ran");
            fs::write(cheats.join("fixture.cht"), CHT).unwrap();
        }
        "none-ticked" => {
            blue.pick_cheat_file(&cheats.join("fixture.cht"));
            assert!(blue.shown("Infinite Lives") && blue.shown("Max Ammo"));
            assert!(blue.shown("Tick at least one cheat"), "{:?}", blue.last);
            assert!(!blue.shown(PLAY_ONE));
            assert!(!record.join("args.txt").exists());
        }
        "two-versions" => {
            blue.pick_cheat_file(&cheats.join("two-versions.cht"));
            blue.click("Infinite Lives");
            assert!(blue.shown("has more than one version"), "{:?}", blue.last);
            assert!(!blue.shown(PLAY_ONE));
            assert!(!record.join("args.txt").exists());
        }
        // Not a journey: prints where the chooser button and the Play
        // button are painted at 1280x800, for the real-binary Xvfb click.
        "coords" => {
            let layout = blue.paint();
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
        let context = eframe::egui::Context::default();
        context.style_mut(|style| style.animation_time = 0.0);
        let mut app = super::tests::fixture(&context);
        app.simple.enabled = true;
        app.library = Arc::new(super::verified_game_fixture_tests::load(fixture));
        let game = app.library.games[0].archive.id;
        assert_eq!(app.library.games[0].identity_summary(), "Verified");
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
        // As the live frame does: collect worker results, then paint.
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
        format!("{evidence}; {profiles}")
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
