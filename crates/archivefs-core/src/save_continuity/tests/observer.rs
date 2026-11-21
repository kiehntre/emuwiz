//! The production safe-point check, against a stand-in `/proc` and real
//! kernel leases on temporary files.

use super::super::ContinuityError;
use super::super::discover::{ProcessObserver, SAFE_POINT_EMULATORS, safe_point_supported};
use super::super::model::InUse;
use super::super::store::{CaptureBoundary, InUseObserver};
use super::*;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

struct Proc(tempfile::TempDir);

impl Proc {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    /// One process: name, command line (spaces separate arguments), cgroup.
    fn add(&self, pid: u32, comm: &str, cmdline: &str, cgroup: &str) -> PathBuf {
        let dir = self.0.path().join(pid.to_string());
        fs::create_dir_all(dir.join("fd")).unwrap();
        fs::write(dir.join("comm"), comm).unwrap();
        fs::write(dir.join("cmdline"), cmdline.replace(' ', "\0")).unwrap();
        fs::write(dir.join("cgroup"), cgroup).unwrap();
        fs::write(dir.join("status"), "State:\tS (sleeping)\n").unwrap();
        dir
    }

    fn observer(&self) -> ProcessObserver {
        ProcessObserver::with_proc_root(self.0.path().to_path_buf())
    }

    /// Also answers for Dolphin, whose rules are synthetic-only.
    fn synthetic(&self) -> ProcessObserver {
        ProcessObserver::synthetic_including_not_verified(self.0.path().to_path_buf())
    }
}

const ALL_SIX: [&str; 6] = [
    "PCSX2",
    "RetroArch",
    "DuckStation",
    "Dolphin",
    "RPCS3",
    "PPSSPP",
];
const USER: &str = "0::/user.slice/user-1000.slice/session-2.scope\n";

fn save_file(dir: &tempfile::TempDir) -> PathBuf {
    let path = dir.path().join("save.bin");
    fs::write(&path, b"save").unwrap();
    path
}

