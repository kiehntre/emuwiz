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
}

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
            proc.observer().observe(emulator, &save),
            InUse::EmulatorRunning,
            "{emulator} as `{cmdline}` must count as running"
        );
        // And no other emulator's saves are blocked by it.
        for other in SAFE_POINT_EMULATORS
            .iter()
            .filter(|other| !other.eq_ignore_ascii_case(emulator))
        {
            assert_ne!(
                proc.observer().observe(other, &save),
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
    for emulator in ["mGBA", "melonDS", "xemu", "Flycast", ""] {
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
        proc.observer().observe("Dolphin", &save),
        InUse::OpenByAProgram
    );
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
