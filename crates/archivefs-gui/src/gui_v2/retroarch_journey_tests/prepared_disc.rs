//! Recovery engine acceptance, through Blue itself: a CD32 disc stored as a
//! ZIP cannot start; the game's page offers Game recovery; the person
//! confirms; a verified playable copy is prepared as new files; the Launch
//! page asks for the usual folder-only confirmation; Play hands the prepared
//! cue sheet to RetroArch. The ZIP is unchanged throughout.
//!
//! Synthetic disc, disposable profile, recording fake RetroArch (see the
//! parent module): this proves what EmuWiz prepares, checks and passes to the
//! emulator, not that an emulator ran a game.
use super::*;

const CHILD: &str = "gui_v2::retroarch_journey_tests::prepared_disc::child_entry";
const MARK: &str = "EMUWIZ_PREPARED_DISC_JOURNEY";
const TITLE: &str = "Synthetic Disc (1994)(Test)[!]";
const TRY_ONCE: &str = "Try once as Amiga CD32";

fn disc_members() -> Vec<(String, Vec<u8>)> {
    archivefs_core::synthetic_media::cd_disc_set(TITLE)
}

fn zip_path(root: &Path) -> PathBuf {
    root.join("collection/amiga-cd32")
        .join(format!("{TITLE}.zip"))
}

/// The disposable profile: the shared verified-game library, plus a second
/// games folder whose `amiga-cd32` subfolder holds the zipped disc, scanned by
/// the production scan; and a fake RetroArch with a PUAE core whose `.info`
/// names the CD32 and its two required firmware files (synthetic stand-ins).
fn profile(root: &Path) -> VerifiedGameFixture {
    profile_in(root, "amiga-cd32")
}

/// [`profile`], with the zipped disc in `collection/<folder>`. Only the
/// `amiga-cd32` folder names the system; any other leaves it unrecorded.
fn profile_in(root: &Path, folder: &str) -> VerifiedGameFixture {
    let fixture = VerifiedGameFixture::build_at(root).unwrap();
    let zip = root
        .join("collection")
        .join(folder)
        .join(format!("{TITLE}.zip"));
    fs::create_dir_all(zip.parent().unwrap()).unwrap();
    let members = disc_members();
    let borrowed: Vec<(&str, &[u8])> = members
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    fs::write(&zip, archivefs_core::synthetic_media::stored_zip(&borrowed)).unwrap();
    let collection = root.join("collection");
    archivefs_core::add_source_folder_at(&fixture.config_path, &fixture.database_path, &collection)
        .unwrap();
    archivefs_core::scan_source_folder_at(
        &fixture.config_path,
        &fixture.database_path,
        &collection,
        "prepared-disc-journey",
    )
    .unwrap();
    let row = archivefs_core::Database::open_read_only(&fixture.database_path)
        .unwrap()
        .load_archives()
        .unwrap()
        .into_iter()
        .find(|row| row.absolute_path == zip)
        .expect("the zipped disc is catalogued");
    if folder != "amiga-cd32" {
        assert_eq!(row.platform, None, "{row:?}");
    } else {
        assert_eq!(row.platform.as_deref(), Some("AmigaCD32"), "{row:?}");
        assert_eq!(
            row.platform_source.as_deref(),
            Some("folder_alias"),
            "{row:?}"
        );
    }

    let fake = fixture.fake_retroarch("mGBA").unwrap();
    let core = fake.cores_dir.join("puae_libretro.so");
    archivefs_core::verified_game_fixture::compile_stub_core(&core, "PUAE").unwrap();
    fs::write(
        fake.root.join("info/puae.info"),
        "display_name = \"Commodore - Amiga (PUAE)\"\nsystemname = \"Amiga\"\n\
         supported_extensions = \"adf|hdf|lha|cue|ccd|chd|iso|m3u\"\n\
         database = \"Commodore - Amiga|Commodore - CD32|Commodore - CDTV\"\n\
         firmware_count = 2\n\
         firmware0_desc = \"kick40060.CD32 (synthetic stand-in)\"\nfirmware0_path = \"kick40060.CD32\"\nfirmware0_opt = \"false\"\n\
         firmware1_desc = \"kick40060.CD32.ext (synthetic stand-in)\"\nfirmware1_path = \"kick40060.CD32.ext\"\nfirmware1_opt = \"false\"\n",
    )
    .unwrap();
    let system = fake.root.join("system");
    fs::create_dir_all(&system).unwrap();
    for name in ["kick40060.CD32", "kick40060.CD32.ext"] {
        fs::write(system.join(name), b"synthetic stand-in, not firmware").unwrap();
    }
    let mut settings = fs::read_to_string(&fake.config).unwrap();
    settings.push_str(&format!("system_directory = \"{}\"\n", system.display()));
    fs::write(&fake.config, settings).unwrap();
    fixture
}