#[test]
fn every_supported_emulator_is_seen_in_each_form_it_runs_in() {
    // (emulator, process name, command line, cgroup)
    let forms: &[(&str, &str, &str, &str)] = &[
        (
            "DuckStation",
            "duckstation-qt",
            "/usr/bin/duckstation-qt",
            USER,
        ),
        (
            "DuckStation",
            "duckstation-qt",
            "/tmp/.mount_DuckStAbCdEf/usr/bin/duckstation-qt -batch game.chd",
            USER,
        ),
        (
            "DuckStation",
            "DuckStation.App",
            "/home/u/Applications/DuckStation/DuckStation.AppImage",
            USER,
        ),
        (
            "DuckStation",
            "duckstation-nog",
            "duckstation-nogui game.cue",
            USER,
        ),
        (
            "DuckStation",
            "bwrap",
            "bwrap --args 41 duckstation-qt",
            "0::/user.slice/app.slice/app-flatpak-org.duckstation.DuckStation-123.scope\n",
        ),
        (
            "DuckStation",
            "flatpak",
            "flatpak run org.duckstation.DuckStation",
            USER,
        ),
        (
            "Dolphin",
            "dolphin-emu",
            "/usr/bin/dolphin-emu -e game.rvz",
            USER,
        ),
        (
            "Dolphin",
            "dolphin-emu-nog",
            "dolphin-emu-nogui -e game.iso",
            USER,
        ),
        (
            "Dolphin",
            "bwrap",
            "bwrap --args 40 dolphin-emu-wrapper",
            "0::/app.slice/app-flatpak-org.DolphinEmu.dolphin-emu-99.scope\n",
        ),
        (
            "Dolphin",
            "flatpak",
            "flatpak run --command=dolphin-emu org.DolphinEmu.dolphin-emu",
            USER,
        ),
        (
            "RPCS3",
            "rpcs3",
            "/usr/bin/rpcs3 --no-gui /games/BLUS30750",
            USER,
        ),
        (
            "RPCS3",
            "rpcs3-v0.0.42-1",
            "/home/u/rpcs3-v0.0.42-19980_linux64.AppImage",
            USER,
        ),
        (
            "RPCS3",
            "rpcs3",
            "/app/bin/rpcs3",
            "0::/app.slice/app-flatpak-net.rpcs3.RPCS3-7.scope\n",
        ),
        ("PPSSPP", "PPSSPPSDL", "/usr/bin/PPSSPPSDL game.iso", USER),
        ("PPSSPP", "PPSSPPQt", "PPSSPPQt", USER),
        ("PPSSPP", "ppsspp", "/snap/bin/ppsspp", USER),
        (
            "PPSSPP",
            "bwrap",
            "bwrap --args 40 PPSSPPSDL",
            "0::/app.slice/app-flatpak-org.ppsspp.PPSSPP-5.scope\n",
        ),
        (
            "PCSX2",
            "pcsx2-qt",
            "/usr/bin/pcsx2-qt -batch game.iso",
            USER,
        ),
        (
            "PCSX2",
            "pcsx2-qt",
            "/app/bin/pcsx2-qt",
            "0::/app.slice/app-flatpak-net.pcsx2.PCSX2-3.scope\n",
        ),
        (
            "RetroArch",
            "retroarch",
            "retroarch -L /cores/snes9x_libretro.so game.sfc",
            USER,
        ),
        (
            "RetroArch",
            "flatpak",
            "flatpak run org.libretro.RetroArch",
            USER,
        ),
    ];
    for (emulator, comm, cmdline, cgroup) in forms {
        let dir = tempfile::tempdir().unwrap();
        let save = save_file(&dir);
        let proc = Proc::new();
        proc.add(4242, comm, cmdline, cgroup);
        assert_eq!(
            proc.synthetic().observe(emulator, &save),
            InUse::EmulatorRunning,
            "{emulator} as `{cmdline}` must count as running"
        );
        // And no other emulator's saves are blocked by it.
        for other in ALL_SIX
            .iter()
            .filter(|other| !other.eq_ignore_ascii_case(emulator))
        {
            assert_ne!(
                proc.synthetic().observe(other, &save),
                InUse::EmulatorRunning,
                "{cmdline} is not {other}"
            );
        }
    }
}

#[test]
fn with_no_emulator_and_nothing_holding_the_file_it_is_a_verified_safe_point() {
    for emulator in SAFE_POINT_EMULATORS {
        let dir = tempfile::tempdir().unwrap();
        let save = save_file(&dir);
        let proc = Proc::new();
        // Unrelated programs, including KDE's file manager, which shares a name with Dolphin.
        proc.add(10, "bash", "/bin/bash", USER);
        proc.add(11, "dolphin", "/usr/bin/dolphin /home/u", USER);
        proc.add(12, "firefox", "firefox https://duckduckgo.com", USER);
        assert_eq!(
            proc.observer().observe(emulator, &save),
            InUse::NotDetected,
            "{emulator}"
        );
    }
}

#[test]
fn an_emulator_with_no_reviewed_identity_is_unknown_not_safe() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    let proc = Proc::new();
    // Dolphin is here on purpose: its rules pass synthetic tests but were
    // never checked against a real installation, so production says unknown.
    proc.add(50, "dolphin-emu", "/usr/bin/dolphin-emu", USER);
    for emulator in [
        "Dolphin", "dolphin", "mGBA", "melonDS", "xemu", "Flycast", "",
    ] {
        assert!(!safe_point_supported(emulator));
        assert_eq!(
            proc.observer().observe(emulator, &save),
            InUse::Unknown,
            "{emulator}"
        );
        assert_eq!(
            proc.observer().guard(emulator, &save).state(),
            InUse::Unknown,
            "{emulator}"
        );
    }
}

