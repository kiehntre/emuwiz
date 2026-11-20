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
    let fixture = VerifiedGameFixture::build_at(root).unwrap();
    let zip = zip_path(root);
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
    assert_eq!(row.platform.as_deref(), Some("AmigaCD32"), "{row:?}");
    assert_eq!(
        row.platform_source.as_deref(),
        Some("folder_alias"),
        "{row:?}"
    );

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