fn file_facts(path: &Path) -> (Vec<u8>, std::time::SystemTime, u64) {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).unwrap();
    (
        fs::read(path).unwrap(),
        metadata.modified().unwrap(),
        metadata.ino(),
    )
}

#[test]
fn a_zipped_cd32_disc_is_diagnosed_prepared_and_started_from_blue() {
    let owned = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(owned.path()).unwrap().join("profile");
    let fixture = profile(&root);
    let zip = zip_path(&root);
    let before = file_facts(&zip);
    let database_before = fs::read(&fixture.database_path).unwrap();
    let record = root.join("retroarch/record");
    assert!(!record.join("args.txt").exists());
    assert!(!root.join("data/prepared-media").exists());

    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD, "--nocapture", "--test-threads=1"])
        .env_clear()
        .envs(fixture.child_environment())
        .env("EMUWIZ_TEST_REAL_ENVIRONMENT", "1")
        .env("RUST_MIN_STACK", "8388608")
        .env("XDG_CONFIG_HOME", root.join("retroarch/config"))
        .env("PATH", root.join("retroarch/bin"))
        .env(ROOT, &root)
        .env(MARK, "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("journey prepared-disc: ok"), "{text}");
    println!("{text}");

    // The original and the library are exactly as they were.
    assert_eq!(file_facts(&zip), before, "the ZIP changed");
    assert_eq!(
        fs::read(&fixture.database_path).unwrap(),
        database_before,
        "the library database changed"
    );

    // What RetroArch was handed: the prepared cue sheet, never the ZIP.
    let args: Vec<String> = fs::read_to_string(record.join("args.txt"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let store = archivefs_core::recovery::PreparedMediaStore::at(root.join("data/prepared-media"));
    let prepared = store
        .prepared_for_source(&zip)
        .expect("a verified prepared copy is linked to the ZIP");
    assert_eq!(prepared.platform_id, "AmigaCD32");
    assert_eq!(prepared.file_count, 3);
    assert!(
        args.iter()
            .any(|arg| Path::new(arg) == prepared.launch_path),
        "{args:?}"
    );
    assert!(
        args.iter().all(|arg| Path::new(arg) != zip),
        "the ZIP itself was handed to the emulator: {args:?}"
    );
    assert_core(&args, &root.join("retroarch/cores/puae_libretro.so"));
    for (name, bytes) in disc_members() {
        assert_eq!(
            fs::read(prepared.directory.join("media").join(&name)).unwrap(),
            bytes,
            "{name}"
        );
    }
    assert!(
        store
            .verify_full(&prepared.set_id, &std::sync::atomic::AtomicBool::new(false))
            .is_ok()
    );

    // Undo removes exactly the prepared copy; the ZIP stays.
    let removal = store.remove(&prepared.set_id).unwrap();
    assert_eq!(removal.removed_files.len(), 3);
    assert!(removal.kept.is_empty());
    assert!(store.prepared_for_source(&zip).is_none());
    assert_eq!(file_facts(&zip), before);
}

#[test]
fn child_entry() {
    let Some(root) = std::env::var_os(MARK).and_then(|_| std::env::var_os(ROOT)) else {
        return;
    };
    let root = PathBuf::from(root);
    let fixture = VerifiedGameFixture::reopen(&root).unwrap();
    let zip = zip_path(&root);
    if let Ok(scenario) = std::env::var(SCENARIO) {
        let zip = current_zip(&root);
        if scenario == "rename" {
            rename_and_undo_child(&fixture, &root, &zip);
        } else if scenario == "organise" {
            organise_move_child(&fixture, &root, &zip);
        } else {
            remembered_choice_child(&fixture, &root, &zip, &scenario);
        }
        return;
    }

    // Blue on the game's own page.
    let context = eframe::egui::Context::default();
    context.style_mut(|style| style.animation_time = 0.0);
    let mut app = super::super::tests::fixture(&context);
    app.simple.enabled = true;
    app.library = Arc::new(super::super::verified_game_fixture_tests::load(&fixture));
    let game = app
        .library
        .games
        .iter()
        .find(|game| game.archive.absolute_path == zip)
        .expect("the zipped disc is in the Library")
        .archive
        .id;
    app.router.current = Route::Game(game);
    let mut blue = Blue {
        context,
        app,
        game,
        last: Vec::new(),
        seen: Vec::new(),
    };

    // 1. Blocked, and the page offers recovery.
    blue.wait_until(|b| b.shown("Open game recovery"), 45);
    blue.wait_until(|b| b.shown("EmuWiz can prepare a playable copy"), 45);
    assert!(!blue.shown(PLAY), "{:?}", blue.last);
    assert!(
        crate::emulator_setup::prepared_media::for_game(&zip, Some("AmigaCD32")).is_none(),
        "nothing is prepared yet"
    );

    // 2. Diagnosis with evidence, on the recovery page.
    blue.click("Open game recovery");
    assert_eq!(
        blue.app.router.current,
        Route::Tool(super::super::routes::ToolPage::GameRecovery)
    );
    blue.wait_until(|b| b.shown("This CD game is stored inside a ZIP"), 45);
    assert!(blue.shown("Creates new files"), "{:?}", blue.last);
    assert!(blue.shown("Stops the game starting"), "{:?}", blue.last);
    assert!(
        blue.shown("RetroArch's PUAE core is installed"),
        "{:?}",
        blue.last
    );
    assert!(
        blue.shown("The CD32 firmware is in place"),
        "{:?}",
        blue.last
    );
    assert!(
        blue.shown("The system is known only from the folder"),
        "{:?}",
        blue.last
    );
    let report = blue.app.tools.game_recovery.report().unwrap().clone();
    assert_eq!(
        report.verdict,
        archivefs_core::recovery::RecoveryVerdict::FixableByPreparation
    );

    // 3. Nothing happens without the confirmation tick.
    blue.click("Prepare a playable copy");
    for _ in 0..5 {
        blue.paint();
    }
    assert!(blue.app.tools.game_recovery.is_idle());
    assert!(!root.join("data/prepared-media").exists());

    // 4. Confirmed preparation.
    blue.click("Yes, create these new files. My game file stays as it is.");
    blue.click("Prepare a playable copy");
    blue.wait_until(|b| b.shown("Ready from a prepared copy"), 60);
    assert!(
        blue.saw("A copy was prepared and checked against the ZIP"),
        "{:?}",
        blue.last
    );
    let copy = crate::emulator_setup::prepared_media::for_game(&zip, Some("AmigaCD32"))
        .expect("readiness can now find the prepared copy");
    assert_eq!(copy.platform_id, "AmigaCD32");
    assert!(
        copy.launch_path
            .starts_with(root.join("data/prepared-media"))
    );

    // 5. The Launch page: the folder-only system still needs the person.
    blue.click("Open the Launch page");
    assert_eq!(
        blue.app.router.current,
        Route::Task {
            section: Section::Launch,
            game
        }
    );
    blue.wait_until(|b| b.shown(TRY_ONCE), 60);
    assert!(
        !blue.shown(PLAY),
        "Play before confirmation: {:?}",
        blue.last
    );
    assert!(
        !blue
            .last
            .iter()
            .any(|text| text.starts_with("Try once as ") && text != TRY_ONCE),
        "a prepared CD32 disc is offered for CD32 only: {:?}",
        blue.last
    );
    blue.app
        .native_workflows
        .as_mut()
        .expect("the Launch page created its workflows")
        .app
        .launch_retroarch
        .use_probe(Arc::new(TestProbe));
    blue.guard_executables(&root);

    // 6. One try, then Play through the real planner, preflight and spawn.
    blue.click(TRY_ONCE);
    blue.wait_until(|b| b.shown("Launch recipe"), 45);
    // The plan names the prepared cue sheet, and says the system is a try.
    assert!(
        blue.shown(copy.launch_path.to_str().unwrap()),
        "{:?}",
        blue.last
    );
    assert!(
        blue.last
            .iter()
            .any(|text| text.starts_with("System not confirmed")),
        "{:?}",
        blue.last
    );
    // The page is long; `click` scrolls to the Play button.
    blue.click(PLAY);
    // The try is consumed by the attempt, so the page returns to asking;
    // what matters is what the emulator was handed.
    let recorded = root.join("retroarch/record/args.txt");
    blue.wait_until(|_| recorded.exists(), 45);
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(100));
        blue.paint();
    }
    assert!(!blue.saw("could not be launched"), "{:?}", blue.seen);
    assert!(
        blue.shown(TRY_ONCE),
        "a one-time try must not outlive its launch: {:?}",
        blue.last
    );
    let args: Vec<String> = fs::read_to_string(&recorded)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert!(
        args.iter().any(|arg| Path::new(arg) == copy.launch_path),
        "{args:?}"
    );
    println!("journey prepared-disc: ok");
}

