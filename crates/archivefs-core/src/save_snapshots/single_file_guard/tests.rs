use super::*;
use crate::memory_card_inventory::restore_guard::ProcScanQuiescence;
use crate::save_snapshots::history::restore_quiescence;
use crate::save_snapshots::tree_restore::safety::DirectorySaveBinding;
use crate::save_snapshots::{
    EmulatorUseStatus, SaveArtifactType, SaveLocation, SaveProvenance, SaveSnapshotRequest,
    SaveTargetBinding, create_snapshot, list_single_file_restore_receipts,
};
use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

struct Fixture {
    dir: tempfile::TempDir,
    save: PathBuf,
    snapshot: SaveSnapshot,
    confirmed: SaveRestorePreflight,
}

fn fixture() -> Fixture {
    let dir = tempdir().unwrap();
    let save = dir.path().join("saves").join("game.srm");
    fs::create_dir_all(save.parent().unwrap()).unwrap();
    fs::write(&save, b"good progress").unwrap();
    let snapshot = create_snapshot(&SaveSnapshotRequest {
        location: SaveLocation {
            path: save.clone(),
            emulator: Some("RetroArch".into()),
            profile: Some("profile".into()),
            artifact_type: SaveArtifactType::Sram,
            provenance: SaveProvenance::EmulatorProfile,
        },
        game_identity: Some("LooseRomSha256:abc".into()),
        platform: Some("Game Boy".into()),
        storage_root: dir.path().join("snapshots"),
        available_space_bytes: Some(1 << 20),
        snapshot_id: Some("snap".into()),
        now_unix_seconds: Some(100),
        emulator_use: EmulatorUseStatus::NotDetected,
    })
    .unwrap();
    fs::write(&save, b"newer progress").unwrap();
    let confirmed = build_single_file_restore_preflight(
        &snapshot,
        &SaveTargetBinding::from_snapshot(&snapshot),
        SaveQuiescenceRequirement::ConfirmedClosed,
    );
    assert!(confirmed.ready());
    Fixture {
        dir,
        save,
        snapshot,
        confirmed,
    }
}

fn options(f: &Fixture) -> SaveRestoreApplyOptions {
    SaveRestoreApplyOptions {
        backup_root: f.dir.path().join("backups"),
        receipt_root: f.dir.path().join("receipts"),
        now_unix_seconds: 200,
    }
}

fn proc(dir: &Path, name: &str, processes: &[(&str, &str, &[u8])]) -> ProcScanQuiescence {
    let root = dir.join(name);
    fs::create_dir_all(&root).unwrap();
    for (pid, comm, cmdline) in processes {
        let base = root.join(pid);
        fs::create_dir_all(base.join("fd")).unwrap();
        fs::write(base.join("comm"), format!("{comm}\n")).unwrap();
        fs::write(base.join("cmdline"), cmdline).unwrap();
        fs::write(base.join("status"), "State:\tS (sleeping)\n").unwrap();
    }
    ProcScanQuiescence::with_root(root)
}

fn restore(
    f: &Fixture,
    observer: &dyn QuiescenceProvider,
) -> Result<GuardedRestore, SaveRestoreError> {
    restore_single_file_guarded(
        &f.snapshot,
        &f.confirmed,
        StateEmulator::RetroArch,
        observer,
        &options(f),
    )
}

