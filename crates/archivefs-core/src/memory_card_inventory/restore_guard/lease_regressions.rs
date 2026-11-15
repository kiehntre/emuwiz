//! Kernel-lease protection of the card operations. Cards and processes are
//! synthetic. Competing writers are threads of this test process: the process
//! scan always skips its own pid, so to the scan they are exactly as invisible
//! as a process whose descriptor table cannot be read. Only the kernel lease
//! can catch them, which is the point.
use super::*;
use std::cell::RefCell;
use std::io::{BufRead, Write};
use std::os::fd::AsRawFd;
use std::thread::JoinHandle;

fn binding(world: &World) -> DirectorySaveBinding {
    ps2_card_binding(&world.card, "synthetic")
}

/// Fake processes whose descriptor tables cannot be read.
fn uninspectable(entries: &[(&str, &str, &str)]) -> tempfile::TempDir {
    let all: Vec<_> = entries.iter().map(|(p, c, a)| (*p, *c, *a, None)).collect();
    let proc = fake_proc(&all);
    for (pid, _, _) in entries {
        fs::remove_dir(proc.path().join(pid).join("fd")).unwrap();
    }
    proc
}

fn lease(path: &Path) -> CardWriteLease {
    match CardWriteLease::acquire(path, None) {
        CardLeaseAttempt::Held(lease) => lease,
        _ => panic!("the kernel must grant a read lease on a synthetic card"),
    }
}

/// Wait until the kernel shows a lease on `inode` being broken.
fn wait_breaking(inode: u64) {
    let needle = format!(":{inode} ");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let locks = fs::read_to_string("/proc/locks").unwrap_or_default();
        if locks
            .lines()
            .any(|line| line.contains("BREAKING") && line.contains(&needle))
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "lease on inode {inode} never started breaking"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A competing program opening `path` for writing. The kernel holds its open
/// back while our lease is being broken; we wait until it is.
fn competing_writer(path: PathBuf) -> JoinHandle<fs::File> {
    let inode = fs::metadata(&path).unwrap().ino();
    let writer = std::thread::spawn(move || fs::OpenOptions::new().write(true).open(path).unwrap());
    wait_breaking(inode);
    writer
}

fn inode_of(file: &fs::File) -> u64 {
    file.metadata().unwrap().ino()
}

fn stage_path(world: &World) -> PathBuf {
    fs::read_dir(world.dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(STAGE_PREFIX)
        })
        .expect("staged file")
}

fn only_journal(world: &World) -> Ps2RestoreJournal {
    let entry = fs::read_dir(&world.journals)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    load_ps2_restore_journal(&entry).unwrap()
}

// ------------------------------------------------------------ the scan alone

#[test]
fn the_scan_alone_never_clears_an_uninspectable_process_whatever_its_name() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd: user@pts/0")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let report = scan.report(&binding(&world));
    assert_eq!(report.state, EmulatorQuiescence::Unknown);
    assert_eq!(report.unreadable_processes, [993]);
    assert!(report.covered_by_lease.is_empty());
    assert_eq!(scan.observe(&binding(&world)), EmulatorQuiescence::Unknown);
}

#[test]
fn under_a_card_lease_uninspectable_descriptors_are_left_to_the_kernel() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let held = lease(&world.card);
    let report = scan.report_under_card_lease(&binding(&world), &held);
    assert_eq!(report.state, EmulatorQuiescence::Closed);
    assert_eq!(report.covered_by_lease, [993]);
    assert!(report.unreadable_processes.is_empty());
    // A lease on some other file proves nothing about this card.
    let other = world.dir.path().join("other.bin");
    fs::write(&other, b"x").unwrap();
    let elsewhere = lease(&other);
    let report = scan.report_under_card_lease(&binding(&world), &elsewhere);
    assert_eq!(report.state, EmulatorQuiescence::Unknown);
    assert_eq!(report.unreadable_processes, [993]);
}

