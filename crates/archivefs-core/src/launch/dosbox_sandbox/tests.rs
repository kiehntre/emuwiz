//! Adversarial tests for the DOSBox OS sandbox. Enforcement tests run the real
//! `bwrap` (and the real DOSBox where installed) and skip loudly, never
//! silently pass, when this machine cannot create the sandbox.

use super::*;
use crate::launch::dosbox_command::{DosBoxCommandSelection, DosBoxVariant};
use crate::launch::dosbox_execution::{
    DosBoxLaunchSpawnError, LaunchedDosBoxProcess, spawn_dosbox, spawn_dosbox_with_bwrap,
};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const REAL_DOSBOX: &str = "/usr/bin/dosbox";
/// A binary under /usr that exists everywhere, so the pure argv and grant tests
/// do not depend on DOSBox being installed.
const STAND_IN_EMULATOR: &str = "/usr/bin/true";

fn sandbox_available() -> bool {
    match probe_sandbox(Path::new(BWRAP_PATH)) {
        Ok(()) => true,
        Err(error) => {
            assert!(
                std::env::var_os("EMUWIZ_REQUIRE_SANDBOX").is_none(),
                "EMUWIZ_REQUIRE_SANDBOX is set but the sandbox is unusable: {error:?}"
            );
            eprintln!("SKIPPED (no usable bubblewrap here): {error:?}");
            false
        }
    }
}

fn headless() -> DosBoxHostAccess {
    DosBoxHostAccess {
        read_only_paths: Vec::new(),
        environment: vec![
            ("SDL_VIDEODRIVER".into(), "dummy".into()),
            ("SDL_AUDIODRIVER".into(), "dummy".into()),
        ],
    }
}

/// A game folder in /tmp and a canary folder under the real HOME, which the
/// sandbox must never expose.
struct World {
    _game_root: TempDir,
    _canary_root: TempDir,
    game: PathBuf,
    canary_dir: PathBuf,
    canary_text: String,
}

fn world() -> Option<World> {
    let built = build_world();
    assert!(
        built.is_some() || std::env::var_os("EMUWIZ_REQUIRE_SANDBOX").is_none(),
        "EMUWIZ_REQUIRE_SANDBOX is set but the test folders could not be created (HOME?)"
    );
    if built.is_none() {
        eprintln!("SKIPPED (no HOME or temporary folders here)");
    }
    built
}

fn build_world() -> Option<World> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let canary_root = tempfile::tempdir_in(&home).ok()?;
    let game_root = tempfile::tempdir().ok()?;
    let game = game_root.path().join("Game");
    std::fs::create_dir(&game).ok()?;
    let canary_dir = canary_root.path().join("outside");
    std::fs::create_dir(&canary_dir).ok()?;
    let canary_text = format!(
        "CANARY-{}-{}",
        std::process::id(),
        canary_root.path().display()
    );
    std::fs::write(canary_dir.join("secret.txt"), &canary_text).ok()?;
    Some(World {
        _game_root: game_root,
        _canary_root: canary_root,
        game,
        canary_dir,
        canary_text,
    })
}

fn command_for(exe: &Path, game: &Path) -> DosBoxCommand {
    let config = game.join("dosbox.conf");
    DosBoxCommand {
        executable: exe.to_path_buf(),
        arguments: vec!["-conf".into(), config.clone().into_os_string()],
        working_directory: Some(game.to_path_buf()),
        selection: DosBoxCommandSelection {
            platform_id: "DOS".into(),
            variant: DosBoxVariant::Classic,
            game_directory: game.to_path_buf(),
            config_path: config,
        },
    }
}

fn wait(process: &mut LaunchedDosBoxProcess) -> String {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if process.poll().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "sandboxed process did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
    let report = process.poll().unwrap();
    format!(
        "{:?} {}",
        report.status,
        String::from_utf8_lossy(&report.stderr)
    )
}

/// Runs the REAL DOSBox on `autoexec` inside the sandbox. The config is
/// deliberately unreviewed, so these tests prove the sandbox, not the parser.
fn run_real_dosbox(world: &World, autoexec: &str) -> String {
    std::fs::write(
        world.game.join("dosbox.conf"),
        format!("[dosbox]\nmemsize=16\n[autoexec]\n{autoexec}\nexit\n"),
    )
    .unwrap();
    let command = command_for(Path::new(REAL_DOSBOX), &world.game);
    let mut process = spawn_dosbox(command, &headless()).expect("sandboxed spawn");
    wait(&mut process)
}

