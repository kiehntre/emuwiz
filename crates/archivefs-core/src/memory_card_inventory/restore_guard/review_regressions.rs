//! Independent-review reproductions. All images and saves are synthetic.
use super::*;

fn replace_card(world: &World) -> (Vec<u8>, FileIdentity) {
    let mut replacement = world.card_bytes();
    replacement[2000] ^= 0x55;
    let path = world.dir.path().join("replacement.tmp");
    fs::write(&path, &replacement).unwrap();
    fs::rename(path, &world.card).unwrap();
    (replacement, path_identity(&world.card).unwrap())
}

#[test]
fn unsupported_exchange_apply_preserves_replacement() {
    let world = World::new();
    let replacement = RefCell::new(None);
    let result = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::BeforeRename {
            *replacement.borrow_mut() = Some(replace_card(&world));
            EXCHANGE_ERRORS.with(|errors| errors.borrow_mut().push_back(libc::ENOSYS));
        }
        Ok(())
    });
    assert_eq!(
        result.unwrap_err(),
        Ps2PsuRestoreError::UnsupportedAtomicExchange
    );
    let replacement = replacement.borrow();
    let (bytes, identity) = replacement.as_ref().unwrap();
    assert_eq!(world.card_bytes(), *bytes);
    assert_eq!(path_identity(&world.card).unwrap(), *identity);
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
}

#[test]
fn unsupported_exchange_undo_preserves_replacement() {
    let world = World::new();
    let applied = world.apply().unwrap();
    let replacement = RefCell::new(None);
    let result = undo_with_hook(&applied.journal_path, &CLOSED, 2, &|step| {
        if step == Ps2RestoreStep::BeforeUndoRename {
            *replacement.borrow_mut() = Some(replace_card(&world));
            EXCHANGE_ERRORS.with(|errors| errors.borrow_mut().push_back(libc::EINVAL));
        }
        Ok(())
    });
    assert_eq!(
        result.unwrap_err(),
        Ps2PsuRestoreError::UnsupportedAtomicExchange
    );
    let replacement = replacement.borrow();
    let (bytes, identity) = replacement.as_ref().unwrap();
    assert_eq!(world.card_bytes(), *bytes);
    assert_eq!(path_identity(&world.card).unwrap(), *identity);
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
}

#[test]
fn real_proc_excludes_our_synthetic_card_descriptor() {
    let world = World::new();
    let _pinned = fs::File::open(&world.card).unwrap();
    let report = ProcScanQuiescence::new().report(&ps2_card_binding(&world.card, "synthetic"));
    eprintln!(
        "Real /proc observation: {:?}; holders={}, emulators={}, unreadable_live={}, listing_unavailable={}",
        report.state,
        report.card_holders.len(),
        report.emulator_processes.len(),
        report.unreadable_processes.len(),
        report.listing_unavailable
    );
    assert!(!report.card_holders.contains(&std::process::id()));
    assert!(!report.unreadable_processes.contains(&std::process::id()));
}

#[test]
fn self_only_real_proc_allows_guarded_publication_and_undo() {
    let world = World::new();
    let proc = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(
        format!("/proc/{}", std::process::id()),
        proc.path().join(std::process::id().to_string()),
    )
    .unwrap();
    let result = apply_ps2_psu_restore_guarded(
        &world.plan(),
        &ProcScanQuiescence::with_root(proc.path().to_path_buf()),
        &world.options(),
    );
    let result = result.unwrap();
    undo_ps2_psu_restore_guarded(
        &result.journal_path,
        &ProcScanQuiescence::with_root(proc.path().to_path_buf()),
        2,
    )
    .unwrap();
    assert_eq!(world.card_bytes(), world.original);
}