#[test]
fn a_writer_arriving_during_the_scan_breaks_the_lease() {
    let world = World::new();
    let proc = uninspectable(&[("994", "sshd", "sshd")]);
    let status = proc.path().join("994/status");
    let c_status = std::ffi::CString::new(status.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: valid C string, plain mode.
    assert_eq!(unsafe { libc::mkfifo(c_status.as_ptr(), 0o600) }, 0);
    let held = lease(&world.card);
    let card = world.card.clone();
    let inode = fs::metadata(&card).unwrap().ino();
    // The scan blocks on the FIFO until the writer has started breaking it.
    let release = std::thread::spawn(move || {
        let writer =
            std::thread::spawn(move || fs::OpenOptions::new().write(true).open(card).map(drop));
        wait_breaking(inode);
        let mut fifo = fs::OpenOptions::new().write(true).open(status).unwrap();
        fifo.write_all(b"State:\tS (sleeping)\n").unwrap();
        writer
    });
    let report = ProcScanQuiescence::with_root(proc.path().to_path_buf())
        .report_under_card_lease(&binding(&world), &held);
    drop(held);
    release.join().unwrap().join().unwrap().unwrap();
    assert!(report.card_open_for_writing);
    assert_eq!(report.state, EmulatorQuiescence::CardInUse);
    assert!(
        report.covered_by_lease.is_empty(),
        "nothing covered once broken"
    );
}

#[test]
fn emulator_identity_refuses_even_under_a_lease() {
    let world = World::new();
    let held = lease(&world.card);
    for args in [
        "sh -c exec /opt/emulators/pcsx2-qt -fullscreen",
        "env QT_QPA_PLATFORM=xcb /home/u/Apps/pcsx2-v2.2.0-linux-appimage-x64-Qt.AppImage",
        "bwrap --args 42 -- net.pcsx2.PCSX2",
    ] {
        let proc = uninspectable(&[("995", "sh", args)]);
        let report = ProcScanQuiescence::with_root(proc.path().to_path_buf())
            .report_under_card_lease(&binding(&world), &held);
        assert_eq!(report.emulator_processes, [995], "{args}");
        assert_eq!(report.state, EmulatorQuiescence::Running, "{args}");
    }
    // A Flatpak app scope marks every process of the app, whatever its name.
    for inspectable in [true, false] {
        let proc = fake_proc(&[("996", "QThread", "QThread", None)]);
        if !inspectable {
            fs::remove_dir(proc.path().join("996/fd")).unwrap();
        }
        fs::write(
            proc.path().join("996/cgroup"),
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-net.pcsx2.PCSX2-4242.scope\n",
        )
        .unwrap();
        let report = ProcScanQuiescence::with_root(proc.path().to_path_buf())
            .report_under_card_lease(&binding(&world), &held);
        assert_eq!(report.state, EmulatorQuiescence::Running);
        assert_eq!(report.emulator_processes, [996]);
    }
}

#[test]
fn unreadable_identity_stays_unknown_even_under_a_lease() {
    let world = World::new();
    let held = lease(&world.card);
    for leaf in ["comm", "cmdline", "cgroup"] {
        let proc = uninspectable(&[("997", "sshd", "sshd")]);
        let path = proc.path().join("997").join(leaf);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap(); // reading it fails, but not NotFound
        let report = ProcScanQuiescence::with_root(proc.path().to_path_buf())
            .report_under_card_lease(&binding(&world), &held);
        assert_eq!(report.unreadable_processes, [997], "{leaf}");
        assert_eq!(report.state, EmulatorQuiescence::Unknown, "{leaf}");
    }
}

#[test]
fn without_kernel_evidence_the_strict_scan_decides() {
    // A card reached through a link cannot be leased (no-follow open).
    let world = World::new();
    let link = world.dir.path().join("card-link.ps2");
    std::os::unix::fs::symlink(&world.card, &link).unwrap();
    assert!(matches!(
        CardWriteLease::acquire(&link, None),
        CardLeaseAttempt::Unavailable
    ));
    let proc = uninspectable(&[("998", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    assert_eq!(
        observe_card_now(&scan, &ps2_card_binding(&link, "synthetic"), &link),
        EmulatorQuiescence::Unknown
    );
    assert_eq!(
        observe_card_now(&scan, &binding(&world), &world.card),
        EmulatorQuiescence::Closed
    );
}

#[test]
fn the_kernel_refuses_a_lease_while_any_writer_or_writable_mapping_exists() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let writer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&world.card)
        .unwrap();
    assert!(matches!(
        CardWriteLease::acquire(&world.card, None),
        CardLeaseAttempt::Writers
    ));
    assert_eq!(
        observe_card_now(&scan, &binding(&world), &world.card),
        EmulatorQuiescence::CardInUse
    );
    assert!(matches!(
        apply_ps2_psu_restore_guarded(&world.plan(), &scan, &world.options()),
        Err(Ps2PsuRestoreError::CardInUse)
    ));
    assert_eq!(world.card_bytes(), world.original);
    // A writable shared mapping outlives its descriptor and still counts.
    let length = world.original.len();
    // SAFETY: mapping a regular file we own; unmapped below.
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            length,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            writer.as_raw_fd(),
            0,
        )
    };
    assert_ne!(mapping, libc::MAP_FAILED);
    drop(writer);
    assert!(matches!(
        CardWriteLease::acquire(&world.card, None),
        CardLeaseAttempt::Writers
    ));
    // SAFETY: the mapping created above.
    unsafe { libc::munmap(mapping, length) };
    assert!(matches!(
        CardWriteLease::acquire(&world.card, None),
        CardLeaseAttempt::Held(_)
    ));
}