fn out(world: &World, name: &str) -> String {
    std::fs::read_to_string(world.game.join(name)).unwrap_or_default()
}

fn ready() -> Option<World> {
    if !sandbox_available() {
        return None;
    }
    if !Path::new(REAL_DOSBOX).exists() {
        assert!(
            std::env::var_os("EMUWIZ_REQUIRE_SANDBOX").is_none(),
            "EMUWIZ_REQUIRE_SANDBOX is set but {REAL_DOSBOX} is missing"
        );
        eprintln!("SKIPPED (needs {REAL_DOSBOX})");
        return None;
    }
    world()
}

// --- real DOSBox inside the sandbox -----------------------------------------

#[test]
fn control_dosbox_can_read_a_file_that_is_granted() {
    // Without this, "the canary was not read" could just mean the harness
    // never reads anything.
    let Some(w) = ready() else { return };
    std::fs::create_dir(w.game.join("inside")).unwrap();
    std::fs::write(w.game.join("inside").join("secret.txt"), &w.canary_text).unwrap();
    run_real_dosbox(
        &w,
        "mount c .\nmount d inside\nc:\ntype d:\\secret.txt > out.txt",
    );
    assert!(
        out(&w, "OUT.TXT").contains(&w.canary_text),
        "harness is blind"
    );
}

#[test]
fn mounting_a_host_folder_outside_the_grant_finds_nothing() {
    let Some(w) = ready() else { return };
    let outside = w.canary_dir.display();
    run_real_dosbox(
        &w,
        &format!(
            "mount c .\nmount d {outside}\nc:\ntype d:\\secret.txt > out.txt\ndir d:\\ > dir.txt"
        ),
    );
    assert!(!out(&w, "OUT.TXT").contains(&w.canary_text));
    assert!(!out(&w, "DIR.TXT").to_uppercase().contains("SECRET"));
}

#[test]
fn mounting_the_host_root_shows_only_the_sandbox_view() {
    let Some(w) = ready() else { return };
    run_real_dosbox(
        &w,
        "mount c .\nmount d /\nc:\ndir d:\\ > root.txt\ndir d:\\tmp > tmp.txt",
    );
    let root = out(&w, "ROOT.TXT").to_uppercase();
    for forbidden in ["HOME", "MNT", "VAR", "ROOT", "SRV", "OPT", "MEDIA"] {
        assert!(!root.contains(forbidden), "{forbidden} visible: {root}");
    }
    assert!(!out(&w, "TMP.TXT").to_uppercase().contains("OUTSIDE"));
}

#[test]
fn mounting_the_home_folder_does_not_reach_the_canary() {
    let Some(w) = ready() else { return };
    let home = std::env::var("HOME").unwrap();
    run_real_dosbox(
        &w,
        &format!("mount c .\nmount d {home}\nc:\ndir d:\\emucan*.* > home.txt"),
    );
    let listing = out(&w, "HOME.TXT").to_uppercase();
    let canary_name = w
        ._canary_root
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_uppercase();
    assert!(!listing.contains(&canary_name), "{listing}");
}

#[test]
fn a_symlink_in_the_game_folder_cannot_lead_out() {
    let Some(w) = ready() else { return };
    std::os::unix::fs::symlink(w.canary_dir.join("secret.txt"), w.game.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&w.canary_dir, w.game.join("odir")).unwrap();
    run_real_dosbox(
        &w,
        "mount c .\nc:\ntype link.txt > out1.txt\ntype odir\\secret.txt > out2.txt\necho pwned > odir\\new.txt",
    );
    assert!(!out(&w, "OUT1.TXT").contains(&w.canary_text));
    assert!(!out(&w, "OUT2.TXT").contains(&w.canary_text));
    assert!(!w.canary_dir.join("new.txt").exists());
    assert!(!w.canary_dir.join("NEW.TXT").exists());
    assert_eq!(
        std::fs::read_to_string(w.canary_dir.join("secret.txt")).unwrap(),
        w.canary_text
    );
}

