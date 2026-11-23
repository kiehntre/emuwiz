use super::*;
use crate::launch::safe_launch_sandbox::CleanupOutcome;
use std::collections::BTreeMap;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A complete disposable setup: stand-in emulator, synthetic ROMs, one ATR.
struct Fixture {
    temp: TempDir,
    paths: Atari800Paths,
    setup: Atari800Setup,
    game: PathBuf,
    key: String,
}

fn atr_bytes() -> Vec<u8> {
    let mut bytes = vec![0x11; 92160 + 16];
    bytes[..16].fill(0);
    bytes[..2].copy_from_slice(&[0x96, 0x02]);
    bytes[2..4].copy_from_slice(&5760u16.to_le_bytes());
    bytes[4..6].copy_from_slice(&128u16.to_le_bytes());
    bytes
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let executable = root.join("bin/atari800");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        write_script(&executable, "exit 0");
        let os_xl = root.join("roms/xl.rom");
        let os_b = root.join("roms/osb.rom");
        let basic = root.join("roms/basic.rom");
        std::fs::create_dir_all(root.join("roms")).unwrap();
        std::fs::write(&os_xl, vec![0x31; 16384]).unwrap();
        std::fs::write(&os_b, vec![0x32; 10240]).unwrap();
        std::fs::write(&basic, vec![0x33; 8192]).unwrap();
        let game = root.join("games/Homebrew Disk.atr");
        std::fs::create_dir_all(root.join("games")).unwrap();
        std::fs::write(&game, atr_bytes()).unwrap();
        let key = hex(&Sha256::digest(atr_bytes()));
        let setup = Atari800Setup {
            version: 1,
            executable: Some(executable),
            machine: Some(Atari800Machine::Atari800Xl),
            video: Some(Atari800Video::Pal),
            os_400_800: Some(bind_rom(&os_b, Atari800RomKind::Os400800).unwrap()),
            os_xl_xe: Some(bind_rom(&os_xl, Atari800RomKind::OsXlXe).unwrap()),
            basic: Some(bind_rom(&basic, Atari800RomKind::Basic).unwrap()),
            basic_enabled: false,
            accept_present_unverified_roms: true,
            disposable_session_acknowledged: true,
        };
        let paths = Atari800Paths {
            setup_file: root.join("config/atari800_setup.json"),
            seed_directory: root.join("data/atari800"),
        };
        let fixture = Self {
            temp,
            paths,
            setup,
            game,
            key,
        };
        fixture.save();
        fixture
    }
    fn root(&self) -> PathBuf {
        self.temp.path().canonicalize().unwrap()
    }
    fn save(&self) {
        save_setup(&self.paths.setup_file, &self.setup).unwrap();
    }
    fn offered(&self) -> CapturedFileIdentity {
        CapturedFileIdentity::capture(&std::fs::symlink_metadata(&self.game).unwrap())
    }
    fn profile_id(&self) -> String {
        resolve_setup(&self.setup, None).profile_id().unwrap()
    }
    fn launch(&self) -> Result<LaunchedAtari800, Atari800LaunchRefusal> {
        self.launch_offered(&self.profile_id(), &self.offered())
    }
    fn launch_offered(
        &self,
        profile_id: &str,
        offered: &CapturedFileIdentity,
    ) -> Result<LaunchedAtari800, Atari800LaunchRefusal> {
        let base = self.root().join("run");
        std::fs::create_dir_all(&base).unwrap();
        launch_selected_atr(
            &self.paths,
            None,
            profile_id,
            &self.game,
            &self.key,
            offered,
            || SandboxManager::open(&base),
        )
    }
    fn workspaces(&self) -> usize {
        std::fs::read_dir(self.root().join("run/emuwiz/launch"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .count()
            })
            .unwrap_or(0)
    }
    /// Every original (game, ROMs, emulator) by content, to prove no writes.
    fn originals(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let root = self.root();
        [
            "bin/atari800",
            "roms/xl.rom",
            "roms/osb.rom",
            "roms/basic.rom",
        ]
        .iter()
        .map(|leaf| root.join(leaf))
        .chain([self.game.clone()])
        .map(|path| (path.clone(), std::fs::read(&path).unwrap()))
        .collect()
    }
}