#[test]
fn a_program_holding_the_save_open_is_active_whatever_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    let proc = Proc::new();
    // Seen two ways: the kernel refuses the lease (a real open descriptor)...
    let held = fs::OpenOptions::new().write(true).open(&save).unwrap();
    for emulator in SAFE_POINT_EMULATORS {
        assert_eq!(
            proc.observer().observe(emulator, &save),
            InUse::OpenByAProgram,
            "{emulator}"
        );
        assert_eq!(
            proc.observer().guard(emulator, &save).state(),
            InUse::OpenByAProgram,
            "{emulator}"
        );
    }
    drop(held);
    // ...and a descriptor on the file in another process's table (a sync tool).
    let pid = proc.add(77, "syncthing", "/usr/bin/syncthing", USER);
    std::os::unix::fs::symlink(&save, pid.join("fd/9")).unwrap();
    assert_eq!(
        proc.observer().observe("RPCS3", &save),
        InUse::OpenByAProgram
    );
}

#[test]
fn standalone_dolphin_is_never_a_verified_safe_point_until_checked_for_real() {
    use super::super::discover::SAFE_POINT_NOT_VERIFIED;
    assert!(
        SAFE_POINT_NOT_VERIFIED
            .iter()
            .any(|(name, _)| *name == "Dolphin")
    );
    assert!(!safe_point_supported("Dolphin"));
    let bench = Bench::new();
    let save = bench.file("01-GEDE-Eternal Darkness.gci", b"gamecube save");
    let proc = Proc::new(); // nothing is running at all
    let mut ask = request("dev", &[&save], 1);
    ask.provenance.emulator = "Dolphin".to_string();

    // By default: refused, nothing kept.
    let refused = bench.store.capture(&ask, &proc.observer()).unwrap_err();
    assert!(
        matches!(refused, ContinuityError::ClosureUnproven(_)),
        "{refused}"
    );
    assert!(bench.store.list().is_empty());

    // Asked for explicitly: a copy exists, and can never pass for verified.
    ask.allow_unproven_closure = true;
    let copy = bench
        .store
        .capture(&ask, &proc.observer())
        .unwrap()
        .manifest()
        .clone();
    assert_eq!(copy.body.boundary, CaptureBoundary::StableReadOnly);
    assert_ne!(copy.body.boundary, CaptureBoundary::EmulatorClosedObserved);
    assert!(
        copy.body
            .checks
            .iter()
            .any(|check| check.starts_with("UNVERIFIED"))
    );
    assert_eq!(fs::read(&save).unwrap(), b"gamecube save");
}

#[test]
fn a_writer_that_opens_the_save_during_the_guard_breaks_it() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    let proc = Proc::new();
    let observer = proc.observer();
    let guard = observer.guard("RPCS3", &save);
    assert!(
        guard.kernel_watched(),
        "temporary folders support leases on this machine"
    );
    assert_eq!(guard.state(), InUse::NotDetected);
    // Non-blocking, as a careful writer would; the kernel still records the break.
    let attempt = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&save);
    assert!(
        attempt.is_err(),
        "the kernel makes the writer wait while the lease is held"
    );
    assert_eq!(guard.state(), InUse::OpenByAProgram);
}