#[test]
fn a_batch_file_that_remounts_at_runtime_cannot_reach_host_files() {
    let Some(w) = ready() else { return };
    let outside = w.canary_dir.display();
    std::fs::write(
        w.game.join("run.bat"),
        format!(
            "mount e {outside}\r\ntype e:\\secret.txt > c:\\b1.txt\r\nmount f /\r\ndir f:\\ > c:\\b2.txt\r\n\
             echo pwned > e:\\w.txt\r\nz:\\config.com -writeconf {outside}/x.conf\r\n"
        ),
    )
    .unwrap();
    run_real_dosbox(&w, "mount c .\nc:\ncall run.bat");
    assert!(!out(&w, "B1.TXT").contains(&w.canary_text));
    assert!(!out(&w, "B2.TXT").to_uppercase().contains("HOME"));
    assert!(!w.canary_dir.join("w.txt").exists());
    assert!(!w.canary_dir.join("W.TXT").exists());
    assert!(!w.canary_dir.join("x.conf").exists());
}

#[test]
fn the_game_can_still_write_inside_its_own_folder() {
    let Some(w) = ready() else { return };
    run_real_dosbox(&w, "mount c .\nc:\necho saved > save.dat");
    assert!(out(&w, "SAVE.DAT").contains("saved"));
}

// --- fast process-level tests with a stand-in executable --------------------

fn stand_in(world: &World, script: &str) -> PathBuf {
    let exe = world.game.parent().unwrap().join("fake-dosbox");
    std::fs::write(&exe, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

fn run_stand_in(world: &World, script: &str, access: &DosBoxHostAccess) -> String {
    let exe = stand_in(world, script);
    let mut process = spawn_dosbox(command_for(&exe, &world.game), access).expect("spawn");
    wait(&mut process)
}

#[test]
fn the_sandbox_sees_only_the_expected_top_level_folders() {
    if !sandbox_available() {
        return;
    }
    let Some(w) = world() else { return };
    run_stand_in(
        &w,
        "#!/bin/sh\nls -A / > root.txt\n",
        &DosBoxHostAccess::none(),
    );
    let mut entries: Vec<String> = out(&w, "root.txt").lines().map(str::to_string).collect();
    entries.sort();
    assert_eq!(
        entries,
        [
            "bin", "dev", "etc", "lib", "lib64", "proc", "sbin", "tmp", "usr"
        ]
    );
}

#[test]
fn the_environment_is_cleared_to_the_fixed_set_plus_grants() {
    if !sandbox_available() {
        return;
    }
    let Some(w) = world() else { return };
    let access = DosBoxHostAccess {
        read_only_paths: Vec::new(),
        environment: vec![("GRANTED".into(), "yes".into())],
    };
    run_stand_in(&w, "#!/bin/sh\nenv > env.txt\n", &access);
    let text = out(&w, "env.txt");
    let mut keys: Vec<&str> = text
        .lines()
        .filter_map(|line| line.split('=').next())
        .filter(|key| !["PWD", "SHLVL", "_", "OLDPWD"].contains(key))
        .collect();
    keys.sort();
    assert_eq!(keys, ["GRANTED", "HOME", "PATH"], "{text}");
}

#[test]
fn there_is_no_network_and_no_host_processes() {
    if !sandbox_available() {
        return;
    }
    let Some(w) = world() else { return };
    run_stand_in(
        &w,
        "#!/bin/sh\ncat /proc/net/dev > net.txt\nls /proc > procs.txt\n",
        &DosBoxHostAccess::none(),
    );
    let interfaces: Vec<String> = out(&w, "net.txt")
        .lines()
        .filter(|l| l.contains(':'))
        .map(|l| l.split(':').next().unwrap().trim().to_string())
        .collect();
    assert!(interfaces.iter().all(|i| i == "lo"), "{interfaces:?}");
    assert!(
        !out(&w, "procs.txt")
            .lines()
            .any(|l| l == std::process::id().to_string()),
        "the host process list is visible"
    );
}

#[test]
fn a_private_tmp_hides_other_host_tmp_files() {
    if !sandbox_available() {
        return;
    }
    let Some(w) = world() else { return };
    let host_tmp_file = tempfile::NamedTempFile::new().unwrap();
    let name = host_tmp_file
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    run_stand_in(
        &w,
        "#!/bin/sh\nls -A /tmp > tmp.txt\n",
        &DosBoxHostAccess::none(),
    );
    assert!(!out(&w, "tmp.txt").contains(&name));
}

#[test]
fn an_inheritable_descriptor_does_not_reach_the_sandbox() {
    use std::os::fd::AsRawFd;
    if !sandbox_available() {
        return;
    }
    let Some(w) = world() else { return };
    // An inheritable (non-CLOEXEC) descriptor onto the canary, as a buggy
    // library could leave behind.
    let file = std::fs::File::open(w.canary_dir.join("secret.txt")).unwrap();
    let leaked = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 100) };
    assert!(leaked >= 100);

    // Control: the ordinary spawner leaks it, so the check below can fail.
    let control = w.game.join("control.txt");
    let control_cmd = crate::launch::process_spawn::PreparedProcessCommand {
        executable: "/bin/sh".into(),
        arguments: vec![
            "-c".into(),
            format!("ls -l /proc/self/fd > '{}'", control.display()).into(),
        ],
        working_directory: Some(w.game.clone()),
    };
    let mut watched = crate::launch::process_spawn::spawn_watched_process(&control_cmd).unwrap();
    while watched.poll().is_none() {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        std::fs::read_to_string(&control)
            .unwrap()
            .contains("secret.txt"),
        "control: the leak should be visible without the sandbox"
    );

    run_stand_in(
        &w,
        "#!/bin/sh\nls -l /proc/self/fd > fds.txt\n",
        &DosBoxHostAccess::none(),
    );
    unsafe { libc::close(leaked) };
    assert!(
        !out(&w, "fds.txt").contains("secret.txt"),
        "{}",
        out(&w, "fds.txt")
    );
}