fn wait(launched: &mut LaunchedAtari800) -> Option<i32> {
    let until = Instant::now() + Duration::from_secs(10);
    while launched.process.process.poll().is_none() {
        assert!(Instant::now() < until, "stand-in emulator timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
    let report = launched.process.process.poll().unwrap();
    report.status.as_ref().ok().and_then(|status| status.code())
}

fn wait_for_outcome(launched: &LaunchedAtari800, wanted: impl Fn(&CleanupOutcome) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !wanted(&launched.process.cleanup_outcome()) {
        assert!(Instant::now() < until, "cleanup outcome timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn refusal(result: Result<LaunchedAtari800, Atari800LaunchRefusal>) -> String {
    match result {
        Ok(_) => panic!("launch must be refused"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn an_empty_setup_chooses_nothing_and_names_every_missing_choice() {
    let resolution = resolve_setup(&Atari800Setup::default(), None);
    assert!(!resolution.eligible());
    assert_eq!(resolution.firmware, FirmwareReadiness::Unknown);
    assert!(
        resolution.standalone_input().is_none(),
        "nothing found and nothing configured: no candidate"
    );
    for blocker in [
        Atari800SetupBlocker::EmulatorMissing,
        Atari800SetupBlocker::MachineNotChosen,
        Atari800SetupBlocker::VideoNotChosen,
        Atari800SetupBlocker::DisposableSessionNotAcknowledged,
    ] {
        assert!(resolution.blockers.contains(&blocker), "{blocker:?}");
    }
}

#[test]
fn a_complete_setup_is_eligible_with_present_unverified_firmware() {
    let fixture = Fixture::new();
    let resolution = resolve_setup(&fixture.setup, None);
    assert!(resolution.eligible(), "{:?}", resolution.blockers);
    assert_eq!(resolution.firmware, FirmwareReadiness::PresentUnverified);
    let input = resolution.standalone_input().unwrap();
    assert_eq!(input.adapter_id, "atari800");
    assert!(input.eligible);
    assert_eq!(input.firmware, FirmwareReadiness::PresentUnverified);
}

#[test]
fn discovery_uses_the_only_path_emulator_and_refuses_to_pick_between_two() {
    let mut fixture = Fixture::new();
    let root = fixture.root();
    fixture.setup.executable = None;
    let one = std::env::join_paths([root.join("bin")]).unwrap();
    let resolution = resolve_setup(&fixture.setup, Some(&one));
    assert_eq!(resolution.executable, Some(root.join("bin/atari800")));
    std::fs::create_dir_all(root.join("other")).unwrap();
    write_script(&root.join("other/atari800"), "exit 0");
    let two = std::env::join_paths([root.join("bin"), root.join("other")]).unwrap();
    let resolution = resolve_setup(&fixture.setup, Some(&two));
    assert_eq!(resolution.executable, None);
    assert!(matches!(
        resolution.blockers.first(),
        Some(Atari800SetupBlocker::EmulatorAmbiguous(found)) if found.len() == 2
    ));
    // Found but not selected: offered as a blocked candidate, never eligible.
    assert!(!resolution.standalone_input().unwrap().eligible);
}

#[test]
fn a_symlinked_or_missing_selected_emulator_is_refused() {
    let mut fixture = Fixture::new();
    let root = fixture.root();
    symlink(root.join("bin/atari800"), root.join("bin/link")).unwrap();
    fixture.setup.executable = Some(root.join("bin/link"));
    assert!(matches!(
        resolve_setup(&fixture.setup, None).blockers.first(),
        Some(Atari800SetupBlocker::EmulatorUnsafe(_))
    ));
    fixture.setup.executable = Some(root.join("bin/gone"));
    assert_eq!(
        resolve_setup(&fixture.setup, None).blockers.first(),
        Some(&Atari800SetupBlocker::EmulatorMissing)
    );
}

#[test]
fn rom_binding_checks_role_size_and_refuses_symlinks() {
    let fixture = Fixture::new();
    let root = fixture.root();
    // An XL ROM cannot be bound as a 400/800 OS or as BASIC.
    for kind in [Atari800RomKind::Os400800, Atari800RomKind::Basic] {
        let error = bind_rom(&root.join("roms/xl.rom"), kind).unwrap_err();
        assert!(error.contains("needs exactly"), "{error}");
    }
    symlink(root.join("roms/xl.rom"), root.join("roms/link.rom")).unwrap();
    assert!(bind_rom(&root.join("roms/link.rom"), Atari800RomKind::OsXlXe).is_err());
    assert!(bind_rom(Path::new("roms/xl.rom"), Atari800RomKind::OsXlXe).is_err());
    assert!(bind_rom(&root.join("roms/none.rom"), Atari800RomKind::OsXlXe).is_err());
    let bound = bind_rom(&root.join("roms/xl.rom"), Atari800RomKind::OsXlXe).unwrap();
    assert_eq!(bound.sha256, hex(&Sha256::digest(vec![0x31; 16384])));
}

#[test]
fn missing_changed_and_wrong_firmware_are_distinct_blockers() {
    let mut fixture = Fixture::new();
    let xl = fixture.root().join("roms/xl.rom");
    // Same size, different bytes: the binding no longer matches.
    std::fs::write(&xl, vec![0x77; 16384]).unwrap();
    let changed = resolve_setup(&fixture.setup, None);
    assert_eq!(changed.firmware, FirmwareReadiness::Missing);
    assert!(matches!(
        changed.blockers.first(),
        Some(Atari800SetupBlocker::OsRomChanged(_))
    ));
    std::fs::remove_file(&xl).unwrap();
    assert!(matches!(
        resolve_setup(&fixture.setup, None).blockers.first(),
        Some(Atari800SetupBlocker::OsRomMissing(_))
    ));
    fixture.setup.os_xl_xe = None;
    assert_eq!(
        resolve_setup(&fixture.setup, None).blockers.first(),
        Some(&Atari800SetupBlocker::OsRomNotBound(
            Atari800RomKind::OsXlXe
        ))
    );
    // The 400/800 machine needs its own ROM; the XL binding never stands in.
    fixture.setup.machine = Some(Atari800Machine::Atari400800);
    assert!(resolve_setup(&fixture.setup, None).eligible());
    fixture.setup.os_400_800 = None;
    assert_eq!(
        resolve_setup(&fixture.setup, None).blockers.first(),
        Some(&Atari800SetupBlocker::OsRomNotBound(
            Atari800RomKind::Os400800
        ))
    );
}

#[test]
fn basic_is_only_required_when_switched_on() {
    let mut fixture = Fixture::new();
    fixture.setup.basic = None;
    assert!(resolve_setup(&fixture.setup, None).eligible());
    fixture.setup.basic_enabled = true;
    let resolution = resolve_setup(&fixture.setup, None);
    assert_eq!(resolution.firmware, FirmwareReadiness::Missing);
    assert_eq!(
        resolution.blockers.first(),
        Some(&Atari800SetupBlocker::BasicRomNotBound)
    );
}

#[test]
fn neither_consent_is_ever_assumed() {
    let mut fixture = Fixture::new();
    fixture.setup.accept_present_unverified_roms = false;
    fixture.save();
    assert!(refusal(fixture.launch()).contains("cannot verify their contents"));
    fixture.setup.accept_present_unverified_roms = true;
    fixture.setup.disposable_session_acknowledged = false;
    fixture.save();
    assert!(refusal(fixture.launch()).contains("Temporary Atari800 session"));
    assert_eq!(fixture.workspaces(), 0, "a refused launch prepares nothing");
}

#[test]
fn setup_round_trips_and_a_missing_file_is_an_empty_setup() {
    let fixture = Fixture::new();
    assert_eq!(
        load_setup(&fixture.paths.setup_file).unwrap(),
        fixture.setup
    );
    assert_eq!(
        load_setup(&fixture.root().join("config/absent.json")).unwrap(),
        Atari800Setup::default()
    );
    std::fs::write(&fixture.paths.setup_file, b"{\"surprise\":1}").unwrap();
    assert!(load_setup(&fixture.paths.setup_file).is_err());
    std::fs::write(&fixture.paths.setup_file, b"{\"version\":99}").unwrap();
    assert!(load_setup(&fixture.paths.setup_file).is_err());
    // A directory or symlink in the setup's place is never replaced.
    let odd = fixture.root().join("config/odd.json");
    std::fs::create_dir(&odd).unwrap();
    assert!(save_setup(&odd, &fixture.setup).is_err());
}

#[test]
fn the_seed_is_written_exactly_and_repaired_without_touching_user_config() {
    let fixture = Fixture::new();
    let home_config = fixture.root().join(".atari800.cfg");
    std::fs::write(&home_config, b"USER SETTINGS\n").unwrap();
    let directory = &fixture.paths.seed_directory;
    let off = ensure_isolated_seed(directory, Atari800Machine::Atari800Xl, false).unwrap();
    let on = ensure_isolated_seed(directory, Atari800Machine::Atari800Xl, true).unwrap();
    assert_ne!(off, on);
    assert_eq!(
        std::fs::read_to_string(&off).unwrap(),
        isolated_config_seed(Atari800Machine::Atari800Xl, false)
    );
    std::fs::write(&off, b"tampered").unwrap();
    ensure_isolated_seed(directory, Atari800Machine::Atari800Xl, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(&off).unwrap(),
        isolated_config_seed(Atari800Machine::Atari800Xl, false)
    );
    assert_eq!(std::fs::read(&home_config).unwrap(), b"USER SETTINGS\n");
    // A symlinked seed folder is refused, not followed.
    let link = fixture.root().join("data/link");
    symlink(directory, &link).unwrap();
    assert!(ensure_isolated_seed(&link, Atari800Machine::Atari800Xl, false).is_err());
}

#[test]
fn a_launch_runs_from_scratch_copies_and_cleans_up_without_writing_originals() {
    let fixture = Fixture::new();
    let root = fixture.root();
    // The stand-in records its argv and scribbles on everything it was given.
    write_script(
        &root.join("bin/atari800"),
        &format!(
            "printf '%s\\n' \"$@\" > '{}'\nfor a in \"$@\"; do [ -f \"$a\" ] && echo x >> \"$a\"; done\nexit 0",
            root.join("argv.txt").display()
        ),
    );
    let mut fixture = fixture;
    fixture.setup.executable = Some(root.join("bin/atari800"));
    fixture.save();
    let before = fixture.originals();
    let mut launched = fixture.launch().unwrap();
    assert!(
        launched
            .workspace
            .starts_with(root.join("run/emuwiz/launch"))
    );
    assert_eq!(wait(&mut launched), Some(0));
    wait_for_outcome(&launched, |outcome| *outcome == CleanupOutcome::Removed);
    assert_eq!(fixture.workspaces(), 0);
    assert_eq!(fixture.originals(), before, "originals are never written");
    let argv = std::fs::read_to_string(root.join("argv.txt")).unwrap();
    for original in before.keys() {
        assert!(
            !argv.contains(original.to_str().unwrap()),
            "an original path reached the emulator: {argv}"
        );
    }
    for flag in [
        "-config",
        "-no-autosave-config",
        "-xl",
        "-pal",
        "-nobasic",
        "-xlxe_rom",
    ] {
        assert!(
            argv.lines().any(|line| line == flag),
            "{flag} missing: {argv}"
        );
    }
}

#[test]
fn machine_video_and_basic_choices_reach_the_command() {
    for (machine, video, basic, flags) in [
        (
            Atari800Machine::Atari400800,
            Atari800Video::Ntsc,
            false,
            ["-atari", "-ntsc", "-osb_rom", "-nobasic"],
        ),
        (
            Atari800Machine::Atari130Xe,
            Atari800Video::Pal,
            true,
            ["-xe", "-pal", "-xlxe_rom", "-basic"],
        ),
    ] {
        let mut fixture = Fixture::new();
        let root = fixture.root();
        write_script(
            &root.join("bin/atari800"),
            &format!(
                "printf '%s\\n' \"$@\" > '{}'",
                root.join("argv.txt").display()
            ),
        );
        fixture.setup.machine = Some(machine);
        fixture.setup.video = Some(video);
        fixture.setup.basic_enabled = basic;
        fixture.save();
        let mut launched = fixture.launch().unwrap();
        assert_eq!(
            (launched.machine, launched.video, launched.basic),
            (machine, video, basic)
        );
        assert_eq!(wait(&mut launched), Some(0));
        let argv = std::fs::read_to_string(root.join("argv.txt")).unwrap();
        for flag in flags {
            assert!(
                argv.lines().any(|line| line == flag),
                "{flag} missing: {argv}"
            );
        }
    }
}

#[test]
fn a_game_changed_between_preview_and_launch_is_refused() {
    let fixture = Fixture::new();
    let offered = fixture.offered();
    let profile = fixture.profile_id();
    // Different bytes, same size.
    let mut bytes = atr_bytes();
    bytes[4000] ^= 0xff;
    std::fs::write(&fixture.game, &bytes).unwrap();
    assert!(refusal(fixture.launch_offered(&profile, &offered)).contains("changed since Play"));
    // Even a fresh offer cannot launch bytes the library never catalogued.
    assert!(refusal(fixture.launch()).contains("no longer matches the game in your library"));
    // A symlink swapped into the game's place is refused.
    std::fs::rename(&fixture.game, fixture.root().join("games/real.atr")).unwrap();
    symlink(fixture.root().join("games/real.atr"), &fixture.game).unwrap();
    assert!(refusal(fixture.launch_offered(&profile, &offered)).contains("symbolic link"));
    assert_eq!(fixture.workspaces(), 0);
}

#[test]
fn only_a_real_atr_with_the_catalogued_hash_is_accepted() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let check = |path: &Path, bytes: &[u8]| {
        std::fs::write(path, bytes).unwrap();
        let offered = CapturedFileIdentity::capture(&std::fs::symlink_metadata(path).unwrap());
        verify_selected_atr(path, &hex(&Sha256::digest(bytes)), &offered)
    };
    // Headerless XFD-shaped bytes named .atr: no ATR header, refused.
    assert!(check(&root.join("games/raw.atr"), &vec![0u8; 92160]).is_err());
    // A valid ATR under another extension is not silently widened.
    assert!(check(&root.join("games/disk.xfd"), &atr_bytes()).is_err());
    assert!(check(&root.join("games/disk.xex"), &atr_bytes()).is_err());
    // Oversized content is refused before it is hashed as a floppy.
    assert!(check(&root.join("games/huge.atr"), &vec![0x96; 400_000]).is_err());
    let media = check(&root.join("games/ok.atr"), &atr_bytes()).unwrap();
    assert_eq!(media.format, Atari800MediaFormat::Atr);
    assert_eq!(media.platform_evidence, LocalEvidenceStrength::Verified);
    // A truncated ATR passes the header check here and is refused by Core.
    let mut truncated = atr_bytes();
    truncated.truncate(5000);
    std::fs::write(&fixture.game, &truncated).unwrap();
    let mut fixture = fixture;
    fixture.key = hex(&Sha256::digest(&truncated));
    assert!(matches!(
        fixture.launch(),
        Err(Atari800LaunchRefusal::Core(_))
    ));
}

#[test]
fn firmware_swapped_after_setup_is_refused_at_click_time() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root().join("roms/xl.rom"), vec![0x55; 16384]).unwrap();
    assert!(refusal(fixture.launch()).contains("no longer the file you selected"));
    assert_eq!(fixture.workspaces(), 0);
}

#[test]
fn a_different_emulator_than_the_one_offered_is_refused() {
    let fixture = Fixture::new();
    let offered = fixture.offered();
    assert!(
        refusal(fixture.launch_offered("atari800:/somewhere/else", &offered))
            .contains("program changed since Play")
    );
    let profile = fixture.profile_id();
    std::fs::remove_file(fixture.root().join("bin/atari800")).unwrap();
    assert!(refusal(fixture.launch_offered(&profile, &offered)).contains("not installed"));
}

#[test]
fn an_unavailable_workspace_is_reported_and_nothing_is_started() {
    let fixture = Fixture::new();
    let root = fixture.root();
    write_script(
        &root.join("bin/atari800"),
        &format!("touch '{}'", root.join("ran").display()),
    );
    let profile = fixture.profile_id();
    let result = launch_selected_atr(
        &fixture.paths,
        None,
        &profile,
        &fixture.game,
        &fixture.key,
        &fixture.offered(),
        || SandboxManager::open(&root.join("no-such-base")),
    );
    assert!(matches!(result, Err(Atari800LaunchRefusal::Workspace(_))));
    assert!(!root.join("ran").exists());
}

#[test]
fn a_failed_or_killed_emulator_keeps_its_workspace_briefly_and_says_so() {
    for body in ["exit 3", "kill -9 $$"] {
        let fixture = Fixture::new();
        write_script(&fixture.root().join("bin/atari800"), body);
        let before = fixture.originals();
        let mut launched = fixture.launch().unwrap();
        assert_ne!(wait(&mut launched), Some(0));
        wait_for_outcome(
            &launched,
            |outcome| matches!(outcome, CleanupOutcome::Retained { path, .. } if *path == launched.workspace),
        );
        assert_eq!(fixture.workspaces(), 1, "kept for diagnosis, then removed");
        assert_eq!(fixture.originals(), before);
    }
}

#[test]
fn a_second_launch_after_the_first_exits_starts_cleanly() {
    let fixture = Fixture::new();
    for _ in 0..2 {
        let mut launched = fixture.launch().unwrap();
        assert_eq!(wait(&mut launched), Some(0));
        wait_for_outcome(&launched, |outcome| *outcome == CleanupOutcome::Removed);
        assert!(launched.startup_cleanup_skipped.is_empty());
    }
    assert_eq!(fixture.workspaces(), 0);
}

/// The REAL emulator, headless, through the whole click-time path, if one is
/// installed (skipped otherwise). Synthetic ROM-sized files stand in for
/// firmware, so this proves the launch and its isolation, never gameplay.
#[test]
fn real_atari800_starts_through_the_full_path_and_touches_only_its_workspace() {
    // Opt-in: this starts a real program, so an ordinary test run never does.
    let real = Path::new("/usr/bin/atari800");
    if std::env::var_os("EMUWIZ_TEST_REAL_ATARI800").is_none() || !real.is_file() {
        return;
    }
    let mut fixture = Fixture::new();
    let root = fixture.root();
    // Same argv, only the SDL drivers forced to headless.
    write_script(
        &root.join("bin/atari800"),
        "SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy exec /usr/bin/atari800 \"$@\"",
    );
    fixture.setup.executable = Some(root.join("bin/atari800"));
    fixture.save();
    let user_config =
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".atari800.cfg"));
    let user_config_before = user_config.as_ref().map(|path| std::fs::read(path).ok());
    let before = fixture.originals();
    let mut launched = fixture.launch().unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    let running = launched.process.process.poll().is_none();
    if running {
        // The private workspace exists only while the emulator owns it.
        assert!(launched.workspace.is_dir());
        assert!(matches!(
            launched.process.cleanup_outcome(),
            CleanupOutcome::Running
        ));
        // SAFETY: signals only the child this test started and still watches.
        unsafe { libc::kill(launched.process.process.pid as i32, libc::SIGTERM) };
    }
    // Never leave a real emulator behind: escalate if it ignores SIGTERM.
    let until = Instant::now() + Duration::from_secs(5);
    while launched.process.process.poll().is_none() {
        if Instant::now() > until {
            // SAFETY: as above, only this test's own watched child.
            unsafe { libc::kill(launched.process.process.pid as i32, libc::SIGKILL) };
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = wait(&mut launched);
    assert_eq!(fixture.originals(), before, "originals are never written");
    assert_eq!(
        user_config.as_ref().map(|path| std::fs::read(path).ok()),
        user_config_before,
        "the user's own Atari800 configuration was touched"
    );
    let outside: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        outside
            .iter()
            .all(|name| ["bin", "roms", "games", "config", "data", "run"].contains(&name.as_str())),
        "{outside:?}"
    );
    if !running {
        // No usable headless video here; nothing more can be claimed.
        return;
    }
    wait_for_outcome(&launched, |outcome| *outcome != CleanupOutcome::Running);
}

/// A merged /usr makes `/bin` a link to `/usr/bin`, and PATH names both. That
/// is one program, not a choice between two.
#[test]
fn one_program_reached_through_a_linked_path_entry_is_not_ambiguous() {
    let mut fixture = Fixture::new();
    let root = fixture.root();
    symlink(root.join("bin"), root.join("linked-bin")).unwrap();
    fixture.setup.executable = None;
    let path = std::env::join_paths([root.join("linked-bin"), root.join("bin")]).unwrap();
    let resolution = resolve_setup(&fixture.setup, Some(&path));
    assert_eq!(resolution.discovered, vec![root.join("bin/atari800")]);
    assert_eq!(resolution.executable, Some(root.join("bin/atari800")));
    assert!(resolution.eligible(), "{:?}", resolution.blockers);
}

/// Whatever is wrong with the emulator, a started setup keeps a (blocked)
/// candidate so its panel stays reachable to repair it.
#[test]
fn a_setup_without_a_usable_emulator_still_offers_a_blocked_candidate() {
    let mut fixture = Fixture::new();
    let root = fixture.root();
    fixture.setup.executable = Some(root.join("bin/gone"));
    let input = resolve_setup(&fixture.setup, None)
        .standalone_input()
        .unwrap();
    assert!(!input.eligible);
    assert_eq!(input.profile_id, UNSELECTED_PROFILE_ID);
    // Two programs and no selection: found, so offered, but blocked.
    fixture.setup = Atari800Setup::default();
    std::fs::create_dir_all(root.join("other")).unwrap();
    write_script(&root.join("other/atari800"), "exit 0");
    let two = std::env::join_paths([root.join("bin"), root.join("other")]).unwrap();
    let input = resolve_setup(&fixture.setup, Some(&two))
        .standalone_input()
        .unwrap();
    assert!(!input.eligible);
    assert_eq!(input.profile_id, UNSELECTED_PROFILE_ID);
    // That id can never authorise a launch.
    fixture.setup = Fixture::new().setup;
    assert!(
        refusal(fixture.launch_offered(UNSELECTED_PROFILE_ID, &fixture.offered()))
            .contains("program changed since Play")
    );
}

#[test]
fn a_very_long_emulator_path_still_launches() {
    let mut fixture = Fixture::new();
    let deep = fixture.root().join("d".repeat(120)).join("e".repeat(120));
    std::fs::create_dir_all(&deep).unwrap();
    write_script(&deep.join("atari800"), "exit 0");
    fixture.setup.executable = Some(deep.join("atari800"));
    fixture.save();
    assert!(fixture.profile_id().len() <= 32);
    let mut launched = fixture.launch().unwrap();
    assert_eq!(wait(&mut launched), Some(0));
}

#[test]
fn a_seed_folder_below_a_link_is_refused_before_anything_is_created() {
    let fixture = Fixture::new();
    let root = fixture.root();
    std::fs::create_dir_all(root.join("real-data")).unwrap();
    symlink(root.join("real-data"), root.join("data-link")).unwrap();
    let below = root.join("data-link/atari800");
    assert!(ensure_isolated_seed(&below, Atari800Machine::Atari800Xl, false).is_err());
    assert!(
        !root.join("real-data/atari800").exists(),
        "nothing created through the link"
    );
}