#[test]
fn leases_are_trusted_only_on_local_filesystems_that_cannot_be_bypassed() {
    for magic in [
        0xEF53_i64,
        0x5846_5342,
        0x9123_683E,
        0xF2F5_2010,
        0x0102_1994,
    ] {
        assert!(card_lease::lease_trusted_filesystem(magic), "{magic:#x}");
    }
    // FUSE (mergerfs, rclone), overlayfs, NFS, SMB/CIFS, 9p, ecryptfs.
    for magic in [
        0x6573_5546_i64,
        0x794C_7630,
        0x6969,
        0xFF53_4D42,
        0xFE53_4D42,
        0x0102_1997,
        0xF15F,
    ] {
        assert!(!card_lease::lease_trusted_filesystem(magic), "{magic:#x}");
    }
    // A hard-linked card is never leased (the strict scan decides).
    let world = World::new();
    fs::hard_link(&world.card, world.dir.path().join("alias.ps2")).unwrap();
    assert!(matches!(
        CardWriteLease::acquire(&world.card, None),
        CardLeaseAttempt::Unavailable
    ));
}

#[test]
fn a_read_only_holder_the_scan_cannot_see_is_refused() {
    // The exclusive probe sees every descriptor, readers included.
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let reader = fs::File::open(&world.card).unwrap();
    assert!(matches!(
        CardWriteLease::acquire(&world.card, None),
        CardLeaseAttempt::Writers
    ));
    assert_eq!(
        observe_card_now(&scan, &binding(&world), &world.card),
        EmulatorQuiescence::CardInUse
    );
    assert!(matches!(
        apply_ps2_psu_restore_guarded(&world.plan(), &scan, &world.options()),
        Err(Ps2PsuRestoreError::CardInUse)
    ));
    drop(reader);
    assert_eq!(world.card_bytes(), world.original);
}

// ------------------------------------------- the whole publication window

#[test]
fn a_descriptor_on_the_staged_replacement_refuses_publication() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let holder = RefCell::new(None);
    let hook = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::AfterStaged {
            *holder.borrow_mut() = Some(
                fs::OpenOptions::new()
                    .write(true)
                    .open(stage_path(&world))
                    .unwrap(),
            );
        }
        Ok(())
    };
    let result = apply_with_hook(&world.plan(), &scan, &world.options(), &hook);
    drop(holder.take());
    assert!(
        matches!(result, Err(Ps2PsuRestoreError::CardInUse)),
        "{result:?}"
    );
    assert_eq!(world.card_bytes(), world.original);
}

#[test]
fn a_late_opener_of_the_restored_card_during_the_reversal_keeps_its_file() {
    // First a writer on the outgoing card forces a reversal; then, before
    // the reversal exchange, another program opens the card path (the
    // restored card). After the reversal it holds the restored card at the
    // staging path: that file must be kept and the record parked.
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let writers = RefCell::new(Vec::new());
    let marked = RefCell::new(None);
    let hook = |step: Ps2RestoreStep| {
        match step {
            Ps2RestoreStep::AfterRename => {
                writers
                    .borrow_mut()
                    .push(competing_writer(stage_path(&world)));
            }
            Ps2RestoreStep::BeforeRollbackWrite => {
                // Crash safety: the record is parked before reversing.
                *marked.borrow_mut() = Some(only_journal(&world).phase);
                writers
                    .borrow_mut()
                    .push(competing_writer(world.card.clone()));
            }
            _ => {}
        }
        Ok(())
    };
    let result = apply_with_hook(&world.plan(), &scan, &world.options(), &hook);
    assert!(
        matches!(result, Err(Ps2PsuRestoreError::RecoveryRequired(_))),
        "{result:?}"
    );
    assert_eq!(marked.take(), Some(Ps2RestorePhase::NeedsAttention));
    let files: Vec<_> = writers
        .take()
        .into_iter()
        .map(|w| w.join().unwrap())
        .collect();
    let journal = only_journal(&world);
    assert_eq!(journal.phase, Ps2RestorePhase::NeedsAttention);
    let staged = journal.staged_path.unwrap();
    assert_eq!(world.card_bytes(), world.original, "card reversed");
    assert_eq!(
        inode_of(&files[0]),
        fs::metadata(&world.card).unwrap().ino()
    );
    assert_eq!(
        inode_of(&files[1]),
        fs::metadata(&staged).unwrap().ino(),
        "the late opener's file is kept, not deleted"
    );
}