// ---------------------------------------------------------------------------
// "Remember this launch choice" with a prepared copy
// ---------------------------------------------------------------------------

const REMEMBER: &str = "Remember this launch choice";
const REMEMBER_AS: &str = "Remember as Amiga CD32";
const REMEMBERED_NOTE: &str = "Remembered launch choice: this file starts as Amiga CD32 while it is unchanged. This is your choice, not game identification.";
const REMEMBERED_WARNING: &str = "System not confirmed: you chose to remember Amiga CD32 as the launch choice for this unchanged file.";

/// A headless Blue on the zipped disc's Launch page, in this process.
fn blue_on_launch_page(fixture: &VerifiedGameFixture, zip: &Path) -> Blue {
    let context = eframe::egui::Context::default();
    context.style_mut(|style| style.animation_time = 0.0);
    let mut app = super::super::tests::fixture(&context);
    app.simple.enabled = true;
    app.library = Arc::new(super::super::verified_game_fixture_tests::load(fixture));
    let game = app
        .library
        .games
        .iter()
        .find(|game| game.archive.absolute_path == zip)
        .expect("the zipped disc is in the Library")
        .archive
        .id;
    app.router.current = Route::Task {
        section: Section::Launch,
        game,
    };
    let mut blue = Blue {
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

/// Where `label` is on the page, scrolling through it like a person would
/// (text outside the visible area is not painted, so one frame cannot say).
fn find_by_scrolling(blue: &mut Blue, label: &str) -> Option<eframe::egui::Pos2> {
    for attempt in 0..18 {
        let layout = blue.paint();
        if let Some(rect) = bounds(&layout, label)
            .into_iter()
            .find(|rect| rect.min.y > 64.0)
        {
            return Some(rect.center());
        }
        let mut raw = input(None, SIZE);
        raw.events = vec![
            eframe::egui::Event::PointerMoved(eframe::egui::pos2(SIZE[0] * 0.7, SIZE[1] - 150.0)),
            eframe::egui::Event::MouseWheel {
                phase: eframe::egui::TouchPhase::Move,
                unit: eframe::egui::MouseWheelUnit::Point,
                delta: eframe::egui::vec2(0.0, if attempt == 0 { 10000.0 } else { -300.0 }),
                modifiers: eframe::egui::Modifiers::NONE,
            },
        ];
        let _ = blue.context.run(raw, |ctx| blue.app.show(ctx));
        blue.paint();
    }
    None
}

/// Presses Play if the page offers it anywhere, and says whether it did.
fn press_play_if_offered(blue: &mut Blue) -> bool {
    match find_by_scrolling(blue, PLAY) {
        Some(point) => {
            blue.click_at(point);
            true
        }
        None => false,
    }
}

fn recorded_arguments(root: &Path) -> Option<Vec<String>> {
    fs::read_to_string(root.join("retroarch/record/args.txt"))
        .ok()
        .map(|text| text.lines().map(str::to_string).collect())
}

/// One launch from a page that already shows the plan; returns what the
/// emulator was handed.
fn play_and_collect(blue: &mut Blue, root: &Path) -> Vec<String> {
    let recorded = root.join("retroarch/record/args.txt");
    let _ = fs::remove_file(&recorded);
    blue.click(PLAY);
    blue.wait_until(|_| recorded.exists(), 45);
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(100));
        blue.paint();
    }
    assert!(!blue.saw("could not be launched"), "{:?}", blue.seen);
    recorded_arguments(root).unwrap()
}

