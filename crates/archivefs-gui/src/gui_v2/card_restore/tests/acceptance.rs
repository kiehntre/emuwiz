//! Local acceptance with the PRODUCTION closure observer (real `/proc` scan),
//! real filesystem publication and a disposable synthetic card. Ignored by
//! default; run with
//! `EMUWIZ_CARD_RESTORE_ACCEPTANCE=1 scripts/cargo-iso test -p archivefs-gui --lib
//!  gui_v2::card_restore::tests::acceptance -- --ignored`.
//!
//! It never launches the real PCSX2 (the Flatpak always mounts the user's
//! production PCSX2 configuration) and never reads or writes a real card.
//! "PCSX2 running" is a stand-in process named `pcsx2-qt` (a copy of
//! `sleep`), which is exactly what the observer matches on, plus a process
//! holding the card open; "uncertain" is the same observer on a synthetic
//! `/proc` containing an unreadable process.

use super::*;
use crate::gui_v2::card_restore::recovery;
use archivefs_core::memory_card_inventory::restore_guard::ProcScanQuiescence;
use std::{os::unix::fs::PermissionsExt, process::Command, time::Duration};

fn wait_for(observer: &ProcScanQuiescence, card: &Path, wanted: EmulatorQuiescence) {
    let binding =
        archivefs_core::memory_card_inventory::restore_guard::ps2_card_binding(card, "acceptance");
    for _ in 0..100 {
        if observer.observe(&binding) == wanted {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "observer never reported {wanted:?}: {:?}",
        observer.report(&binding)
    );
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The production observer on a view of the real `/proc` that contains
/// only processes whose descriptor tables this user can read. This is the
/// acceptance stand-in for "PCSX2 is closed"; see the finding below.
fn readable_view() -> (tempfile::TempDir, ProcScanQuiescence) {
    let view = tempfile::tempdir().unwrap();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let name = entry.file_name();
        let dir = entry.path();
        let fully_readable = fs::read(dir.join("comm")).is_ok()
            && fs::read(dir.join("cmdline")).is_ok()
            && fs::read_dir(dir.join("fd")).is_ok_and(|fds| {
                fds.flatten().all(|fd| match fs::metadata(fd.path()) {
                    Ok(_) => true,
                    Err(error) => error.kind() == std::io::ErrorKind::NotFound,
                })
            });
        if name.to_string_lossy().parse::<u32>().is_ok() && fully_readable {
            let _ = std::os::unix::fs::symlink(entry.path(), view.path().join(&name));
        }
    }
    let observer = ProcScanQuiescence::with_root(view.path().to_path_buf());
    (view, observer)
}

#[test]
#[ignore = "local acceptance; set EMUWIZ_CARD_RESTORE_ACCEPTANCE=1"]
fn live_observer_acceptance_on_a_disposable_card() {
    if std::env::var_os("EMUWIZ_CARD_RESTORE_ACCEPTANCE").is_none() {
        eprintln!("skipped: EMUWIZ_CARD_RESTORE_ACCEPTANCE is not set");
        return;
    }
    let live = archivefs_core::save_snapshots::history::default_closure_observer();
    let world = World::new();
    let binding = archivefs_core::memory_card_inventory::restore_guard::ps2_card_binding(
        &world.card,
        "acceptance",
    );

    // 0. FINDING: on an ordinary desktop the real /proc contains processes
    //    whose descriptor tables a normal user cannot read (root services,
    //    sshd, ssh-agent...), so the production observer reports Unknown and
    //    every write is refused even with no emulator running.
    let report = live.report(&binding);
    eprintln!(
        "acceptance: live observer state {:?}; {} unreadable processes; {} emulator processes",
        report.state,
        report.unreadable_processes.len(),
        report.emulator_processes.len()
    );
    assert!(
        report.emulator_processes.is_empty(),
        "close every PS2 emulator before acceptance"
    );
    if report.state == EmulatorQuiescence::Unknown {
        assert!(create_card_snapshot(&world.snapshots, &world.location, &live).is_err());
        eprintln!("acceptance: real uncertain process state refused snapshot creation");
    }

    // 1. "Closed" (readable real processes only): snapshot -> one folder ->
    //    guarded restore -> verify -> Undo.
    let (_view, closed) = readable_view();
    wait_for(&closed, &world.card, EmulatorQuiescence::Closed);
    create_card_snapshot(&world.snapshots, &world.location, &closed).unwrap();
    let snapshot = load_card_history(&world.snapshots, &world.location, &closed).snapshots[0]
        .snapshot
        .clone();
    let snapshot_bytes = snapshot_card_bytes(&snapshot);
    let played = world.play();
    let folders = snapshot_folders(&snapshot, &world.location).unwrap();
    assert_eq!(folders.len(), 1);
    let raw = folders[0].raw_name.clone();
    if report.state == EmulatorQuiescence::Unknown {
        let preview = prepare_restore(&snapshot, &world.location, &raw, &world.dirs, 1).unwrap();
        assert!(apply_restore(&preview.plan, &live, &world.options).is_err());
        assert_eq!(world.card_bytes(), played);
        assert!(!preview.plan.backup_path.exists());
        discard_preview(&preview, &world.dirs);
        eprintln!("acceptance: real uncertain process state refused the restore; card unchanged");
    }
    let preview = prepare_restore(&snapshot, &world.location, &raw, &world.dirs, 2).unwrap();
    let done = apply_restore(&preview.plan, &closed, &world.options).unwrap();
    assert_eq!(&world.card_bytes()[ICON_DATA], &snapshot_bytes[ICON_DATA]);
    assert_eq!(fs::read(&done.result.backup_path).unwrap(), played);
    undo_restore(&done.journal_path, &closed, 3).unwrap();
    assert_eq!(world.card_bytes(), played);
    eprintln!(
        "acceptance: snapshot -> folder restore -> verify -> Undo passed on the real filesystem"
    );

    let preview = prepare_restore(&snapshot, &world.location, &raw, &world.dirs, 4).unwrap();
    let second = apply_restore(&preview.plan, &closed, &world.options).unwrap();
    let restored = world.card_bytes();

    // 2. A process named like PCSX2 runs: detected on the REAL /proc too.
    let stand_in_dir = tempfile::tempdir().unwrap();
    let stand_in = stand_in_dir.path().join("pcsx2-qt");
    fs::copy("/bin/sleep", &stand_in).unwrap();
    fs::set_permissions(&stand_in, fs::Permissions::from_mode(0o755)).unwrap();
    {
        let _running = Child(Command::new(&stand_in).arg("60").spawn().unwrap());
        wait_for(&live, &world.card, EmulatorQuiescence::Running);
        let (_view, viewed) = readable_view();
        for observer in [&live as &dyn QuiescenceProvider, &viewed] {
            assert!(create_card_snapshot(&world.snapshots, &world.location, observer).is_err());
            let preview =
                prepare_restore(&snapshot, &world.location, &raw, &world.dirs, 5).unwrap();
            assert!(apply_restore(&preview.plan, observer, &world.options).is_err());
            assert!(!preview.plan.backup_path.exists());
            discard_preview(&preview, &world.dirs);
            assert!(undo_restore(&second.journal_path, observer, 6).is_err());
            assert!(recovery::recover(&world.options.journal_dir, observer, 6).is_err());
        }
        assert_eq!(world.card_bytes(), restored);
        eprintln!(
            "acceptance: running PCSX2-named process refused snapshot, restore, Undo and recovery"
        );
    }

    // 3. Another process holds the card open: detected on the real /proc.
    {
        let _holder = Child(
            Command::new("sh")
                .arg("-c")
                .arg("exec 3<\"$1\"; exec sleep 60")
                .arg("sh")
                .arg(&world.card)
                .spawn()
                .unwrap(),
        );
        wait_for(&live, &world.card, EmulatorQuiescence::Running);
        assert!(undo_restore(&second.journal_path, &live, 7).is_err());
        assert_eq!(world.card_bytes(), restored);
        eprintln!("acceptance: a process holding the card open refused Undo");
    }

    // 4. Synthetic /proc with one unreadable process: Unknown, refused.
    let fake_proc = tempfile::tempdir().unwrap();
    let pid = fake_proc.path().join("4242");
    fs::create_dir_all(pid.join("fd")).unwrap();
    fs::write(pid.join("comm"), "mystery\n").unwrap();
    fs::set_permissions(pid.join("fd"), fs::Permissions::from_mode(0o000)).unwrap();
    let uncertain = ProcScanQuiescence::with_root(fake_proc.path().to_path_buf());
    assert_eq!(uncertain.observe(&binding), EmulatorQuiescence::Unknown);
    assert!(undo_restore(&second.journal_path, &uncertain, 8).is_err());
    fs::set_permissions(pid.join("fd"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(world.card_bytes(), restored);

    // Closed again: Undo of the second restore succeeds.
    let (_view, closed) = readable_view();
    wait_for(&closed, &world.card, EmulatorQuiescence::Closed);
    undo_restore(&second.journal_path, &closed, 9).unwrap();
    assert_eq!(world.card_bytes(), played);
    eprintln!("acceptance: all checks passed");
}