#[test]
fn undo_with_writers_on_both_sides_keeps_both_cards() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let applied = apply_ps2_psu_restore_guarded(&world.plan(), &scan, &world.options()).unwrap();
    let restored = world.card_bytes();
    let journal = load_ps2_restore_journal(&applied.journal_path).unwrap();
    let writers = RefCell::new(Vec::new());
    let hook = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::AfterUndoRename {
            let temp = undo_temp_path(world.dir.path(), &journal.operation_id);
            writers.borrow_mut().push(competing_writer(temp));
            writers
                .borrow_mut()
                .push(competing_writer(world.card.clone()));
        }
        Ok(())
    };
    let result = undo_with_hook(&applied.journal_path, &scan, 2, &hook);
    assert!(
        matches!(result, Err(Ps2PsuRestoreError::RollbackFailed { .. })),
        "{result:?}"
    );
    for writer in writers.take() {
        writer.join().unwrap();
    }
    let after = load_ps2_restore_journal(&applied.journal_path).unwrap();
    assert_eq!(after.phase, Ps2RestorePhase::UndoFailed);
    let temp = undo_temp_path(world.dir.path(), &journal.operation_id);
    assert_eq!(fs::read(&temp).unwrap(), restored, "restored card kept");
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
}

#[test]
fn restore_and_undo_succeed_with_uninspectable_processes_under_the_lease() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd"), ("994", "dockerd", "dockerd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let applied = apply_ps2_psu_restore_guarded(&world.plan(), &scan, &world.options()).unwrap();
    assert_ne!(world.card_bytes(), world.original);
    undo_ps2_psu_restore_guarded(&applied.journal_path, &scan, 2).unwrap();
    assert_eq!(world.card_bytes(), world.original);
}

#[test]
fn a_writer_opening_the_card_before_the_exchange_stops_the_restore() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let writer = RefCell::new(None);
    let hook = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::BeforeRename {
            *writer.borrow_mut() = Some(competing_writer(world.card.clone()));
        }
        Ok(())
    };
    let result = apply_with_hook(&world.plan(), &scan, &world.options(), &hook);
    assert!(
        matches!(result, Err(Ps2PsuRestoreError::CardInUse)),
        "{result:?}"
    );
    let file = writer.take().unwrap().join().unwrap();
    // The writer got the untouched original, which is still the card.
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(inode_of(&file), fs::metadata(&world.card).unwrap().ino());
    assert_eq!(only_journal(&world).phase, Ps2RestorePhase::Abandoned);
}

#[test]
fn a_writer_holding_the_outgoing_card_across_the_exchange_reverses_it() {
    // The writer resolved the card path before the exchange; its open is
    // held by the kernel and lands on the original inode, which the exchange
    // moved to the staging path. The restore must be reversed before the
    // lease is released, so the writer resumes on the card at its path.
    let world = World::new();
    let original_inode = fs::metadata(&world.card).unwrap().ino();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let writer = RefCell::new(None);
    let hook = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::AfterRename {
            let staged = stage_path(&world);
            assert_eq!(fs::metadata(&staged).unwrap().ino(), original_inode);
            *writer.borrow_mut() = Some(competing_writer(staged));
        }
        Ok(())
    };
    let result = apply_with_hook(&world.plan(), &scan, &world.options(), &hook);
    assert!(
        matches!(result, Err(Ps2PsuRestoreError::CardInUse)),
        "{result:?}"
    );
    let mut file = writer.take().unwrap().join().unwrap();
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(fs::metadata(&world.card).unwrap().ino(), original_inode);
    assert_eq!(
        inode_of(&file),
        original_inode,
        "the writer holds the live card"
    );
    assert_eq!(only_journal(&world).phase, Ps2RestorePhase::RolledBack);
    // Its writes reach the card, not a displaced copy.
    file.write_all(b"X").unwrap();
    assert_eq!(world.card_bytes()[0], b'X');
}

#[test]
fn a_program_opening_the_new_card_after_the_exchange_simply_sees_it() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let writer = RefCell::new(None);
    let hook = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::AfterRename {
            *writer.borrow_mut() = Some(competing_writer(world.card.clone()));
        }
        Ok(())
    };
    let applied = apply_with_hook(&world.plan(), &scan, &world.options(), &hook).unwrap();
    let file = writer.take().unwrap().join().unwrap();
    let live = fs::metadata(&world.card).unwrap().ino();
    assert_eq!(inode_of(&file), live, "it opened the published card");
    assert_eq!(
        sha256_hex(&world.card_bytes()),
        applied.result.post_restore_card_sha256,
        "held back until verification was complete"
    );
}