// This oracle reads raw fixture FAT words, independently of EmuWiz's lenient
// chain reader. PS2dev/mymc and PCSX2's filesystem reference identify bit 31
// as allocated, FFFFFFFF as allocated chain-end and 7FFFFFFF as free.
fn raw_fat(bytes: &[u8], relative: usize) -> u32 {
    let offset = 8 * CLUSTER + relative * 4;
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn restored_tail_is_allocated_and_released_cluster_is_free() {
    let world = World::new();
    world.apply().unwrap();
    let bytes = world.card_bytes();
    assert_eq!(raw_fat(&bytes, 3), 0xffff_ffff);
    assert_eq!(raw_fat(&bytes, 4), 0x7fff_ffff);
}

#[test]
fn every_unsupported_exchange_errno_refuses_apply_and_undo() {
    for errno in [libc::ENOSYS, libc::EINVAL, libc::ENOTSUP] {
        let world = World::new();
        let identity = path_identity(&world.card).unwrap();
        EXCHANGE_ERRORS.with(|errors| errors.borrow_mut().push_back(errno));
        assert_eq!(
            world.apply().unwrap_err(),
            Ps2PsuRestoreError::UnsupportedAtomicExchange
        );
        assert_eq!(path_identity(&world.card).unwrap(), identity);
        assert_eq!(world.card_bytes(), world.original);
        let world = World::new();
        let applied = world.apply().unwrap();
        let identity = path_identity(&world.card).unwrap();
        let bytes = world.card_bytes();
        EXCHANGE_ERRORS.with(|errors| errors.borrow_mut().push_back(errno));
        assert_eq!(
            undo_ps2_psu_restore_guarded(&applied.journal_path, &CLOSED, 2).unwrap_err(),
            Ps2PsuRestoreError::UnsupportedAtomicExchange
        );
        assert_eq!(path_identity(&world.card).unwrap(), identity);
        assert_eq!(world.card_bytes(), bytes);
    }
}

#[test]
fn failed_swap_back_retains_foreign_inode_and_names_its_path() {
    let world = World::new();
    let foreign = RefCell::new(None);
    let result = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::BeforeRename {
            *foreign.borrow_mut() = Some(replace_card(&world));
            EXCHANGE_ERRORS.with(|errors| errors.borrow_mut().extend([0, libc::EIO]));
        }
        Ok(())
    });
    let error = result.unwrap_err();
    assert!(matches!(error, Ps2PsuRestoreError::RollbackFailed { .. }));
    let journal = load_ps2_restore_journal(&world.journal_files()[0]).unwrap();
    let retained = journal.staged_path.unwrap();
    let foreign = foreign.borrow();
    let (bytes, identity) = foreign.as_ref().unwrap();
    assert_eq!(fs::read(&retained).unwrap(), *bytes);
    assert_eq!(path_identity(&retained).unwrap(), *identity);
    assert!(error.to_string().contains(&retained.display().to_string()));
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
}

#[test]
fn rollback_refuses_later_foreign_card() {
    let world = World::new();
    let foreign = RefCell::new(None);
    let result = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::AfterRename {
            *foreign.borrow_mut() = Some(replace_card(&world));
            return Err(std::io::Error::other(
                "publication interrupted by external change",
            ));
        }
        Ok(())
    });
    assert!(matches!(
        result,
        Err(Ps2PsuRestoreError::RollbackFailed { .. })
    ));
    let foreign = foreign.borrow();
    let (bytes, identity) = foreign.as_ref().unwrap();
    assert_eq!(world.card_bytes(), *bytes);
    assert_eq!(path_identity(&world.card).unwrap(), *identity);
    let journal = load_ps2_restore_journal(&world.journal_files()[0]).unwrap();
    assert_eq!(
        fs::read(journal.staged_path.unwrap()).unwrap(),
        world.original
    );
}

#[test]
fn partial_backup_and_stage_are_journaled_before_creation_and_retained() {
    for at in [
        Ps2RestoreStep::BeforeBackupCreate,
        Ps2RestoreStep::BeforeStageCreate,
    ] {
        let world = World::new();
        let artifact = RefCell::new(None);
        let _ = world.apply_hooked(&CLOSED, &|step| {
            if step == at {
                let journal = load_ps2_restore_journal(&world.journal_files()[0]).unwrap();
                let path = if at == Ps2RestoreStep::BeforeBackupCreate {
                    journal.backup_path
                } else {
                    journal.staged_path.unwrap()
                };
                fs::write(&path, b"synthetic partial artifact").unwrap();
                *artifact.borrow_mut() = Some(path);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    SIMULATED_CRASH,
                ));
            }
            Ok(())
        });
        assert!(matches!(
            recover_ps2_psu_restore(&world.journal_files()[0], 2).unwrap(),
            Ps2RecoveryOutcome::NeedsAttention(_)
        ));
        assert_eq!(
            fs::read(artifact.borrow().as_ref().unwrap()).unwrap(),
            b"synthetic partial artifact"
        );
        assert_eq!(world.card_bytes(), world.original);
    }
}