fn remembered_choice_child(fixture: &VerifiedGameFixture, root: &Path, zip: &Path, scenario: &str) {
    let copy = crate::emulator_setup::prepared_media::for_game(zip, Some("AmigaCD32"));
    // The game page's Amiga card answers from the same lookup, on real files.
    let card = crate::gui_v2::amiga_game::prepared_copy(zip, "AmigaCD32");
    let mut blue = blue_on_launch_page(fixture, zip);
    match scenario {
        // Tick, remember, play twice in one session.
        "remember" => {
            let copy = copy.expect("the parent prepared the copy");
            assert_eq!(card, crate::gui_v2::amiga_game::PreparedCopy::Ready);
            blue.wait_until(|b| b.shown(TRY_ONCE), 60);
            blue.guard_executables(root);
            blue.click(REMEMBER);
            blue.wait_until(|b| b.shown(REMEMBER_AS), 15);
            assert!(!blue.shown(TRY_ONCE), "{:?}", blue.last);
            blue.click(REMEMBER_AS);
            blue.wait_until(|b| b.shown("Launch recipe"), 45);
            assert!(
                find_by_scrolling(&mut blue, REMEMBERED_NOTE).is_some(),
                "{:?}",
                blue.seen
            );
            assert!(
                blue.seen
                    .iter()
                    .any(|text| text.starts_with(REMEMBERED_WARNING)),
                "{:?}",
                blue.last
            );
            for round in 0..2 {
                let args = play_and_collect(&mut blue, root);
                assert!(
                    args.iter().any(|arg| Path::new(arg) == copy.launch_path),
                    "launch {round}: {args:?}"
                );
                // Unlike a one-time try, the remembered choice is still there.
                assert!(
                    find_by_scrolling(&mut blue, REMEMBERED_NOTE).is_some(),
                    "{:?}",
                    blue.seen
                );
                blue.wait_until(|b| b.saw("Launch recipe"), 45);
                assert!(find_by_scrolling(&mut blue, TRY_ONCE).is_none());
            }
        }
        // A fresh process: nothing to tick, the plan is there, Play works.
        "restart" => {
            let copy = copy.expect("the copy survives a restart");
            blue.wait_until(|b| b.shown("Launch recipe"), 60);
            blue.guard_executables(root);
            assert!(
                find_by_scrolling(&mut blue, REMEMBERED_NOTE).is_some(),
                "{:?}",
                blue.seen
            );
            assert!(!blue.shown(TRY_ONCE), "{:?}", blue.last);
            assert!(
                blue.seen
                    .iter()
                    .any(|text| text.starts_with(REMEMBERED_WARNING)),
                "the unconfirmed-system warning must stay: {:?}",
                blue.last
            );
            let args = play_and_collect(&mut blue, root);
            assert!(
                args.iter().any(|arg| Path::new(arg) == copy.launch_path),
                "{args:?}"
            );
        }
        // The person chose the system (a saved manual choice): no try, no
        // remembered entry, the "game not identified" warning, Play works.
        "chosen-launch" => {
            let copy = copy.expect("a prepared copy for the chosen game");
            assert_eq!(card, crate::gui_v2::amiga_game::PreparedCopy::Ready);
            blue.wait_until(|b| b.shown("Launch recipe"), 60);
            blue.guard_executables(root);
            assert!(
                blue.seen
                    .iter()
                    .any(|text| text.starts_with("Game not identified: you chose Amiga CD32")),
                "{:?}",
                blue.last
            );
            assert!(find_by_scrolling(&mut blue, TRY_ONCE).is_none());
            let args = play_and_collect(&mut blue, root);
            assert!(
                args.iter().any(|arg| Path::new(arg) == copy.launch_path),
                "{args:?}"
            );
        }
        // Something the remembered choice must not paper over. The page is
        // given time to settle, Play is pressed wherever it is offered, and
        // the emulator must never be reached.
        blocked => {
            let recorded = root.join("retroarch/record/args.txt");
            let _ = fs::remove_file(&recorded);
            blue.wait_until(|b| b.shown("Play / Launch readiness"), 60);
            let mut pressed = 0;
            for _ in 0..12 {
                for _ in 0..5 {
                    std::thread::sleep(Duration::from_millis(100));
                    blue.paint();
                }
                if press_play_if_offered(&mut blue) {
                    pressed += 1;
                }
            }
            for _ in 0..20 {
                std::thread::sleep(Duration::from_millis(100));
                blue.paint();
            }
            assert!(
                !recorded.exists(),
                "{blocked}: the emulator was started: {:?}",
                recorded_arguments(root)
            );
            match blocked {
                "changed-zip" => {
                    assert!(copy.is_none(), "a changed ZIP has no prepared copy");
                    assert_eq!(card, crate::gui_v2::amiga_game::PreparedCopy::Missing);
                    assert!(find_by_scrolling(&mut blue, REMEMBERED_NOTE).is_none());
                    assert!(!blue.saw("Launch recipe"), "{:?}", blue.last);
                }
                "damaged-copy" => {
                    assert!(copy.is_none(), "a damaged copy is never used");
                    assert_eq!(card, crate::gui_v2::amiga_game::PreparedCopy::Missing);
                    assert!(!blue.saw("Launch recipe"), "{:?}", blue.last);
                }
                "missing-firmware" | "missing-core" => {
                    assert!(copy.is_some(), "the copy itself is fine");
                }
                other => panic!("unknown scenario {other}"),
            }
            println!(
                "journey prepared-disc {blocked}: Play pressed {pressed} time(s); settled frame: {:?}",
                blue.last
            );
        }
    }
    println!("journey prepared-disc {scenario}: ok");
}