#[test]
fn writers_on_both_sides_of_the_exchange_keep_both_cards_for_review() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let writers = RefCell::new(Vec::new());
    let hook = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::AfterRename {
            let staged = stage_path(&world);
            writers.borrow_mut().push(competing_writer(staged));
            writers
                .borrow_mut()
                .push(competing_writer(world.card.clone()));
        }
        Ok(())
    };
    let result = apply_with_hook(&world.plan(), &scan, &world.options(), &hook);
    assert!(
        matches!(result, Err(Ps2PsuRestoreError::RollbackFailed { .. })),
        "{result:?}"
    );
    for writer in writers.take() {
        writer.join().unwrap();
    }
    let journal = only_journal(&world);
    assert_eq!(journal.phase, Ps2RestorePhase::RollbackFailed);
    let staged = journal.staged_path.unwrap();
    assert_eq!(fs::read(&staged).unwrap(), world.original, "original kept");
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
}

#[test]
fn undo_is_reversed_when_the_restored_card_was_opened_across_its_exchange() {
    let world = World::new();
    let proc = uninspectable(&[("993", "sshd", "sshd")]);
    let scan = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let applied = apply_ps2_psu_restore_guarded(&world.plan(), &scan, &world.options()).unwrap();
    let restored = world.card_bytes();
    let restored_inode = fs::metadata(&world.card).unwrap().ino();

    // Before the exchange: Undo stops, the card stays the restored one.
    let writer = RefCell::new(None);
    let before = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::BeforeUndoRename {
            *writer.borrow_mut() = Some(competing_writer(world.card.clone()));
        }
        Ok(())
    };
    let result = undo_with_hook(&applied.journal_path, &scan, 2, &before);
    assert!(
        matches!(result, Err(Ps2PsuRestoreError::CardInUse)),
        "{result:?}"
    );
    drop(writer.take().unwrap().join().unwrap());
    assert_eq!(world.card_bytes(), restored);
    let journal = load_ps2_restore_journal(&applied.journal_path).unwrap();
    assert_eq!(journal.phase, Ps2RestorePhase::Published);

    // Across the exchange: swapped back before release.
    let after = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::AfterUndoRename {
            let temp = undo_temp_path(world.dir.path(), &journal.operation_id);
            assert_eq!(fs::metadata(&temp).unwrap().ino(), restored_inode);
            *writer.borrow_mut() = Some(competing_writer(temp));
        }
        Ok(())
    };
    let result = undo_with_hook(&applied.journal_path, &scan, 3, &after);
    assert!(
        matches!(result, Err(Ps2PsuRestoreError::CardInUse)),
        "{result:?}"
    );
    let file = writer.take().unwrap().join().unwrap();
    assert_eq!(world.card_bytes(), restored);
    assert_eq!(fs::metadata(&world.card).unwrap().ino(), restored_inode);
    assert_eq!(
        inode_of(&file),
        restored_inode,
        "the writer holds the live card"
    );
    assert!(
        !undo_temp_path(world.dir.path(), &journal.operation_id).exists(),
        "the unused copy of the original was removed while its lease was held"
    );
    assert_eq!(
        load_ps2_restore_journal(&applied.journal_path)
            .unwrap()
            .phase,
        Ps2RestorePhase::Published,
        "Undo is still available"
    );
    drop(file);
    undo_ps2_psu_restore_guarded(&applied.journal_path, &scan, 4).unwrap();
    assert_eq!(world.card_bytes(), world.original);
}

// ------------------------------------------------- real /proc (acceptance)

fn acceptance() -> bool {
    std::env::var_os("EMUWIZ_QUIESCENCE_ACCEPTANCE").is_some()
}