#[test]
fn allocated_unreachable_cluster_is_never_reused() {
    let mut bytes = ps2_inventory_fixture();
    let relative = 1usize; // not reachable from any fixture directory
    let offset = 8 * CLUSTER + relative * 4;
    bytes[offset..offset + 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    bytes[(41 + relative) * CLUSTER..(42 + relative) * CLUSTER].fill(0xa6);
    let world = World::with_card(bytes);
    world.apply().unwrap();
    assert_eq!(raw_fat(&world.card_bytes(), relative), 0xffff_ffff);
    assert_eq!(
        &world.card_bytes()[(41 + relative) * CLUSTER..(42 + relative) * CLUSTER],
        &[0xa6; CLUSTER]
    );
}

#[test]
fn fat_address_indirection_selects_a_second_table() {
    let bytes = ps2_inventory_fixture();
    let world = World::with_card(bytes);
    let geometry = world.plan().geometry;
    assert_eq!(
        ps2_fat_location(&world.original, &geometry, 0).unwrap(),
        (8, 0)
    );
    assert_eq!(
        ps2_fat_location(&world.original, &geometry, 256).unwrap(),
        (9, 0)
    );
    let mut bytes = world.original.clone();
    restore_set_fat(&mut bytes, &geometry, 256, 0xffff_ffff).unwrap();
    assert_eq!(
        &bytes[9 * CLUSTER..9 * CLUSTER + 4],
        &0xffff_ffffu32.to_le_bytes()
    );
    assert_eq!(
        &bytes[7 * CLUSTER..8 * CLUSTER],
        &world.original[7 * CLUSTER..8 * CLUSTER]
    );
}

#[test]
fn free_tail_marker_is_rejected_as_an_allocated_chain() {
    let world = World::new();
    let mut bytes = world.original.clone();
    restore_set_fat(&mut bytes, &world.plan().geometry, 3, 0x7fff_ffff).unwrap();
    assert!(ps2_fat_next(&bytes, &world.plan().geometry, 3).is_err());
}

#[test]
#[allow(deprecated)]
fn public_legacy_restore_and_undo_refuse_without_io() {
    let world = World::new();
    assert!(matches!(
        apply_ps2_psu_restore(&world.plan()),
        Err(Ps2PsuRestoreError::RecoveryRequired(_))
    ));
    assert_eq!(world.card_bytes(), world.original);
    assert!(!world.backup.exists());
    assert!(world.journal_files().is_empty());
    let applied = world.apply().unwrap();
    let before = world.card_bytes();
    assert!(matches!(
        undo_ps2_psu_restore(&applied.result),
        Err(Ps2PsuRestoreError::RecoveryRequired(_))
    ));
    assert_eq!(world.card_bytes(), before);
}

#[test]
fn concurrent_cooperating_restore_is_refused_before_mutation() {
    let world = World::new();
    let lock = ownership::lock(&world.card).unwrap();
    assert!(matches!(
        world.apply(),
        Err(Ps2PsuRestoreError::RecoveryRequired(_))
    ));
    assert_eq!(world.card_bytes(), world.original);
    assert!(!world.backup.exists());
    drop(lock);
    world.apply().unwrap();
}

#[test]
fn hardlinked_card_is_refused_before_backup() {
    let world = World::new();
    fs::hard_link(&world.card, world.dir.path().join("alias.ps2")).unwrap();
    assert!(matches!(
        world.apply(),
        Err(Ps2PsuRestoreError::InvalidPlan(_))
    ));
    assert!(!world.backup.exists());
    assert_eq!(world.card_bytes(), world.original);
}

#[test]
fn real_proc_child_holder() {
    let Some(path) = std::env::var_os("EMUWIZ_SYNTHETIC_PROC_CHILD_CARD") else {
        return;
    };
    let _card = fs::File::open(path).unwrap();
    println!("synthetic-holder-ready");
    std::io::stdout().flush().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(30));
}