/// The one zipped disc in the collection, whatever it is called now.
fn current_zip(root: &Path) -> PathBuf {
    fn walk(folder: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "zip") {
                found.push(path);
            }
        }
    }
    let mut zips = Vec::new();
    walk(&root.join("collection"), &mut zips);
    assert_eq!(zips.len(), 1, "{zips:?}");
    zips.pop().unwrap()
}

/// "Organise my games" for a game whose system the person chose, sitting
/// outside that system's folder: the production plan, prepare and apply, in
/// a process with the profile's own environment. The prepared copy has to
/// arrive with the file.
fn organise_move_child(fixture: &VerifiedGameFixture, root: &Path, zip: &Path) {
    use archivefs_core::folder_platform_check::organise::{
        ChosenGame, apply, plan_organisation_with_choices, prepare,
    };
    let journal = fixture.data_dir.join("rename-transactions-sealed-v1");
    let never = std::sync::atomic::AtomicBool::new(false);
    let store = archivefs_core::recovery::PreparedMediaStore::for_profile().unwrap();
    assert!(store.root().starts_with(root), "{}", store.root().display());
    let copy = store
        .prepared_for_source(zip)
        .expect("prepared before the move");
    let database = archivefs_core::Database::open_existing(&fixture.database_path).unwrap();
    let archive_id = database
        .find_archive_id_by_absolute_path(zip)
        .unwrap()
        .expect("the game is in the library");
    let chosen = [ChosenGame {
        archive_id,
        path: zip.to_path_buf(),
        platform: "AmigaCD32".into(),
    }];
    let roots = [root.join("collection")];
    let mut plans = plan_organisation_with_choices(&[], &chosen, &[], &roots, 1);
    assert_eq!(plans.len(), 1, "{plans:?}");
    let plan = &mut plans[0];
    prepare(plan, &database).unwrap();
    let approved: std::collections::BTreeSet<String> = plan
        .plan
        .suggested()
        .map(|entry| entry.source_path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(approved.len(), 1, "{:?}", plan.left_alone);
    apply(plan, &approved, &database, &journal, &never)
        .map_err(|(error, _)| error.to_string())
        .unwrap();
    drop(database);
    let moved = current_zip(root);
    assert_ne!(moved, zip, "the game was moved");
    assert!(!zip.exists());
    assert_eq!(store.prepared_for_source(&moved), Some(copy));
    println!(
        "journey prepared-disc organise: ok ({} -> {})",
        zip.display(),
        moved.display()
    );
}

/// Blue's Rename, through the functions its page calls, in a process with the
/// profile's own environment: rename, Undo, rename again. After each step the
/// prepared copy and the remembered choice are with the file.
fn rename_and_undo_child(fixture: &VerifiedGameFixture, root: &Path, zip: &Path) {
    use archivefs_core::dat::rom_organisation::game_rename::{
        RetroArchSaveLocations, apply_game_rename, preview_game_rename,
    };
    use archivefs_core::launch::remembered_launch_system::remembered_launch_system;
    let journal = fixture.data_dir.join("rename-transactions-sealed-v1");
    let saves = RetroArchSaveLocations::default();
    let never = std::sync::atomic::AtomicBool::new(false);
    let store = archivefs_core::recovery::PreparedMediaStore::for_profile().unwrap();
    assert!(store.root().starts_with(root), "{}", store.root().display());
    let copy = store
        .prepared_for_source(zip)
        .expect("prepared before the rename");
    assert_eq!(remembered_launch_system(zip), Some("AmigaCD32"));
    let rename = |generation: u64| {
        let database = archivefs_core::Database::open_existing(&fixture.database_path).unwrap();
        let preview = preview_game_rename(
            &database,
            zip,
            "Renamed Disc (1994)(Test)",
            generation,
            &saves,
        )
        .unwrap();
        apply_game_rename(&preview, &database, &journal, &never, &saves).unwrap()
    };
    let renamed = zip.with_file_name("Renamed Disc (1994)(Test).zip");

    let mut outcome = rename(1);
    assert!(renamed.is_file() && !zip.exists());
    assert_eq!(store.prepared_for_source(&renamed), Some(copy.clone()));
    assert!(store.prepared_for_source(zip).is_none());
    assert_eq!(remembered_launch_system(&renamed), Some("AmigaCD32"));

    // History Undo.
    archivefs_core::dat::rom_organisation::rollback_organisation_transaction(
        &mut outcome.transaction,
        &journal,
        &never,
        &root.join("collection"),
    )
    .unwrap();
    assert!(zip.is_file() && !renamed.exists());
    assert_eq!(store.prepared_for_source(zip), Some(copy.clone()));
    assert_eq!(remembered_launch_system(zip), Some("AmigaCD32"));
    assert_eq!(remembered_launch_system(&renamed), None);

    // Renamed again: the state the following launches start from.
    rename(2);
    assert_eq!(store.prepared_for_source(&renamed), Some(copy));
    assert_eq!(remembered_launch_system(&renamed), Some("AmigaCD32"));
    println!("journey prepared-disc rename: ok");
}

fn no_processes(root: &Path) -> PathBuf {
    let empty = root.join("no-processes");
    fs::create_dir_all(&empty).unwrap();
    empty
}

fn run_scenario(root: &Path, fixture: &VerifiedGameFixture, scenario: &str) -> String {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD, "--nocapture", "--test-threads=1"])
        .env_clear()
        .envs(fixture.child_environment())
        .env("EMUWIZ_TEST_REAL_ENVIRONMENT", "1")
        .env("RUST_MIN_STACK", "8388608")
        .env("XDG_CONFIG_HOME", root.join("retroarch/config"))
        .env("PATH", root.join("retroarch/bin"))
        .env(ROOT, root)
        .env(MARK, "1")
        // The standard test isolation of the writer gate: an empty private
        // process table, so a real EmuWiz running on this machine neither
        // blocks these renames nor is ever inspected by them.
        .env(
            archivefs_core::dat::rename_apply::writer_gate::PROC_ROOT_ENV,
            no_processes(root),
        )
        .env(SCENARIO, scenario)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "scenario {scenario} failed:\n{text}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains(&format!("journey prepared-disc {scenario}: ok")),
        "{text}"
    );
    text
}