/// A separate process that makes itself non-dumpable (so its descriptors and
/// executable cannot be read, like ssh-agent), renames itself, then follows
/// commands on stdin: "open <mode> <path>" prints the opened inode.
struct HiddenProcess {
    child: std::process::Child,
    lines: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl HiddenProcess {
    fn spawn(name: &str) -> Self {
        let script = r#"
import ctypes, os, sys
libc = ctypes.CDLL(None)
libc.prctl(4, 0, 0, 0, 0)                     # PR_SET_DUMPABLE = 0
libc.prctl(15, sys.argv[1].encode(), 0, 0, 0)  # PR_SET_NAME
held = []
print("ready", flush=True)
for line in sys.stdin:
    _, mode, path = line.rstrip("\n").split(" ", 2)
    try:
        f = open(path, mode)
        held.append(f)
        inode = os.fstat(f.fileno()).st_ino
    except OSError:
        inode = 0  # failed, or the opened file is gone; no inode is 0
    print(inode, flush=True)
"#;
        // Inside a test user namespace our own non-dumpable children stay
        // readable; started in an unmapped nested namespace, /proc/<pid> is
        // owned by global root again, as for other users' processes.
        let mut command = if std::env::var_os("EMUWIZ_HELPER_NESTED_USERNS").is_some() {
            let mut command = std::process::Command::new("unshare");
            command.args(["-U", "python3"]);
            command
        } else {
            std::process::Command::new("python3")
        };
        let mut child = command
            .args(["-I", "-c", script, name])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        assert_eq!(lines.next().unwrap().unwrap(), "ready");
        let pid = child.id();
        assert!(
            fs::read_dir(format!("/proc/{pid}/fd")).is_err(),
            "the helper must be uninspectable"
        );
        Self { child, lines }
    }
    fn request_open(&mut self, mode: &str, path: &Path) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "open {mode} {}", path.display()).unwrap();
        stdin.flush().unwrap();
    }
    fn opened_inode(&mut self) -> u64 {
        self.lines.next().unwrap().unwrap().parse().unwrap()
    }
}

impl Drop for HiddenProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn describe(report: &ProcScanReport) -> String {
    format!(
        "state={:?} open_for_writing={} holders={:?} emulators={:?} blocking_unreadable={} covered_by_lease={}",
        report.state,
        report.card_open_for_writing,
        report.card_holders,
        report.emulator_processes,
        report.unreadable_processes.len(),
        report.covered_by_lease.len()
    )
}

#[test]
#[ignore = "real /proc acceptance; set EMUWIZ_QUIESCENCE_ACCEPTANCE=1"]
fn real_proc_quiescence_acceptance() {
    if !acceptance() {
        return;
    }
    let observer = ProcScanQuiescence::new();
    let world = World::new();
    eprintln!(
        "strict scan: {}",
        describe(&observer.report(&binding(&world)))
    );
    let held = lease(&world.card);
    let report = observer.report_under_card_lease(&binding(&world), &held);
    drop(held);
    eprintln!("under a card lease: {}", describe(&report));
    assert_eq!(
        report.state,
        EmulatorQuiescence::Closed,
        "{}",
        describe(&report)
    );

    // Restore -> verify -> Undo on the disposable card, real observer.
    let applied =
        apply_ps2_psu_restore_guarded(&world.plan(), &observer, &world.options()).unwrap();
    assert_ne!(world.card_bytes(), world.original);
    undo_ps2_psu_restore_guarded(&applied.journal_path, &observer, 2).unwrap();
    assert_eq!(world.card_bytes(), world.original);
    eprintln!("restore -> verify -> Undo: passed");

    // A hidden process already holding the card for writing.
    let world = World::new();
    let mut writer = HiddenProcess::spawn("card-writer");
    writer.request_open("r+b", &world.card);
    writer.opened_inode();
    assert_eq!(
        observe_card_now(&observer, &binding(&world), &world.card),
        EmulatorQuiescence::CardInUse
    );
    let result = apply_ps2_psu_restore_guarded(&world.plan(), &observer, &world.options());
    eprintln!("hidden writer holding the card: {result:?}");
    assert!(matches!(result, Err(Ps2PsuRestoreError::CardInUse)));
    assert_eq!(world.card_bytes(), world.original);
    drop(writer);

    // A hidden process opening the card during the publication window.
    let world = World::new();
    let original_inode = fs::metadata(&world.card).unwrap().ino();
    let competitor = RefCell::new(HiddenProcess::spawn("innocent-name"));
    let hook = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::AfterRename {
            competitor
                .borrow_mut()
                .request_open("r+b", &stage_path(&world));
            wait_breaking(original_inode);
        }
        Ok(())
    };
    let result = apply_with_hook(&world.plan(), &observer, &world.options(), &hook);
    let opened = competitor.borrow_mut().opened_inode();
    eprintln!("hidden writer opening during publication: {result:?}");
    assert!(matches!(result, Err(Ps2PsuRestoreError::CardInUse)));
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(opened, fs::metadata(&world.card).unwrap().ino());
    assert_eq!(only_journal(&world).phase, Ps2RestorePhase::RolledBack);
    drop(competitor);

    // A hidden process holding the card read-only.
    let world = World::new();
    let mut reader = HiddenProcess::spawn("card-reader");
    reader.request_open("rb", &world.card);
    reader.opened_inode();
    let result = apply_ps2_psu_restore_guarded(&world.plan(), &observer, &world.options());
    eprintln!("hidden read-only holder: {result:?}");
    assert!(matches!(result, Err(Ps2PsuRestoreError::CardInUse)));
    assert_eq!(world.card_bytes(), world.original);
    drop(reader);

    // A hidden process named like PCSX2, not touching the card.
    let world = World::new();
    let emulator = HiddenProcess::spawn("pcsx2-qt");
    let result = apply_ps2_psu_restore_guarded(&world.plan(), &observer, &world.options());
    eprintln!("hidden pcsx2-qt: {result:?}");
    assert!(matches!(result, Err(Ps2PsuRestoreError::EmulatorRunning)));
    assert_eq!(world.card_bytes(), world.original);
    drop(emulator);
    eprintln!("all checks passed");
}