#[test]
fn a_process_that_cannot_be_inspected_is_unknown_without_a_lease_and_covered_with_one() {
    if unsafe { libc::geteuid() } == 0 {
        return; // root can read everything; the case cannot be staged
    }
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    let proc = Proc::new();
    let opaque = proc.add(1, "systemd", "/sbin/init", "0::/init.scope\n");
    fs::set_permissions(opaque.join("fd"), fs::Permissions::from_mode(0o000)).unwrap();
    let observer = proc.observer();

    // With a lease the kernel answers for that process's descriptors.
    let guard = observer.guard("PPSSPP", &save);
    assert!(guard.kernel_watched());
    assert_eq!(guard.state(), InUse::NotDetected);
    drop(guard);

    // A second name for the file means writes could bypass the leased path:
    // no lease, and the uninspectable process leaves the answer unknown.
    fs::hard_link(&save, dir.path().join("other-name.bin")).unwrap();
    let guard = observer.guard("PPSSPP", &save);
    assert!(!guard.kernel_watched());
    assert_eq!(
        guard.state(),
        InUse::Unknown,
        "absence of evidence is not a safe point"
    );

    // An uninspectable process that names the emulator is active, lease or not.
    let hidden = proc.add(2, "bwrap", "bwrap --args 40 PPSSPPSDL", USER);
    fs::set_permissions(hidden.join("fd"), fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        observer.guard("PPSSPP", &save).state(),
        InUse::EmulatorRunning
    );

    for path in [opaque, hidden] {
        fs::set_permissions(path.join("fd"), fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn an_unreadable_process_list_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    fs::hard_link(&save, dir.path().join("second")).unwrap(); // no lease to lean on
    let observer = ProcessObserver::with_proc_root(dir.path().join("no-such-proc"));
    assert_eq!(observer.observe("DuckStation", &save), InUse::Unknown);
}

#[test]
fn snapshots_through_the_real_check_are_verified_only_at_a_safe_point() {
    for emulator in SAFE_POINT_EMULATORS {
        let bench = Bench::new();
        let save = bench.file("save.bin", b"progress");
        let proc = Proc::new();
        let mut ask = request("dev", &[&save], 1);
        ask.provenance.emulator = emulator.to_string();

        let manifest = bench
            .store
            .capture(&ask, &proc.observer())
            .unwrap()
            .manifest()
            .clone();
        assert_eq!(
            manifest.body.boundary,
            CaptureBoundary::EmulatorClosedObserved,
            "{emulator}"
        );
        assert!(
            manifest
                .body
                .checks
                .iter()
                .any(|check| check.contains("kernel confirmed")),
            "{emulator}"
        );

        // The emulator starts: the next snapshot is refused, even best-effort.
        fs::write(&save, b"more progress").unwrap();
        proc.add(
            900,
            &emulator.to_lowercase(),
            &format!("flatpak run {emulator}"),
            USER,
        );
        let mut name = match emulator {
            "Dolphin" => "dolphin-emu".to_string(),
            "PPSSPP" => "PPSSPPSDL".to_string(),
            other => other.to_lowercase(),
        };
        if emulator == "PCSX2" {
            name = "pcsx2-qt".into();
        }
        proc.add(901, &name, &format!("/usr/bin/{name}"), USER);
        ask.captured_unix = Some(2);
        ask.allow_unproven_closure = true;
        let refused = bench.store.capture(&ask, &proc.observer()).unwrap_err();
        assert!(
            matches!(refused, ContinuityError::InUse(_)),
            "{emulator}: {refused}"
        );
        assert_eq!(bench.store.list().len(), 1, "{emulator}");
    }
}

impl Proc {
    /// A process whose executable can be read, as any process of the same
    /// user can: `exe` points at `executable`.
    fn readable(&self, pid: u32, executable: &str, cmdline: &str) -> PathBuf {
        let name = executable.rsplit('/').next().unwrap_or(executable);
        let dir = self.add(pid, &name[..name.len().min(15)], cmdline, USER);
        std::os::unix::fs::symlink(executable, dir.join("exe")).unwrap();
        dir
    }

    /// A process whose executable cannot be read (another user's): `exe`
    /// exists but is not a link that can be followed.
    fn hidden(&self, pid: u32, comm: &str, cmdline: &str) -> PathBuf {
        let dir = self.add(pid, comm, cmdline, USER);
        fs::write(dir.join("exe"), b"").unwrap();
        dir
    }
}

/// Commands that mention an emulator without being one. The first four are
/// the ones that really made every RetroArch save read "in use" on
/// 2026-11-20 (other sessions' test runs) plus the probe's own command.
const NOT_EMULATORS: &[(&str, &str)] = &[
    (
        "/home/u/.rustup/toolchains/1.97.1/bin/cargo",
        "cargo test -p archivefs-gui --lib -- gui_v2::retroarch_journey_tests",
    ),
    (
        "/usr/bin/bash",
        "/bin/bash -c scripts/cargo-iso test -p archivefs-gui --lib -- gui_v2::retroarch_journey_tests gui_v2::game_rename",
    ),
    (
        "/usr/bin/bwrap",
        "bwrap --bind / / -- cargo test -p archivefs-gui --lib -- gui_v2::retroarch_journey_tests",
    ),
    (
        "/usr/bin/python3.12",
        "python3 -B scripts/qa/emulator_safe_point_probe.py --work /mnt/x/probe-retroarch --only retroarch-native,retroarch-flatpak",
    ),
    (
        "/usr/bin/vim",
        "vim /home/u/.config/retroarch/retroarch.cfg",
    ),
    (
        "/usr/bin/less",
        "less /usr/lib/x86_64-linux-gnu/libretro/snes9x_libretro.so",
    ),
    ("/usr/bin/grep", "grep -r libretro /home/u/notes"),
    (
        "/usr/bin/nautilus",
        "nautilus /home/u/.var/app/org.libretro.RetroArch/config/retroarch/saves",
    ),
    (
        "/usr/bin/bash",
        "bash -c echo retroarch pcsx2 duckstation rpcs3 ppsspp",
    ),
    (
        "/usr/bin/python3.12",
        "python3 notes.py --about retroarch -L not-a-core.txt",
    ),
    ("/usr/bin/tail", "tail -f /home/u/.config/rpcs3/RPCS3.log"),
    (
        "/usr/bin/vim",
        "vim duckstation-settings.ini ppsspp.ini PCSX2.ini",
    ),
    (
        "/usr/bin/git",
        "git log --grep=ppsspp --grep=duckstation --grep=rpcs3",
    ),
];

#[test]
fn a_command_that_only_mentions_an_emulator_is_not_that_emulator() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    let proc = Proc::new();
    for (index, (executable, cmdline)) in NOT_EMULATORS.iter().enumerate() {
        proc.readable(1000 + index as u32, executable, cmdline);
    }
    for emulator in ALL_SIX {
        assert_eq!(
            proc.synthetic().observe(emulator, &save),
            InUse::NotDetected,
            "{emulator}: none of these commands is the emulator"
        );
    }
    // One at a time as well, so no single entry hides behind another.
    for (executable, cmdline) in NOT_EMULATORS {
        let proc = Proc::new();
        proc.readable(4242, executable, cmdline);
        assert_eq!(
            proc.synthetic().observe("RetroArch", &save),
            InUse::NotDetected,
            "{cmdline}"
        );
    }
}

#[test]
fn the_real_emulator_is_still_seen_in_every_way_it_is_started() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    let cases: &[(&str, &str, &str)] = &[
        (
            "RetroArch",
            "/usr/games/retroarch",
            "/usr/games/retroarch --verbose --config /x/ro.cfg -L /cores/gambatte_libretro.so game.gb",
        ),
        ("RetroArch", "/app/bin/retroarch", "retroarch"),
        // A front end under another name, loading a core.
        (
            "RetroArch",
            "/opt/frontend/emu",
            "emu -L /cores/snes9x_libretro.so game.sfc",
        ),
        (
            "RetroArch",
            "/opt/frontend/emu",
            "emu --libretro=/cores/mgba_libretro.so game.gba",
        ),
        // A wrapper script named like the emulator, run by a shell.
        (
            "RetroArch",
            "/usr/bin/bash",
            "bash /home/u/.local/bin/retroarch --fullscreen",
        ),
        (
            "PCSX2",
            "/usr/bin/bash",
            "/usr/bin/env bash /home/u/.local/bin/pcsx2-qt",
        ),
        (
            "RetroArch",
            "/usr/bin/flatpak",
            "flatpak run org.libretro.RetroArch",
        ),
        ("RetroArch", "/usr/bin/bwrap", "bwrap --args 41 retroarch"),
        (
            "DuckStation",
            "/tmp/.mount_DuckStAbCd/usr/bin/duckstation-qt",
            "/home/u/Applications/DuckStation.AppImage",
        ),
        ("RPCS3", "/app/bin/rpcs3", "rpcs3 --no-gui"),
        ("PPSSPP", "/app/bin/PPSSPPSDL", "PPSSPPSDL game.iso"),
    ];
    for (emulator, executable, cmdline) in cases {
        let proc = Proc::new();
        // Alongside the commands that only mention it.
        for (index, (other, text)) in NOT_EMULATORS.iter().enumerate() {
            proc.readable(1000 + index as u32, other, text);
        }
        proc.readable(4242, executable, cmdline);
        assert_eq!(
            proc.synthetic().observe(emulator, &save),
            InUse::EmulatorRunning,
            "{emulator}: `{cmdline}`"
        );
    }
    // RetroDECK runs RetroArch inside its own Flatpak scope: the scope says so.
    let proc = Proc::new();
    let scoped = proc.add(
        7,
        "bwrap",
        "bwrap --args 40 retrodeck.sh",
        "0::/app.slice/app-flatpak-net.retrodeck.retrodeck-7.scope\n",
    );
    std::os::unix::fs::symlink("/usr/bin/bwrap", scoped.join("exe")).unwrap();
    assert_eq!(
        proc.observer().observe("RetroArch", &save),
        InUse::EmulatorRunning
    );
}