/// "Remember this launch choice" with a prepared copy: it is saved for the
/// unchanged ZIP only, survives later launches and a restart, keeps the
/// unconfirmed-system warning, and never carries a launch past a missing
/// firmware file, a missing core, a damaged copy or a changed ZIP.
#[test]
fn a_remembered_choice_for_a_prepared_disc_survives_restarts_and_bypasses_no_check() {
    use archivefs_core::launch::remembered_launch_system as remembered;
    use archivefs_core::recovery::{DiscSetLimits, PreparedMediaStore};
    let owned = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(owned.path()).unwrap().join("profile");
    let fixture = profile(&root);
    let zip = zip_path(&root);
    let store = PreparedMediaStore::at(root.join("data/prepared-media"));
    let never = std::sync::atomic::AtomicBool::new(false);
    let prepare = |zip: &Path| {
        store
            .prepare(
                zip,
                "AmigaCD32",
                &archivefs_core::safe_read::TrustedRoots::none(),
                &DiscSetLimits::default(),
                &never,
                &|_| {},
            )
            .unwrap()
            .summary
    };
    let summary = prepare(&zip);
    let zip_before = file_facts(&zip);
    let database_before = fs::read(&fixture.database_path).unwrap();
    let remembered_file = fixture
        .config_dir
        .join(remembered::REMEMBERED_LAUNCH_SYSTEMS_FILE);
    assert!(!remembered_file.exists());

    // Remember, and launch twice in the same session.
    run_scenario(&root, &fixture, "remember");
    let entries = remembered::load_remembered_launch_systems_at(&remembered_file).unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(
        entries[0].path, zip,
        "the choice belongs to the library file"
    );
    assert_eq!(entries[0].platform_id, "AmigaCD32");
    let saved = fs::read(&remembered_file).unwrap();

    // A new process: still remembered, still warned, still launches.
    run_scenario(&root, &fixture, "restart");
    run_scenario(&root, &fixture, "restart");
    assert_eq!(
        fs::read(&remembered_file).unwrap(),
        saved,
        "launching rewrote the choice"
    );
    assert_eq!(file_facts(&zip), zip_before);
    assert_eq!(fs::read(&fixture.database_path).unwrap(), database_before);

    // Blue's Rename (and Undo, and Rename again): the copy and the choice
    // stay with the file, and it still launches without asking.
    run_scenario(&root, &fixture, "rename");
    let zip = current_zip(&root);
    assert_eq!(
        zip.file_name().unwrap(),
        "Renamed Disc (1994)(Test).zip",
        "{}",
        zip.display()
    );
    assert_eq!(
        fs::read(&zip).unwrap(),
        zip_before.0,
        "a rename changes no byte"
    );
    let entries = remembered::load_remembered_launch_systems_at(&remembered_file).unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].path, zip);
    let saved = fs::read(&remembered_file).unwrap();
    assert_eq!(
        store.prepared_for_source(&zip).map(|copy| copy.set_id),
        Some(summary.set_id.clone())
    );
    run_scenario(&root, &fixture, "restart");
    run_scenario(&root, &fixture, "restart");

    // Renamed outside EmuWiz: the copy is found again by content on the
    // game's own page (worker), nothing is extracted, and the choice follows.
    let outside = zip.with_file_name("Renamed Outside EmuWiz.zip");
    fs::rename(&zip, &outside).unwrap();
    let moved = store
        .relink(
            &outside,
            &archivefs_core::safe_read::TrustedRoots::none(),
            &DiscSetLimits::default(),
            &never,
            &|_| {},
        )
        .unwrap();
    assert!(
        matches!(&moved, archivefs_core::recovery::RelinkOutcome::Linked { moved_from: Some(old), .. } if *old == zip),
        "{moved:?}"
    );
    assert_eq!(
        store.prepared_for_source(&outside).map(|copy| copy.set_id),
        Some(summary.set_id.clone())
    );
    fs::rename(&outside, &zip).unwrap();
    store
        .relink(
            &zip,
            &archivefs_core::safe_read::TrustedRoots::none(),
            &DiscSetLimits::default(),
            &never,
            &|_| {},
        )
        .unwrap();
    run_scenario(&root, &fixture, "restart");

    // Firmware: one required file gone.
    let firmware = root.join("retroarch/system/kick40060.CD32.ext");
    let firmware_bytes = fs::read(&firmware).unwrap();
    fs::remove_file(&firmware).unwrap();
    run_scenario(&root, &fixture, "missing-firmware");
    fs::write(&firmware, firmware_bytes).unwrap();

    // Emulator: the core gone.
    let core = root.join("retroarch/cores/puae_libretro.so");
    let parked = root.join("retroarch/puae_libretro.so.parked");
    fs::rename(&core, &parked).unwrap();
    run_scenario(&root, &fixture, "missing-core");
    fs::rename(&parked, &core).unwrap();

    // Everything back: it launches again without asking.
    run_scenario(&root, &fixture, "restart");

    // The prepared copy damaged (the ZIP untouched).
    let track = summary.directory.join("media").join(format!("{TITLE}.iso"));
    fs::set_permissions(&track, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let mut bytes = fs::read(&track).unwrap();
    bytes[30_000] ^= 1;
    fs::write(&track, bytes).unwrap();
    run_scenario(&root, &fixture, "damaged-copy");
    store.remove(&summary.set_id).unwrap();
    prepare(&zip);
    run_scenario(&root, &fixture, "restart");

    // The ZIP itself changed: neither the remembered choice nor the copy applies.
    let file = fs::OpenOptions::new().append(true).open(&zip).unwrap();
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000))
        .unwrap();
    drop(file);
    run_scenario(&root, &fixture, "changed-zip");
    assert_eq!(fs::read(&zip).unwrap(), zip_before.0);
    assert_eq!(fs::read(&remembered_file).unwrap(), saved);
}