// --- failing closed --------------------------------------------------------

#[test]
fn a_missing_bwrap_starts_nothing() {
    let Some(w) = world() else { return };
    let exe = stand_in(&w, "#!/bin/sh\ntouch ran.txt\n");
    let result = spawn_dosbox_with_bwrap(
        command_for(&exe, &w.game),
        &DosBoxHostAccess::none(),
        Path::new("/nonexistent/bwrap"),
    );
    assert!(matches!(
        result,
        Err(DosBoxLaunchSpawnError::Sandbox(
            DosBoxSandboxError::Unavailable(_)
        ))
    ));
    assert!(!w.game.join("ran.txt").exists());
}

#[test]
fn a_bwrap_that_cannot_build_the_sandbox_starts_nothing() {
    let Some(w) = world() else { return };
    let exe = stand_in(&w, "#!/bin/sh\ntouch ran.txt\n");
    // /usr/bin/false stands in for a bwrap that fails (for example userns off).
    let result = spawn_dosbox_with_bwrap(
        command_for(&exe, &w.game),
        &DosBoxHostAccess::none(),
        Path::new("/usr/bin/false"),
    );
    assert!(matches!(
        result,
        Err(DosBoxLaunchSpawnError::Sandbox(
            DosBoxSandboxError::Unavailable(_)
        ))
    ));
    assert!(!w.game.join("ran.txt").exists());
}

// --- argv shape and grant validation (no bwrap needed) ----------------------

fn plan(
    game: &str,
    exe: &str,
    access: &DosBoxHostAccess,
) -> Result<Vec<String>, DosBoxSandboxError> {
    let dir = tempfile::tempdir().unwrap();
    let exe_path = if exe.starts_with("/usr/") {
        PathBuf::from(exe)
    } else {
        let p = dir.path().join("dosbox");
        std::fs::write(&p, "x").unwrap();
        p
    };
    let command = command_for(&exe_path, Path::new(game));
    sandboxed_command(&command, access, Path::new(BWRAP_PATH)).map(|p| {
        p.arguments
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    })
}

/// The reason a plan was refused. Panics if it was not refused.
fn refusal(game: &str, exe: &str, access: &DosBoxHostAccess) -> String {
    plan(game, exe, access)
        .expect_err("expected the sandbox plan to be refused")
        .detail()
        .to_string()
}