#[test]
fn a_process_that_cannot_be_classified_still_refuses_when_it_names_the_emulator() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    // Its executable cannot be read, so what it is cannot be established.
    let proc = Proc::new();
    proc.hidden(55, "python3", "python3 launcher.py --start retroarch");
    assert_eq!(
        proc.observer().observe("RetroArch", &save),
        InUse::EmulatorRunning,
        "unclassifiable and naming the emulator: fail closed"
    );
    assert_eq!(
        proc.observer().observe("PCSX2", &save),
        InUse::NotDetected,
        "it does not name PCSX2"
    );
    // The same command from a process that *can* be read is just a script.
    let proc = Proc::new();
    proc.readable(
        55,
        "/usr/bin/python3.12",
        "python3 launcher.py --start retroarch",
    );
    assert_eq!(
        proc.observer().observe("RetroArch", &save),
        InUse::NotDetected
    );
}

#[test]
fn a_process_that_vanishes_while_being_looked_at_is_not_an_emulator() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    let proc = Proc::new();
    // Gone between reading its command line and its executable: no `exe`.
    proc.add(
        60,
        "cargo",
        "cargo test gui_v2::retroarch_journey_tests",
        USER,
    );
    // Not a process at all.
    fs::create_dir(proc.0.path().join("self")).unwrap();
    assert_eq!(
        proc.observer().observe("RetroArch", &save),
        InUse::NotDetected
    );
    // A real emulator that is still there is not hidden by the ones that left.
    proc.readable(62, "/usr/games/retroarch", "/usr/games/retroarch");
    assert_eq!(
        proc.observer().observe("RetroArch", &save),
        InUse::EmulatorRunning
    );
}