/// Run while a real PCSX2 (for example an isolated Flatpak instance) is open.
#[test]
#[ignore = "needs a running PCSX2; set EMUWIZ_EXPECT_PCSX2_RUNNING=1"]
fn real_proc_refuses_while_pcsx2_runs() {
    if std::env::var_os("EMUWIZ_EXPECT_PCSX2_RUNNING").is_none() {
        return;
    }
    let world = World::new();
    let observer = ProcScanQuiescence::new();
    let held = lease(&world.card);
    let report = observer.report_under_card_lease(&binding(&world), &held);
    drop(held);
    eprintln!("with PCSX2 running: {}", describe(&report));
    for pid in &report.emulator_processes {
        let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        let fd = fs::read_dir(format!("/proc/{pid}/fd")).is_ok();
        eprintln!(
            "  emulator pid {pid}: comm={} descriptors_readable={fd}",
            comm.trim()
        );
    }
    assert_eq!(report.state, EmulatorQuiescence::Running);
    assert!(matches!(
        apply_ps2_psu_restore_guarded(&world.plan(), &observer, &world.options()),
        Err(Ps2PsuRestoreError::EmulatorRunning)
    ));
    assert_eq!(world.card_bytes(), world.original);
}

/// Storage acceptance on a real filesystem. Cards are created under TMPDIR
/// (the path the restore uses). With `EMUWIZ_STORAGE_ALIAS`, competing
/// programs open the same directory through that other path instead (a bind
/// mount, the underlying ext4 path, or a mergerfs view of it). The expected
/// outcome comes from `EMUWIZ_STORAGE_EXPECT` (`protected` / `unprotected`),
/// which the test checks against the filesystem of the card itself.
#[test]
#[ignore = "real storage acceptance; set EMUWIZ_STORAGE_EXPECT and TMPDIR"]
fn real_storage_acceptance() {
    let Ok(expect) = std::env::var("EMUWIZ_STORAGE_EXPECT") else {
        return;
    };
    let observer = ProcScanQuiescence::new();
    let alias_root = std::env::var_os("EMUWIZ_STORAGE_ALIAS").map(PathBuf::from);
    let alias = |world: &World, path: &Path| -> PathBuf {
        match &alias_root {
            Some(root) => root
                .join(world.dir.path().file_name().unwrap())
                .join(path.file_name().unwrap()),
            None => path.to_path_buf(),
        }
    };
    let world = World::new();
    let seen = alias(&world, &world.card);
    let mut statfs: libc::statfs = unsafe { std::mem::zeroed() };
    let c = std::ffi::CString::new(world.card.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::statfs(c.as_ptr(), &mut statfs) }, 0);
    eprintln!(
        "card {} (filesystem magic {:#x}); competitors use {} (same inode: {})",
        world.card.display(),
        statfs.f_type,
        seen.display(),
        fs::metadata(&seen).unwrap().ino() == fs::metadata(&world.card).unwrap().ino()
    );
    assert_eq!(
        fs::read(&seen).unwrap(),
        world.original,
        "alias shows the card"
    );

    if expect == "unprotected" {
        assert!(card_storage_unprotectable(&world.card));
        let result = apply_ps2_psu_restore_guarded(&world.plan(), &observer, &world.options());
        eprintln!("restore on unprotectable storage: {result:?}");
        assert!(matches!(
            result,
            Err(Ps2PsuRestoreError::StorageUnprotected)
        ));
        assert_eq!(world.card_bytes(), world.original);
        eprintln!("storage checks passed");
        return;
    }
    assert_eq!(expect, "protected");
    assert!(!card_storage_unprotectable(&world.card));
    let applied =
        apply_ps2_psu_restore_guarded(&world.plan(), &observer, &world.options()).unwrap();
    assert_ne!(world.card_bytes(), world.original);
    undo_ps2_psu_restore_guarded(&applied.journal_path, &observer, 2).unwrap();
    assert_eq!(world.card_bytes(), world.original);
    eprintln!("restore -> verify -> Undo: passed");

    let world = World::new();
    let mut writer = HiddenProcess::spawn("card-writer");
    writer.request_open("r+b", &alias(&world, &world.card));
    writer.opened_inode();
    let result = apply_ps2_psu_restore_guarded(&world.plan(), &observer, &world.options());
    eprintln!("writer holding the card through the alias: {result:?}");
    assert!(matches!(result, Err(Ps2PsuRestoreError::CardInUse)));
    assert_eq!(world.card_bytes(), world.original);
    drop(writer);

    let world = World::new();
    let original_inode = fs::metadata(&world.card).unwrap().ino();
    let competitor = RefCell::new(HiddenProcess::spawn("innocent-name"));
    let hook = |step: Ps2RestoreStep| {
        if step == Ps2RestoreStep::AfterRename {
            competitor
                .borrow_mut()
                .request_open("r+b", &alias(&world, &stage_path(&world)));
            wait_breaking(original_inode);
        }
        Ok(())
    };
    let result = apply_with_hook(&world.plan(), &observer, &world.options(), &hook);
    let opened = competitor.borrow_mut().opened_inode();
    eprintln!(
        "writer opening through the alias during publication: {result:?}; its open {}",
        if opened == 0 {
            "failed".to_string()
        } else {
            format!("got inode {opened}")
        }
    );
    assert!(matches!(result, Err(Ps2PsuRestoreError::CardInUse)));
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(only_journal(&world).phase, Ps2RestorePhase::RolledBack);
    drop(competitor);

    let world = World::new();
    let mut reader = HiddenProcess::spawn("card-reader");
    reader.request_open("rb", &alias(&world, &world.card));
    reader.opened_inode();
    let result = apply_ps2_psu_restore_guarded(&world.plan(), &observer, &world.options());
    eprintln!("read-only holder through the alias: {result:?}");
    assert!(matches!(result, Err(Ps2PsuRestoreError::CardInUse)));
    assert_eq!(world.card_bytes(), world.original);
    drop(reader);
    eprintln!("storage checks passed");
}