#[test]
fn the_argv_is_default_deny_with_only_the_game_folder_writable() {
    let args = plan(
        "/srv/games/Keen",
        STAND_IN_EMULATOR,
        &DosBoxHostAccess::none(),
    )
    .unwrap();
    for required in [
        "--unshare-user",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-ipc",
        "--disable-userns",
        "--new-session",
        "--clearenv",
    ] {
        assert!(args.contains(&required.to_string()), "{required}");
    }
    let binds: Vec<_> = args
        .windows(3)
        .filter(|w| w[0] == "--bind" || w[0] == "--ro-bind")
        .map(|w| (w[0].as_str(), w[1].as_str()))
        .collect();
    assert!(binds.contains(&("--bind", "/srv/games/Keen")));
    assert!(binds.contains(&("--ro-bind", "/usr")));
    // The only writable host bind is the game folder.
    assert_eq!(
        binds.iter().filter(|(kind, _)| *kind == "--bind").count(),
        1
    );
    assert!(!args.iter().any(|a| a == "/" || a == "/home" || a == "/etc"));
    let split = args.iter().position(|a| a == "--").unwrap();
    assert_eq!(args[split + 1], STAND_IN_EMULATOR);
    assert_eq!(args[split + 2], "-conf");
    assert_eq!(args.len(), split + 4);
}

#[test]
fn an_executable_outside_usr_is_bound_read_only() {
    let args = plan("/srv/games/Keen", "/opt/dosbox", &DosBoxHostAccess::none()).unwrap();
    assert!(
        args.windows(2)
            .any(|w| w[0] == "--ro-bind" && w[1].ends_with("/dosbox"))
    );
}

#[test]
fn broad_or_hostile_grants_are_refused() {
    let none = DosBoxHostAccess::none();
    let home = std::env::var("HOME").unwrap();
    let parent_of_home = Path::new(&home)
        .parent()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    for (game, why) in [
        ("/home", "too broad"),
        ("/", "too broad"),
        ("/srv/../etc/Keen", "`..`"),
        ("relative/Keen", "not absolute"),
        (parent_of_home.as_str(), "contains the home folder"),
    ] {
        let detail = refusal(game, STAND_IN_EMULATOR, &none);
        assert!(detail.contains(why), "{game}: {detail}");
    }
    let overlapping = DosBoxHostAccess {
        read_only_paths: vec![PathBuf::from("/srv/games/Keen/sub")],
        environment: vec![],
    };
    let detail = refusal("/srv/games/Keen", STAND_IN_EMULATOR, &overlapping);
    assert!(detail.contains("overlaps the game folder"), "{detail}");
    let broad = DosBoxHostAccess {
        read_only_paths: vec![PathBuf::from("/home")],
        environment: vec![],
    };
    let detail = refusal("/srv/games/Keen", STAND_IN_EMULATOR, &broad);
    assert!(detail.contains("too broad"), "{detail}");
    let path_override = DosBoxHostAccess {
        read_only_paths: vec![],
        environment: vec![("PATH".into(), "/tmp".into())],
    };
    let detail = refusal("/srv/games/Keen", STAND_IN_EMULATOR, &path_override);
    assert!(detail.contains("fixed by the sandbox"), "{detail}");
}

#[test]
fn an_emulator_inside_the_game_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("Game");
    std::fs::create_dir(&game).unwrap();
    let exe = game.join("dosbox");
    std::fs::write(&exe, "x").unwrap();
    let command = command_for(&exe, &game);
    let error =
        sandboxed_command(&command, &DosBoxHostAccess::none(), Path::new(BWRAP_PATH)).unwrap_err();
    assert!(
        error.detail().contains("inside the game folder"),
        "{error:?}"
    );
}

#[test]
fn the_desktop_session_grant_lets_real_dosbox_open_a_window() {
    // Runs only inside a desktop or Xvfb session (DISPLAY set). The sandbox
    // gets the display socket and nothing else of the host.
    let access = DosBoxHostAccess::from_session_environment();
    if std::env::var_os("DISPLAY").is_none() || access.read_only_paths.is_empty() {
        eprintln!("SKIPPED (no X11 or Wayland session here)");
        return;
    }
    let Some(w) = ready() else { return };
    std::fs::write(
        w.game.join("dosbox.conf"),
        "[sdl]\nwaitonerror=false\n[autoexec]\nmount c .\nc:\necho up > ok.txt\nexit\n",
    )
    .unwrap();
    let mut process = spawn_dosbox(command_for(Path::new(REAL_DOSBOX), &w.game), &access)
        .expect("sandboxed spawn");
    let summary = wait(&mut process);
    assert!(out(&w, "OK.TXT").contains("up"), "{summary}");
}