#[test]
fn an_entry_that_exists_but_cannot_be_read_at_all_keeps_the_answer_unknown() {
    // Fail closed: something is there and nothing about it can be read.
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    fs::hard_link(&save, dir.path().join("second-name")).unwrap(); // no lease to lean on
    let proc = Proc::new();
    fs::create_dir(proc.0.path().join("61")).unwrap();
    assert_eq!(proc.observer().observe("RetroArch", &save), InUse::Unknown);
}

// ---- Regression tests for the independent review of 2026-11-21 ----

#[test]
fn an_emulator_started_through_a_loader_or_a_runner_is_still_seen() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    let cases: &[(&str, &str, &str)] = &[
        // The dynamic loader runs the program named in its arguments.
        (
            "RetroArch",
            "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
            "/lib64/ld-linux-x86-64.so.2 /opt/bundle/bin/retroarch",
        ),
        (
            "RetroArch",
            "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
            "/lib64/ld-linux-x86-64.so.2 --argv0 retroarch /opt/bundle/bin/retroarch",
        ),
        (
            "PCSX2",
            "/usr/bin/valgrind",
            "valgrind --tool=memcheck /usr/bin/pcsx2-qt",
        ),
        ("RPCS3", "/usr/bin/box64", "box64 /opt/rpcs3/rpcs3"),
        (
            "PPSSPP",
            "/usr/bin/gamemoderun",
            "gamemoderun /usr/bin/PPSSPPSDL game.iso",
        ),
        // Attached and bundled forms of the core option.
        (
            "RetroArch",
            "/opt/frontend/emu",
            "emu -L/usr/lib/libretro/snes9x_libretro.so game.sfc",
        ),
        (
            "RetroArch",
            "/opt/frontend/emu",
            "emu -fL /cores/mgba_libretro.so game.gba",
        ),
        (
            "RetroArch",
            "/opt/frontend/emu",
            "emu --libr=/cores/gambatte_libretro.so game.gb",
        ),
        // The binary was replaced while running (an update): same program.
        (
            "RetroArch",
            "/usr/local/bin/retroarch (deleted)",
            "ra --menu",
        ),
        (
            "DuckStation",
            "/home/u/Applications/duckstation-qt (deleted)",
            "ds",
        ),
    ];
    for (emulator, executable, cmdline) in cases {
        let proc = Proc::new();
        // comm is the executable's own name, which for a loader or a
        // differently named link says nothing.
        let dir = proc.add(4242, "x", cmdline, USER);
        std::os::unix::fs::symlink(executable, dir.join("exe")).unwrap();
        assert_eq!(
            proc.synthetic().observe(emulator, &save),
            InUse::EmulatorRunning,
            "{emulator}: `{cmdline}` ({executable})"
        );
    }
    // A loader or runner running something else is not an emulator.
    let proc = Proc::new();
    let dir = proc.add(
        1,
        "x",
        "/lib64/ld-linux-x86-64.so.2 /usr/bin/python3 notes.py retroarch",
        USER,
    );
    std::os::unix::fs::symlink(
        "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
        dir.join("exe"),
    )
    .unwrap();
    assert_eq!(
        proc.synthetic().observe("RetroArch", &save),
        InUse::NotDetected
    );
}