/// Read-only: classify existing files (paths in `EMUWIZ_STORAGE_PROBE`,
/// colon-separated) the way a restore would. Opens read-only, writes nothing.
#[test]
#[ignore = "read-only storage probe; set EMUWIZ_STORAGE_PROBE"]
fn real_storage_probe_read_only() {
    let Some(paths) = std::env::var_os("EMUWIZ_STORAGE_PROBE") else {
        return;
    };
    for path in std::env::split_paths(&paths) {
        eprintln!(
            "{}: unprotectable={} lease={}",
            path.display(),
            card_storage_unprotectable(&path),
            match CardWriteLease::acquire(&path, None) {
                CardLeaseAttempt::Held(_) => "held",
                CardLeaseAttempt::Writers => "writers",
                CardLeaseAttempt::Changed => "changed",
                CardLeaseAttempt::Unavailable => "unavailable",
            }
        );
    }
}

#[test]
fn a_descriptor_held_only_for_an_instant_does_not_make_the_card_look_in_use() {
    // Like a forked child holding copies of its parent's descriptors until
    // it executes: gone within a few milliseconds.
    let world = World::new();
    let brief = fs::File::open(&world.card).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        drop(brief);
    });
    assert!(matches!(
        CardWriteLease::acquire(&world.card, None),
        CardLeaseAttempt::Held(_)
    ));
    release.join().unwrap();
    // One that stays open is still reported.
    let _kept = fs::File::open(&world.card).unwrap();
    assert!(matches!(
        CardWriteLease::acquire(&world.card, None),
        CardLeaseAttempt::Writers
    ));
}

#[test]
fn emuwiz_waits_for_its_own_brief_lease_on_the_card_instead_of_refusing() {
    // A preview check on another thread holds a lease for one /proc scan,
    // longer than the fork retry window.
    let world = World::new();
    let (taken, wait) = std::sync::mpsc::channel();
    let card = world.card.clone();
    let preview = std::thread::spawn(move || {
        let held = lease(&card);
        taken.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        drop(held);
    });
    wait.recv().unwrap();
    assert!(matches!(
        CardWriteLease::acquire(&world.card, None),
        CardLeaseAttempt::Held(_)
    ));
    preview.join().unwrap();
    // The same thread's own lease is not waited for: the probe is refused.
    let _mine = lease(&world.card);
    let started = std::time::Instant::now();
    assert!(matches!(
        CardWriteLease::acquire(&world.card, None),
        CardLeaseAttempt::Writers
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}