#[test]
fn real_proc_detects_child_holding_a_hardlink_alias() {
    use std::io::BufRead;
    let world = World::new();
    let alias = world.dir.path().join("synthetic-hardlink.ps2");
    fs::hard_link(&world.card, &alias).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "memory_card_inventory::restore_guard::tests::review_regressions::real_proc_child_holder", "--nocapture"])
        .env("EMUWIZ_SYNTHETIC_PROC_CHILD_CARD", &alias)
        .stdout(std::process::Stdio::piped()).spawn().unwrap();
    let ready = std::io::BufReader::new(child.stdout.take().unwrap())
        .lines()
        .any(|line| line.unwrap().contains("synthetic-holder-ready"));
    let report = ProcScanQuiescence::new().report(&ps2_card_binding(&world.card, "synthetic"));
    let found = report.card_holders.contains(&child.id());
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        ready && found,
        "real /proc must identify the synthetic alias holder by inode"
    );
    assert_eq!(report.state, EmulatorQuiescence::Running);
}

#[test]
fn proc_aliases_match_by_inode_and_notes_do_not_match_emulator_name() {
    let world = World::new();
    let alias = world.dir.path().join("alias.ps2");
    fs::hard_link(&world.card, &alias).unwrap();
    let proc = fake_proc(&[("991", "reader", "reader", Some(&alias))]);
    assert_eq!(
        ProcScanQuiescence::with_root(proc.path().to_path_buf())
            .observe(&ps2_card_binding(&world.card, "synthetic")),
        EmulatorQuiescence::Running
    );
    let proc = fake_proc(&[("992", "vim", "vim\0notes-about-pcsx2.txt\0", None)]);
    assert_eq!(
        ProcScanQuiescence::with_root(proc.path().to_path_buf())
            .observe(&ps2_card_binding(&world.card, "synthetic")),
        EmulatorQuiescence::Closed
    );
}

#[test]
fn unreadable_live_process_is_explicit_but_zombies_do_not_poison_scan() {
    let world = World::new();
    let proc = fake_proc(&[("993", "bash", "bash", None)]);
    fs::remove_dir(proc.path().join("993/fd")).unwrap();
    let provider = ProcScanQuiescence::with_root(proc.path().to_path_buf());
    let binding = ps2_card_binding(&world.card, "synthetic");
    assert_eq!(provider.report(&binding).unreadable_processes, [993]);
    assert_eq!(provider.observe(&binding), EmulatorQuiescence::Unknown);
    fs::write(proc.path().join("993/status"), "State:\tZ (zombie)\n").unwrap();
    assert_eq!(provider.observe(&binding), EmulatorQuiescence::Closed);
}

#[test]
fn corrupt_journal_is_reported_by_batch_recovery_without_card_io() {
    let world = World::new();
    let path = world.journals.join("ps2-psu-restore-corrupt.json");
    fs::write(&path, b"corrupt synthetic journal").unwrap();
    let outcomes = recover_all_interrupted_ps2_restores(&world.journals, 2);
    assert_eq!(outcomes.len(), 1);
    assert!(matches!(
        outcomes[0].1,
        Err(Ps2PsuRestoreError::JournalCorrupt(_))
    ));
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(fs::read(path).unwrap(), b"corrupt synthetic journal");
}

#[test]
fn successful_reversal_preserves_foreign_inode_on_apply_and_undo() {
    for undo in [false, true] {
        let world = World::new();
        let applied = undo.then(|| world.apply().unwrap());
        let foreign = RefCell::new(None);
        let hook = |step| {
            if step
                == if undo {
                    Ps2RestoreStep::BeforeUndoRename
                } else {
                    Ps2RestoreStep::BeforeRename
                }
            {
                *foreign.borrow_mut() = Some(replace_card(&world));
            }
            Ok(())
        };
        let result = if let Some(applied) = applied {
            undo_with_hook(&applied.journal_path, &CLOSED, 2, &hook)
        } else {
            world.apply_hooked(&CLOSED, &hook).map(|_| ())
        };
        assert_eq!(
            result.unwrap_err(),
            if undo {
                Ps2PsuRestoreError::StaleUndo
            } else {
                Ps2PsuRestoreError::CardChanged
            }
        );
        let foreign = foreign.borrow();
        let (bytes, identity) = foreign.as_ref().unwrap();
        assert_eq!(world.card_bytes(), *bytes);
        assert_eq!(path_identity(&world.card).unwrap(), *identity);
    }
}