// ---- Regression tests for the delta review of cdc75795 ----

#[test]
fn a_runner_in_front_of_a_renamed_front_end_or_with_its_own_valued_options_is_still_seen() {
    let dir = tempfile::tempdir().unwrap();
    let save = save_file(&dir);
    let cases: &[(&str, &str)] = &[
        // The program is not named like the emulator, but it loads a core:
        // the runner rule must not stop the core rule from being applied.
        (
            "/usr/bin/valgrind",
            "valgrind /opt/ra/ra.bin -L /c/snes9x_libretro.so game.sfc",
        ),
        (
            "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
            "/lib64/ld-linux-x86-64.so.2 /opt/ra/ra.bin -L /c/mgba_libretro.so",
        ),
        (
            "/usr/bin/box64",
            "box64 /opt/ra/frontend --libretro=/c/gambatte_libretro.so",
        ),
        // qemu's own -L takes a value (a folder), which is not the program.
        (
            "/usr/bin/qemu-x86_64",
            "qemu-x86_64 -L /usr/x86_64-linux-gnu /usr/bin/retroarch",
        ),
        (
            "/usr/bin/qemu-x86_64",
            "qemu-x86_64 -cpu max -E LANG=C /opt/bin/retroarch --menu",
        ),
        // Bundled short options with the core path attached.
        (
            "/opt/frontend/emu",
            "emu -fL/cores/snes9x_libretro.so game.sfc",
        ),
    ];
    for (executable, cmdline) in cases {
        let proc = Proc::new();
        let dir = proc.add(4242, "x", cmdline, USER);
        std::os::unix::fs::symlink(executable, dir.join("exe")).unwrap();
        assert_eq!(
            proc.observer().observe("RetroArch", &save),
            InUse::EmulatorRunning,
            "`{cmdline}`"
        );
    }
    // And the same runners running something else stay harmless.
    for (executable, cmdline) in [
        (
            "/usr/bin/qemu-x86_64",
            "qemu-x86_64 -L /usr/x86_64-linux-gnu /usr/bin/python3 notes.py retroarch",
        ),
        (
            "/usr/bin/valgrind",
            "valgrind /usr/bin/less /usr/lib/libretro/snes9x_libretro.so",
        ),
    ] {
        let proc = Proc::new();
        let dir = proc.add(1, "x", cmdline, USER);
        std::os::unix::fs::symlink(executable, dir.join("exe")).unwrap();
        assert_eq!(
            proc.observer().observe("RetroArch", &save),
            InUse::NotDetected,
            "`{cmdline}`"
        );
    }
}