fn refused(result: Result<GuardedRestore, SaveRestoreError>) -> SaveRestoreRefusal {
    match result {
        Err(SaveRestoreError::Refused(reason)) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn closed_retroarch_restores_and_undoes_under_the_guard() {
    let f = fixture();
    let observer = proc(f.dir.path(), "closed", &[("10", "bash", b"/bin/bash\0")]);
    let done = restore(&f, &observer).unwrap();
    assert!(!done.opened_during_restore);
    assert_eq!(fs::read(&f.save).unwrap(), b"good progress");
    let receipts =
        list_single_file_restore_receipts(&options(&f).receipt_root, &options(&f).backup_root);
    assert_eq!(receipts.len(), 1);
    undo_single_file_guarded(&receipts[0], StateEmulator::RetroArch, &observer).unwrap();
    assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");
}

#[test]
fn running_retroarch_refuses_and_leaves_the_save_untouched() {
    for (comm, cmdline) in [
        ("retroarch", &b"/usr/bin/retroarch\0-L\0core.so\0"[..]),
        ("RetroArch", b"/opt/RetroArch-Linux-x86_64.AppImage\0"),
    ] {
        let f = fixture();
        let observer = proc(f.dir.path(), "running", &[("11", comm, cmdline)]);
        assert_eq!(
            refused(restore(&f, &observer)),
            SaveRestoreRefusal::EmulatorRunning
        );
        assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");
        assert!(!options(&f).backup_root.exists());
    }
}

#[test]
fn wrapper_launched_retroarch_is_seen_by_its_launch_route() {
    for cmdline in [
        &b"flatpak\0run\0org.libretro.RetroArch\0"[..],
        b"bwrap\0--args\x0042\0org.libretro.RetroArch\0",
    ] {
        let f = fixture();
        let launcher = if cmdline.starts_with(b"flatpak") {
            "flatpak"
        } else {
            "bwrap"
        };
        let observer = proc(f.dir.path(), "wrapped", &[("12", launcher, cmdline)]);
        assert_eq!(
            refused(restore(&f, &observer)),
            SaveRestoreRefusal::EmulatorRunning
        );
        assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");
    }
    // A renamed child inside the Flatpak scope is found by its cgroup.
    let f = fixture();
    let observer = proc(f.dir.path(), "scope", &[("13", "worker", b"/app/bin/x\0")]);
    fs::write(
        f.dir.path().join("scope/13/cgroup"),
        "0::/user.slice/app.slice/app-flatpak-org.libretro.RetroArch-4242.scope\n",
    )
    .unwrap();
    assert_eq!(
        refused(restore(&f, &observer)),
        SaveRestoreRefusal::EmulatorRunning
    );
}

#[test]
fn another_program_holding_the_exact_save_open_refuses() {
    let f = fixture();
    let observer = proc(
        f.dir.path(),
        "holder",
        &[("14", "tool", b"/usr/bin/tool\0")],
    );
    std::os::unix::fs::symlink(&f.save, f.dir.path().join("holder/14/fd/3")).unwrap();
    assert_eq!(
        refused(restore(&f, &observer)),
        SaveRestoreRefusal::SaveInUse
    );
    assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");
}

#[test]
fn an_uninspectable_process_is_unknown_and_refuses() {
    let f = fixture();
    let observer = proc(f.dir.path(), "ambiguous", &[("15", "bash", b"/bin/bash\0")]);
    fs::remove_file(f.dir.path().join("ambiguous/15/comm")).unwrap();
    fs::create_dir(f.dir.path().join("ambiguous/15/comm")).unwrap();
    assert_eq!(
        refused(restore(&f, &observer)),
        SaveRestoreRefusal::EmulatorStateUnknown
    );
    assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");
}

#[test]
fn a_missing_process_listing_is_unknown_not_closed() {
    let f = fixture();
    let observer = ProcScanQuiescence::with_root(f.dir.path().join("no-proc-here"));
    assert_eq!(
        refused(restore(&f, &observer)),
        SaveRestoreRefusal::EmulatorStateUnknown
    );
}

#[test]
fn a_missing_target_cannot_be_proved_closed() {
    let f = fixture();
    fs::remove_file(&f.save).unwrap();
    let observer = proc(f.dir.path(), "closed", &[("10", "bash", b"/bin/bash\0")]);
    let confirmed = build_single_file_restore_preflight(
        &f.snapshot,
        &SaveTargetBinding::from_snapshot(&f.snapshot),
        SaveQuiescenceRequirement::ConfirmedClosed,
    );
    let result = restore_single_file_guarded(
        &f.snapshot,
        &confirmed,
        StateEmulator::RetroArch,
        &observer,
        &options(&f),
    );
    assert_eq!(refused(result), SaveRestoreRefusal::EmulatorStateUnknown);
    assert!(!f.save.exists());
}

#[test]
fn unsupported_emulators_stay_unknown() {
    let f = fixture();
    let observer = proc(f.dir.path(), "closed", &[("10", "bash", b"/bin/bash\0")]);
    let result = restore_single_file_guarded(
        &f.snapshot,
        &f.confirmed,
        StateEmulator::Dolphin,
        &observer,
        &options(&f),
    );
    assert_eq!(refused(result), SaveRestoreRefusal::EmulatorStateUnknown);
}

#[test]
fn a_save_changed_after_confirmation_refuses() {
    let f = fixture();
    let observer = proc(f.dir.path(), "closed", &[("10", "bash", b"/bin/bash\0")]);
    fs::write(&f.save, b"changed after the preview").unwrap();
    assert_eq!(
        refused(restore(&f, &observer)),
        SaveRestoreRefusal::DestinationChanged
    );
    assert_eq!(fs::read(&f.save).unwrap(), b"changed after the preview");
}

/// Reports closed to the first `closed_for` observations, then Running: a
/// RetroArch that starts while the restore is in flight.
struct Starts {
    closed_for: Cell<u32>,
}
impl QuiescenceProvider for Starts {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        let left = self.closed_for.get();
        if left > 0 {
            self.closed_for.set(left - 1);
            EmulatorQuiescence::Closed
        } else {
            EmulatorQuiescence::Running
        }
    }
}

#[test]
fn an_emulator_starting_before_the_check_is_refused_and_after_it_is_reported() {
    // Starts before the first observation: refused, nothing written.
    let f = fixture();
    let observer = Starts {
        closed_for: Cell::new(0),
    };
    assert_eq!(
        refused(restore(&f, &observer)),
        SaveRestoreRefusal::EmulatorRunning
    );
    assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");

    // Starts after the last pre-write check (open + gate = 2 observations),
    // during the write: the restore stands (receipt allows Undo) but is
    // reported as possibly opened, never as clean.
    let f = fixture();
    let observer = Starts {
        closed_for: Cell::new(2),
    };
    let done = restore(&f, &observer).unwrap();
    assert!(done.opened_during_restore);
    assert_eq!(fs::read(&f.save).unwrap(), b"good progress");
    assert!(done.result.receipt.receipt_path.is_file());
}

#[test]
fn undo_refuses_while_the_emulator_runs_and_keeps_the_restored_bytes() {
    let f = fixture();
    let closed = proc(f.dir.path(), "closed", &[("10", "bash", b"/bin/bash\0")]);
    let done = restore(&f, &closed).unwrap();
    let running = proc(
        f.dir.path(),
        "running",
        &[("11", "retroarch", b"/usr/bin/retroarch\0")],
    );
    let undo = undo_single_file_guarded(&done.result.receipt, StateEmulator::RetroArch, &running);
    assert!(matches!(
        undo,
        Err(SaveRestoreError::Refused(
            SaveRestoreRefusal::EmulatorRunning
        ))
    ));
    assert_eq!(fs::read(&f.save).unwrap(), b"good progress");
}

#[test]
fn the_receipt_is_durable_before_the_save_changes() {
    // A crash after publication but before the final receipt rewrite leaves
    // the journalled receipt, which still undoes exactly the published bytes.
    let f = fixture();
    let closed = proc(f.dir.path(), "closed", &[("10", "bash", b"/bin/bash\0")]);
    let done = restore(&f, &closed).unwrap();
    let journalled =
        list_single_file_restore_receipts(&options(&f).receipt_root, &options(&f).backup_root);
    assert_eq!(journalled.len(), 1);
    assert_eq!(
        journalled[0].post_restore_actual_sha256,
        done.result.receipt.snapshot_sha256
    );
    // A refused restore leaves no receipt behind.
    let g = fixture();
    let running = proc(
        g.dir.path(),
        "running",
        &[("11", "retroarch", b"retroarch\0")],
    );
    let _ = restore(&g, &running);
    assert!(
        list_single_file_restore_receipts(&options(&g).receipt_root, &options(&g).backup_root)
            .is_empty()
    );
}

/// Closed for the first observation(s), then Running: lets the pre-publication
/// gate see a RetroArch that started after the first proof.
#[test]
fn an_emulator_starting_between_proof_and_publication_blocks_the_write() {
    let f = fixture();
    // observation 1 (open) Closed, observation 2 (pre-publish gate) Running.
    let observer = Starts {
        closed_for: Cell::new(1),
    };
    let result = restore(&f, &observer);
    // The gate (second observation) refuses before the rename.
    assert_eq!(refused(result), SaveRestoreRefusal::SaveInUse);
    assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");
    assert!(
        list_single_file_restore_receipts(&options(&f).receipt_root, &options(&f).backup_root)
            .is_empty()
    );
}

/// A process whose descriptors cannot be read (as for root services and
/// privilege-separated daemons) but whose name, command line and cgroup can.
fn uninspectable(
    f: &Fixture,
    name: &str,
    comm: &str,
    cmdline: &[u8],
    cgroup: &str,
) -> ProcScanQuiescence {
    let observer = proc(f.dir.path(), name, &[("16", comm, cmdline)]);
    let base = f.dir.path().join(name).join("16");
    fs::remove_dir(base.join("fd")).unwrap();
    fs::write(base.join("fd"), b"").unwrap();
    fs::write(base.join("cgroup"), cgroup).unwrap();
    observer
}

#[test]
fn an_unreadable_but_identified_service_is_covered_by_an_intact_lease() {
    let f = fixture();
    let observer = uninspectable(
        &f,
        "svc",
        "sshd",
        b"/usr/sbin/sshd\0",
        "0::/system.slice/ssh.service\n",
    );
    let done = restore(&f, &observer).unwrap();
    assert!(!done.opened_during_restore);
    assert_eq!(fs::read(&f.save).unwrap(), b"good progress");
}

#[test]
fn an_unreadable_process_with_any_emulator_marker_is_running_not_waived() {
    for (comm, cmdline, cgroup) in [
        (
            "wrapper",
            &b"/opt/launch/retroarch-wrapper.sh\0"[..],
            "0::/user.slice\n",
        ),
        (
            "wrapper",
            b"/opt/x/start\0--core\0libretro_gambatte\0",
            "0::/user.slice\n",
        ),
        (
            "frontend",
            b"/app/bin/frontend\0",
            "0::/app.slice/app-flatpak-net.retrodeck.retrodeck-1.scope\n",
        ),
        (
            "frontend",
            b"/app/bin/frontend\0",
            "0::/app.slice/app-flatpak-org.libretro.RetroArch-9.scope\n",
        ),
    ] {
        let f = fixture();
        let observer = uninspectable(&f, "marked", comm, cmdline, cgroup);
        assert_eq!(
            refused(restore(&f, &observer)),
            SaveRestoreRefusal::EmulatorRunning
        );
        assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");
    }
}

#[test]
fn an_unreadable_process_with_unreadable_identity_stays_unknown() {
    let f = fixture();
    let observer = uninspectable(&f, "hidden", "sshd", b"/usr/sbin/sshd\0", "");
    fs::remove_file(f.dir.path().join("hidden/16/cgroup")).unwrap();
    fs::create_dir(f.dir.path().join("hidden/16/cgroup")).unwrap();
    assert_eq!(
        refused(restore(&f, &observer)),
        SaveRestoreRefusal::EmulatorStateUnknown
    );
}

#[test]
fn renamed_retroarch_binaries_are_refused_not_cleared() {
    for name in ["retroarch32", "retroarch_vulkan", "RetroArch.bin"] {
        let f = fixture();
        let cmdline = format!("/opt/{name}\0");
        let observer = proc(f.dir.path(), "renamed", &[("17", name, cmdline.as_bytes())]);
        assert_eq!(
            refused(restore(&f, &observer)),
            SaveRestoreRefusal::EmulatorRunning,
            "{name}"
        );
    }
}

/// Live acceptance probe (run by hand with `--ignored --nocapture`): the
/// production observer against this machine's real /proc, as the current
/// user. `PROBE_FILE` names a disposable synthetic file (default: a fresh
/// temp file). Reads only; writes nothing.
#[test]
#[ignore = "reads the live /proc; run manually"]
fn live_proc_probe() {
    let dir = tempdir().unwrap();
    let save = match std::env::var_os("PROBE_FILE") {
        Some(path) => PathBuf::from(path),
        None => {
            let save = dir.path().join("probe.srm");
            fs::write(&save, b"synthetic").unwrap();
            save
        }
    };
    let observer = ProcScanQuiescence::new();
    let binding = closure_binding(StateEmulator::RetroArch, &save);
    let report = observer.report(&binding);
    println!(
        "STRICT state={:?} emulators={:?} holders={:?} unreadable={} listing_unavailable={}",
        report.state,
        report.emulator_processes,
        report.card_holders,
        report.unreadable_processes.len(),
        report.listing_unavailable
    );
    println!(
        "PRODUCTION restore_quiescence = {:?}",
        restore_quiescence(StateEmulator::RetroArch, &save, &observer)
    );
}

#[test]
fn readable_wrappers_non_utf8_paths_and_flatpak_branches_are_refused() {
    for (comm, cmdline) in [
        // A non-UTF-8 byte in the path must not hide the name.
        ("retroarch", &b"/home/Jos\xe9/bin/retroarch\0-L\0core\0"[..]),
        // Inspectable wrapper / AppRun carrying only a libretro core path.
        ("emu", b"/opt/emu\0-L\0/cores/x_libretro.so\0game.gb\0"),
        // Launcher forms with a branch suffix.
        ("flatpak", b"flatpak\0run\0org.libretro.RetroArch//stable\0"),
        (
            "flatpak",
            b"flatpak\0run\0--branch=stable\0org.libretro.RetroArch\0",
        ),
    ] {
        let f = fixture();
        let observer = proc(f.dir.path(), "readable", &[("18", comm, cmdline)]);
        assert_eq!(
            refused(restore(&f, &observer)),
            SaveRestoreRefusal::EmulatorRunning,
            "{comm} {:?}",
            String::from_utf8_lossy(cmdline)
        );
        assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");
    }
}

/// Closed to the first `closed_for` observations, then reports `CardInUse`
/// (a new reader, not recognisably the emulator): the gate must refuse it,
/// not wave it through as "not Running".
struct Reader {
    closed_for: Cell<u32>,
}
impl QuiescenceProvider for Reader {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        let left = self.closed_for.get();
        if left > 0 {
            self.closed_for.set(left - 1);
            EmulatorQuiescence::Closed
        } else {
            EmulatorQuiescence::CardInUse
        }
    }
}

#[test]
fn a_reader_appearing_before_publication_blocks_and_one_after_is_reported() {
    let f = fixture();
    assert_eq!(
        refused(restore(
            &f,
            &Reader {
                closed_for: Cell::new(1)
            }
        )),
        SaveRestoreRefusal::SaveInUse
    );
    assert_eq!(fs::read(&f.save).unwrap(), b"newer progress");
    let f = fixture();
    let done = restore(
        &f,
        &Reader {
            closed_for: Cell::new(2),
        },
    )
    .unwrap();
    assert!(done.opened_during_restore);
}