#[test]
fn failed_undo_swap_back_preserves_displaced_foreign_inode() {
    let world = World::new();
    let applied = world.apply().unwrap();
    let foreign = RefCell::new(None);
    let result = undo_with_hook(&applied.journal_path, &CLOSED, 2, &|step| {
        if step == Ps2RestoreStep::BeforeUndoRename {
            *foreign.borrow_mut() = Some(replace_card(&world));
            EXCHANGE_ERRORS.with(|errors| errors.borrow_mut().extend([0, libc::EIO]));
        }
        Ok(())
    });
    let error = result.unwrap_err();
    assert!(matches!(error, Ps2PsuRestoreError::RollbackFailed { .. }));
    let journal = load_ps2_restore_journal(&applied.journal_path).unwrap();
    assert_eq!(journal.phase, Ps2RestorePhase::UndoFailed);
    let retained = undo_temp_path(world.dir.path(), &journal.operation_id);
    let foreign = foreign.borrow();
    let (bytes, identity) = foreign.as_ref().unwrap();
    assert_eq!(fs::read(&retained).unwrap(), *bytes);
    assert_eq!(path_identity(&retained).unwrap(), *identity);
    assert!(error.to_string().contains(&retained.display().to_string()));
}

#[test]
fn journal_persistence_failure_after_publication_is_explicit() {
    let world = World::new();
    let result = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::AfterRename {
            let journal = world.journal_files()[0].clone();
            fs::rename(&journal, world.dir.path().join("retained-journal.json")).unwrap();
            fs::create_dir(&journal).unwrap(); // deterministic rename failure, no permission assumptions
        }
        Ok(())
    });
    assert!(matches!(
        result,
        Err(Ps2PsuRestoreError::RecoveryRequired(_))
    ));
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("journal update failed")
    );
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
}

#[test]
fn restart_never_deletes_a_late_foreign_displaced_card() {
    let world = World::new();
    let foreign = RefCell::new(None);
    let _ = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::BeforeRename {
            *foreign.borrow_mut() = Some(replace_card(&world));
        }
        crash(step, Ps2RestoreStep::AfterRename)
    });
    let journal = world.only_journal();
    let retained = journal.staged_path.unwrap();
    assert!(matches!(
        recover_ps2_psu_restore(&world.journal_files()[0], 2).unwrap(),
        Ps2RecoveryOutcome::NeedsAttention(_)
    ));
    let foreign = foreign.borrow();
    let (bytes, identity) = foreign.as_ref().unwrap();
    assert_eq!(fs::read(&retained).unwrap(), *bytes);
    assert_eq!(path_identity(&retained).unwrap(), *identity);
}

#[test]
fn restart_flags_complete_stage_created_before_identity_was_persisted() {
    let world = World::new();
    let image = build_restored_card(&world.plan(), &world.original).unwrap();
    let _ = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::BeforeStageCreate {
            let journal = world.only_journal();
            assert!(journal.post_identity.is_none());
            fs::write(journal.staged_path.unwrap(), &image).unwrap();
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                SIMULATED_CRASH,
            ));
        }
        Ok(())
    });
    assert!(matches!(
        recover_ps2_psu_restore(&world.journal_files()[0], 2).unwrap(),
        Ps2RecoveryOutcome::NeedsAttention(_)
    ));
    let retained = world.only_journal().staged_path.unwrap();
    assert_eq!(fs::read(retained).unwrap(), image);
    assert_eq!(world.card_bytes(), world.original);
}

#[test]
fn cleanup_never_unlinks_card_even_when_its_name_matches_stage_prefix() {
    let mut world = World::new();
    let named = world.dir.path().join(".emuwiz-ps2-stage-synthetic.tmp");
    fs::rename(&world.card, &named).unwrap();
    world.card = named;
    let applied = world.apply().unwrap();
    let mut journal = load_ps2_restore_journal(&applied.journal_path).unwrap();
    let before = world.card_bytes();
    journal.staged_path = Some(world.card.clone());
    remove_owned_temp(&journal);
    assert_eq!(world.card_bytes(), before);

    // The deterministic Undo temp can also alias a specially named card.
    let undo_named = undo_temp_path(world.dir.path(), &journal.operation_id);
    fs::rename(&world.card, &undo_named).unwrap();
    journal.card_path = undo_named.clone();
    clean_undo_temp(&journal);
    assert_eq!(fs::read(&undo_named).unwrap(), before);
    persist_journal(&applied.journal_path, &journal).unwrap();
    assert!(matches!(
        undo_ps2_psu_restore_guarded(&applied.journal_path, &CLOSED, 2),
        Err(Ps2PsuRestoreError::InvalidPlan(_))
    ));
    assert_eq!(fs::read(&undo_named).unwrap(), before);
}