#[test]
fn the_host_environment_is_not_readable_from_inside_the_sandbox() {
    if !sandbox_available() {
        return;
    }
    let Some(w) = world() else { return };
    // The test process has a real environment (HOME at least).
    assert!(std::env::var_os("HOME").is_some());
    run_stand_in(
        &w,
        "#!/bin/sh\ncat /proc/1/environ > pid1env.txt\ncat /proc/$PPID/environ > ppidenv.txt 2>/dev/null\n",
        &DosBoxHostAccess::none(),
    );
    assert_eq!(out(&w, "pid1env.txt"), "", "bwrap leaked its environment");
    assert!(!out(&w, "ppidenv.txt").contains(&std::env::var("HOME").unwrap()));
}

#[test]
fn system_locations_and_loader_variables_are_refused() {
    let none = DosBoxHostAccess::none();
    for game in [
        "/usr/lib/Keen",
        "/etc/games/Keen",
        "/var/games/Keen",
        "/opt/games/Keen",
    ] {
        let detail = refusal(game, STAND_IN_EMULATOR, &none);
        assert!(
            detail.contains("inside a system location"),
            "{game}: {detail}"
        );
    }
    let preload = DosBoxHostAccess {
        read_only_paths: vec![],
        environment: vec![("LD_PRELOAD".into(), "/tmp/x.so".into())],
    };
    let detail = refusal("/srv/games/Keen", STAND_IN_EMULATOR, &preload);
    assert!(detail.contains("what code the emulator loads"), "{detail}");
    // Control: the same plan with no hostile input succeeds, so each refusal
    // above is for its own stated reason and not a missing emulator.
    assert!(plan("/srv/games/Keen", STAND_IN_EMULATOR, &none).is_ok());
}

#[test]
fn audio_is_not_part_of_the_default_session_grant() {
    let access = DosBoxHostAccess::from_session_environment();
    assert!(
        !access
            .read_only_paths
            .iter()
            .any(|p| p.to_string_lossy().contains("pulse"))
    );
    assert!(!access.environment.iter().any(|(k, _)| k == "PULSE_SERVER"));
}

#[test]
fn a_denied_sandbox_names_apparmor_and_the_scoped_fix() {
    let hint = sandbox_denial_hint(Some(true));
    assert!(hint.contains("apparmor_restrict_unprivileged_userns=1"));
    assert!(hint.contains("docs/DOSBOX_SANDBOX_UBUNTU.md"));
    assert!(hint.contains("never runs DOSBox without the sandbox"));
    for other in [Some(false), None] {
        let hint = sandbox_denial_hint(other);
        assert!(!hint.contains("AppArmor restricts"));
        assert!(hint.contains("never runs DOSBox without the sandbox"));
    }
}

/// Runs against the real `bwrap` only where CI has placed this test under an
/// AppArmor profile that denies it (`aa-exec -p unprivileged_userns`,
/// EMUWIZ_EXPECT_SANDBOX_DENIED=1): the probe fails with the actionable hint
/// and a launch starts nothing. That is an AppArmor denial, not a byte-exact
/// reproduction of where stock Ubuntu's denial strikes inside bwrap.
#[test]
fn stock_apparmor_denial_fails_closed_with_the_hint() {
    if std::env::var_os("EMUWIZ_EXPECT_SANDBOX_DENIED").is_none() {
        return;
    }
    let error = probe_sandbox(Path::new(BWRAP_PATH)).expect_err("stock AppArmor must deny bwrap");
    let detail = error.detail();
    assert!(
        detail.contains("AppArmor restricts unprivileged user namespaces"),
        "{error:?}"
    );
    // The refusal must come from a permission denial, not some other bwrap error.
    assert!(
        [
            "Operation not permitted",
            "Permission denied",
            "No permissions to create new namespace"
        ]
        .iter()
        .any(|cause| detail.contains(cause)),
        "{error:?}"
    );
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("Game");
    std::fs::create_dir(&game).unwrap();
    std::fs::write(
        game.join("dosbox.conf"),
        "[autoexec]\nmount c .\nc:\necho started > ran.txt\nexit\n",
    )
    .unwrap();
    let result = spawn_dosbox(command_for(Path::new(REAL_DOSBOX), &game), &headless());
    assert!(matches!(
        result,
        Err(DosBoxLaunchSpawnError::Sandbox(
            DosBoxSandboxError::Unavailable(_)
        ))
    ));
    assert!(!game.join("ran.txt").exists() && !game.join("RAN.TXT").exists());
}