/// "Organise my games" moves a prepared game into its system's folder: it
/// still starts from its prepared copy afterwards, with nothing extracted
/// again.
#[test]
fn an_organise_move_keeps_a_prepared_game_playable() {
    use archivefs_core::recovery::{DiscSetLimits, PreparedMediaStore};
    let owned = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(owned.path()).unwrap().join("profile");
    let fixture = profile_in(&root, "unsorted");
    let zip = current_zip(&root);
    // The person chooses the system for this game (a saved manual choice).
    {
        let mut database = archivefs_core::Database::open_existing(&fixture.database_path).unwrap();
        let id = database
            .find_archive_id_by_absolute_path(&zip)
            .unwrap()
            .unwrap();
        database.set_manual_platform(id, "AmigaCD32").unwrap();
    }
    let bytes = fs::read(&zip).unwrap();
    let store = PreparedMediaStore::at(root.join("data/prepared-media"));
    let summary = store
        .prepare(
            &zip,
            "AmigaCD32",
            &archivefs_core::safe_read::TrustedRoots::none(),
            &DiscSetLimits::default(),
            &std::sync::atomic::AtomicBool::new(false),
            &|_| {},
        )
        .unwrap()
        .summary;
    let track_inode = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&summary.launch_path).unwrap().ino()
    };

    run_scenario(&root, &fixture, "chosen-launch");
    run_scenario(&root, &fixture, "organise");
    let moved = current_zip(&root);
    assert_ne!(moved, zip);
    assert_eq!(fs::read(&moved).unwrap(), bytes, "a move changes no byte");
    assert_eq!(
        store.prepared_for_source(&moved).map(|copy| copy.set_id),
        Some(summary.set_id.clone())
    );
    // Blue, restarted, on the moved game: Play from the same copy.
    run_scenario(&root, &fixture, "chosen-launch");
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(&summary.launch_path).unwrap().ino(),
            track_inode,
            "nothing was extracted again"
        );
    }
    assert_eq!(store.list().len(), 1);
}
