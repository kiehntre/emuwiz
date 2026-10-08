// Included inside the existing synthetic updater test module.

#[test]
fn safety_process_worker() {
    let Some(payload) = std::env::var_os("EMUWIZ_UPDATE_TEST_PAYLOAD") else {
        return;
    };
    let (plan, installation, update): (UpdateExecutionPlan, EmulatorInstallation, UpdateResult) =
        serde_json::from_slice(&fs::read(payload).unwrap()).unwrap();
    if std::env::var_os("EMUWIZ_UPDATE_TEST_LOCK_ONLY").is_some() {
        let _guard = acquire_lock(
            plan.rollback_path.parent().unwrap(),
            &plan.target_path,
            &plan.transaction_id,
        )
        .unwrap();
        println!("UPDATE_WORKER_READY");
        std::io::stdout().flush().unwrap();
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
        return;
    }
    struct Wait;
    impl UpdateDownloader for Wait {
        fn download(&mut self, _: &str, out: &mut File) -> Result<(), UpdateExecutionError> {
            out.write_all(NEW).map_err(io_err)?;
            println!("UPDATE_WORKER_READY");
            std::io::stdout().flush().unwrap();
            let mut byte = [0];
            std::io::stdin().read_exact(&mut byte).map_err(io_err)?;
            Ok(())
        }
    }
    execute_staged_update(&plan, &installation, &update, stopped, &mut Wait).unwrap();
}

fn safety_child(f: &Fixture, lock_only: bool) -> std::process::Child {
    use std::io::BufRead;
    let payload = f._dir.path().join("worker.json");
    fs::write(
        &payload,
        serde_json::to_vec(&(&f.plan, &f.installation, &f.update)).unwrap(),
    )
    .unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "emulator_update::tests::safety_process_worker",
            "--nocapture",
        ])
        .env("EMUWIZ_UPDATE_TEST_PAYLOAD", payload)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if lock_only {
        command.env("EMUWIZ_UPDATE_TEST_LOCK_ONLY", "1");
    }
    let mut child = command.spawn().unwrap();
    let mut lines = std::io::BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(
            lines.read_line(&mut line).unwrap() > 0,
            "worker exited before barrier"
        );
        if line.contains("UPDATE_WORKER_READY") {
            break;
        }
    }
    child
}

#[test]
fn safety_kernel_lock_releases_after_process_death_without_a_journal() {
    let f = fixture();
    let mut child = safety_child(&f, true);
    assert!(discover_update_records(&f.installation.installation_root).is_empty());
    assert!(matches!(
        run(&f, stopped),
        Err(UpdateExecutionError::Concurrent(_))
    ));
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(run(&f, stopped).is_ok());
}

#[test]
fn safety_competing_process_recovery_refuses_live_apply_then_recovers_after_death() {
    let f = fixture();
    let mut child = safety_child(&f, false);
    let record = discover_update_records(&f.installation.installation_root).remove(0);
    let before = fs::read(&record.path).unwrap();
    assert!(matches!(
        recover_update(&record.path, stopped),
        Err(UpdateExecutionError::Concurrent(_))
    ));
    assert_eq!(fs::read(&record.path).unwrap(), before);
    assert_eq!(target(&f), OLD);
    child.kill().unwrap();
    child.wait().unwrap();
    let recovered = recover_update(&record.path, stopped).unwrap();
    assert_eq!(recovered.state, UpdateTransactionState::Failed);
    assert_eq!(target(&f), OLD);
    assert!(recovered.staged_path.as_ref().unwrap().is_file());
    assert_eq!(recover_update(&record.path, stopped).unwrap(), recovered);
}

#[test]
fn safety_crashes_at_each_executable_mutation_boundary_are_idempotently_reconciled() {
    for boundary in [
        MutationBoundary::OriginalMoved,
        MutationBoundary::PublishedMoved,
    ] {
        let f = fixture();
        with_fault(Fault::CrashBoundary(boundary), || {
            run(&f, stopped).unwrap_err()
        });
        let record = discover_update_records(&f.installation.installation_root).remove(0);
        let j = recover_update(&record.path, stopped).unwrap();
        assert_eq!(
            target(&f),
            if boundary == MutationBoundary::OriginalMoved {
                OLD
            } else {
                NEW
            }
        );
        assert_eq!(recover_update(&record.path, stopped).unwrap(), j);
    }
    for boundary in [
        MutationBoundary::UndoDisplaced,
        MutationBoundary::UndoRestored,
    ] {
        let f = fixture();
        let j = run(&f, stopped).unwrap();
        with_fault(Fault::CrashBoundary(boundary), || {
            rollback_staged_update(&j, stopped).unwrap_err()
        });
        let recovered = recover_update(&j.record_path().unwrap(), stopped).unwrap();
        assert_eq!(
            target(&f),
            if boundary == MutationBoundary::UndoDisplaced {
                NEW
            } else {
                OLD
            }
        );
        assert_eq!(
            recover_update(&j.record_path().unwrap(), stopped).unwrap(),
            recovered
        );
    }
}

#[test]
fn safety_recovery_crash_after_restore_retries_from_truthful_disk_evidence() {
    let f = fixture();
    with_fault(
        Fault::CrashBoundary(MutationBoundary::OriginalMoved),
        || run(&f, stopped).unwrap_err(),
    );
    let path = discover_update_records(&f.installation.installation_root)
        .remove(0)
        .path;
    with_fault(
        Fault::CrashBoundary(MutationBoundary::RecoveryMoved),
        || recover_update(&path, stopped).unwrap_err(),
    );
    assert_eq!(target(&f), OLD);
    assert_eq!(
        load_journal(&path).unwrap().state,
        UpdateTransactionState::Applying
    );
    assert_eq!(
        recover_update(&path, stopped).unwrap().state,
        UpdateTransactionState::Failed
    );
}

#[test]
fn safety_recovery_failed_journal_save_reports_failure_and_preserves_evidence() {
    let f = fixture();
    with_fault(
        Fault::CrashBoundary(MutationBoundary::OriginalMoved),
        || run(&f, stopped).unwrap_err(),
    );
    let path = discover_update_records(&f.installation.installation_root)
        .remove(0)
        .path;
    let before = fs::read(&path).unwrap();
    let e = with_fault(Fault::FailPersist(UpdateTransactionState::Failed), || {
        recover_update(&path, stopped).unwrap_err()
    });
    assert!(matches!(e, UpdateExecutionError::NeedsReconciliation(_)));
    assert_eq!(target(&f), OLD);
    assert_eq!(fs::read(&path).unwrap(), before);
    let recovered = recover_update(&path, stopped).unwrap();
    assert!(recovered.staged_path.as_ref().unwrap().is_file());
}

#[test]
fn safety_new_target_or_changed_backup_during_recovery_is_preserved() {
    for change_backup in [false, true] {
        let f = fixture();
        with_fault(
            Fault::CrashBoundary(MutationBoundary::OriginalMoved),
            || run(&f, stopped).unwrap_err(),
        );
        let path = discover_update_records(&f.installation.installation_root)
            .remove(0)
            .path;
        let before = fs::read(&path).unwrap();
        let mut calls = 0;
        let result = recover_update(&path, || {
            calls += 1;
            if calls == 2 {
                fs::write(
                    if change_backup {
                        &f.plan.rollback_path
                    } else {
                        &f.plan.target_path
                    },
                    b"external executable",
                )
                .unwrap();
            }
            QuiescenceEvidence::Stopped
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read(if change_backup {
                &f.plan.rollback_path
            } else {
                &f.plan.target_path
            })
            .unwrap(),
            b"external executable"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn safety_contradictory_terminal_records_are_never_automatic_success() {
    for state in [
        UpdateTransactionState::Published,
        UpdateTransactionState::RolledBack,
        UpdateTransactionState::Failed,
    ] {
        let f = fixture();
        let mut j = run(&f, stopped).unwrap();
        if state == UpdateTransactionState::RolledBack {
            j = rollback_staged_update(&j, stopped).unwrap();
        }
        if state == UpdateTransactionState::Failed {
            j.state = state;
            persist_journal(&j).unwrap();
        }
        fs::write(&j.target_path, b"external terminal executable").unwrap();
        let before = fs::read(j.record_path().unwrap()).unwrap();
        assert!(matches!(
            recover_update(&j.record_path().unwrap(), stopped),
            Err(UpdateExecutionError::NeedsReconciliation(_))
        ));
        assert_eq!(target(&f), b"external terminal executable");
        assert_eq!(fs::read(j.record_path().unwrap()).unwrap(), before);
        assert!(
            discover_update_records(&f.installation.installation_root)
                .iter()
                .any(UpdateRecordEntry::needs_attention)
        );
    }
}

#[test]
fn safety_each_journal_path_and_transaction_identity_is_validated_before_mutation() {
    for field in [
        "target",
        "backup",
        "staged",
        "displaced",
        "transaction",
        "old-version",
        "new-version",
    ] {
        let f = fixture();
        with_fault(Fault::Crash(UpdateTransactionState::Applying), || {
            run(&f, stopped).unwrap_err()
        });
        let entry = discover_update_records(&f.installation.installation_root).remove(0);
        let mut j = entry.journal.unwrap();
        let innocent = f._dir.path().join("user-save-data.bin");
        fs::write(&innocent, b"irreplaceable").unwrap();
        match field {
            "target" => j.target_path = innocent.clone(),
            "backup" => j.rollback_path = innocent.clone(),
            "staged" => j.staged_path = Some(innocent.clone()),
            "displaced" => j.displaced_path = Some(innocent.clone()),
            "old-version" => j.old_version = "unsafe/".into(),
            "new-version" => j.new_version = "unsafe/.".into(),
            _ => j.transaction_id = "../outside".into(),
        }
        fs::write(&entry.path, serde_json::to_vec(&j).unwrap()).unwrap();
        let before = fs::read(&entry.path).unwrap();
        assert!(recover_update(&entry.path, stopped).is_err());
        assert_eq!(fs::read(&innocent).unwrap(), b"irreplaceable");
        assert_eq!(fs::read(&entry.path).unwrap(), before);
        assert_eq!(target(&f), OLD);
    }
}

#[test]
fn safety_modified_staging_is_preserved_and_unrelated_temp_is_never_unlinked() {
    let f = fixture();
    with_fault(Fault::Crash(UpdateTransactionState::Applying), || {
        run(&f, stopped).unwrap_err()
    });
    let entry = discover_update_records(&f.installation.installation_root).remove(0);
    let j = entry.journal.unwrap();
    let stage = j.staged_path.unwrap();
    fs::rename(&stage, stage.with_extension("original")).unwrap();
    fs::write(&stage, b"external scratch executable").unwrap();
    assert!(recover_update(&entry.path, stopped).is_err());
    assert_eq!(fs::read(stage).unwrap(), b"external scratch executable");
    let scratch = f
        .plan
        .rollback_path
        .parent()
        .unwrap()
        .join(format!(".{}.journal.tmp", f.plan.transaction_id));
    fs::write(&scratch, b"unowned data").unwrap();
    let mut saved = load_journal(&entry.path).unwrap();
    saved.failure = Some("explicit review still required".into());
    persist_journal(&saved).unwrap();
    assert_eq!(fs::read(scratch).unwrap(), b"unowned data");
}

#[test]
fn safety_superseded_undo_is_not_actionable_even_if_filenames_sort_backwards() {
    let f = fixture();
    let first = run(&f, stopped).unwrap();
    let mut install = f.installation.clone();
    install.version = Some("2.0".into());
    let mut update = update_result(&install);
    update.available_version = Some("3.0".into());
    let mut a = artifact(b"third emulator");
    a.version = "3.0".into();
    let plan = plan_staged_update(&install, &update, a, QuiescenceEvidence::Stopped);
    let second = execute_staged_update(
        &plan,
        &install,
        &update,
        stopped,
        &mut FixtureDownloader {
            bytes: b"third emulator".to_vec(),
        },
    )
    .unwrap();
    let mut records = discover_update_records(&install.installation_root);
    records.reverse();
    assert_eq!(
        actionable_undo(&records, &install.executable_path),
        Some(second.clone())
    );
    assert!(rollback_staged_update(&first, stopped).is_err());
    rollback_staged_update(&second, stopped).unwrap();
    assert!(
        actionable_undo(
            &discover_update_records(&install.installation_root),
            &install.executable_path
        )
        .is_none()
    );
}

#[test]
fn safety_legacy_journals_have_conservative_ordering_compatibility() {
    let f = fixture();
    let mut j = run(&f, stopped).unwrap();
    j.sequence = None;
    j.root_binding = None;
    j.target_parent_binding = None;
    j.original_identity = None;
    j.staged_identity = None;
    // Simulate a journal written by the older schema, not an allowed rewrite
    // of today's immutable identity/ordering receipt.
    let mut old = serde_json::to_value(&j).unwrap();
    for key in [
        "sequence",
        "root_binding",
        "target_parent_binding",
        "original_identity",
        "staged_identity",
    ] {
        old.as_object_mut().unwrap().remove(key);
    }
    fs::write(j.record_path().unwrap(), serde_json::to_vec(&old).unwrap()).unwrap();
    let records = discover_update_records(&f.installation.installation_root);
    // Policy: ownership evidence is required to execute Undo, so a legacy record
    // is never offered (it was offered on content equality alone before).
    assert_eq!(actionable_undo(&records, &j.target_path), None);
    let mut duplicate = records[0].clone();
    duplicate.path = duplicate.path.with_extension("duplicate");
    let mut many = records;
    many.push(duplicate);
    assert!(actionable_undo(&many, &j.target_path).is_none());
}

fn fake_proc(root: &Path, exe: &Path, pid: &str) {
    let p = root.join(pid);
    fs::create_dir_all(&p).unwrap();
    std::os::unix::fs::symlink(exe, p.join("exe")).unwrap();
    fs::write(p.join("cmdline"), b"other-argv-zero\0private-argument\0").unwrap();
}
#[test]
fn safety_proc_uses_device_inode_and_never_reads_environments() {
    let f = fixture();
    let proc = f._dir.path().join("proc");
    fs::create_dir(&proc).unwrap();
    let alias = f._dir.path().join("hardlink");
    fs::hard_link(&f.plan.target_path, &alias).unwrap();
    fake_proc(&proc, &alias, "123");
    fs::create_dir(proc.join("123/environ")).unwrap();
    assert_eq!(
        probe_executable_quiescence_at(&f.plan.target_path, &proc),
        QuiescenceEvidence::Running
    );
}
#[test]
fn safety_proc_uninspectable_processes_and_entries_fail_closed() {
    use std::os::unix::fs::PermissionsExt;
    for fault in ["denied", "loop", "missing-exe", "unreadable-cmdline"] {
        let f = fixture();
        let proc = f._dir.path().join("proc");
        fs::create_dir(&proc).unwrap();
        let other = f._dir.path().join("other");
        fs::write(&other, b"other synthetic binary").unwrap();
        fake_proc(&proc, &other, "123");
        match fault {
            "denied" => {
                fs::set_permissions(proc.join("123"), fs::Permissions::from_mode(0)).unwrap()
            }
            "loop" => {
                fs::remove_file(proc.join("123/exe")).unwrap();
                std::os::unix::fs::symlink("exe", proc.join("123/exe")).unwrap();
            }
            "missing-exe" => {
                fs::remove_file(proc.join("123/exe")).unwrap();
            }
            _ => {
                fs::remove_file(proc.join("123/cmdline")).unwrap();
                fs::create_dir(proc.join("123/cmdline")).unwrap();
            }
        }
        assert_eq!(
            probe_executable_quiescence_at(&f.plan.target_path, &proc),
            QuiescenceEvidence::Unknown,
            "{fault}"
        );
        if fault == "denied" {
            fs::set_permissions(proc.join("123"), fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
}
#[test]
fn safety_proc_stopped_requires_complete_evidence_and_missing_target_uses_backup() {
    let f = fixture();
    let proc = f._dir.path().join("proc");
    fs::create_dir(&proc).unwrap();
    let other = f._dir.path().join("other");
    fs::write(&other, b"other").unwrap();
    fake_proc(&proc, &other, "123");
    assert_eq!(
        probe_executable_quiescence_at(&f.plan.target_path, &proc),
        QuiescenceEvidence::Stopped
    );
    with_fault(
        Fault::CrashBoundary(MutationBoundary::OriginalMoved),
        || run(&f, stopped).unwrap_err(),
    );
    let j = discover_update_records(&f.installation.installation_root)
        .remove(0)
        .journal
        .unwrap();
    assert_eq!(
        process_probe::recorded(&j, &proc),
        QuiescenceEvidence::Stopped
    );
    fake_proc(&proc, &j.rollback_path, "124");
    assert_eq!(
        process_probe::recorded(&j, &proc),
        QuiescenceEvidence::Running
    );
}

#[test]
fn safety_one_kernel_lock_blocks_undo_and_recovery_in_another_process() {
    let f = fixture();
    let j = run(&f, stopped).unwrap();
    let mut child = safety_child(&f, true);
    let before = fs::read(j.record_path().unwrap()).unwrap();
    assert!(matches!(
        rollback_staged_update(&j, stopped),
        Err(UpdateExecutionError::Concurrent(_))
    ));
    assert!(matches!(
        recover_update(&j.record_path().unwrap(), stopped),
        Err(UpdateExecutionError::Concurrent(_))
    ));
    assert_eq!(fs::read(j.record_path().unwrap()).unwrap(), before);
    assert_eq!(target(&f), NEW);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(rollback_staged_update(&j, stopped).is_ok());
}

#[test]
fn safety_replaced_root_and_symlink_alias_are_refused_without_touching_evidence() {
    use std::os::unix::fs::symlink;
    let f = fixture();
    let j = run(&f, stopped).unwrap();
    let root = &f.installation.installation_root;
    let container = tempfile::tempdir().unwrap();
    let alias = container.path().join("alias");
    symlink(root, &alias).unwrap();
    assert!(safety::acquire(&alias.join("dolphin")).is_err());
    let moved = container.path().join("old-root");
    fs::rename(root, &moved).unwrap();
    fs::create_dir(root).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(root.join(ROLLBACK_DIR)).unwrap();
    fs::set_permissions(root.join(ROLLBACK_DIR), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("dolphin"), b"replacement root executable").unwrap();
    let record = j.record_path().unwrap();
    fs::copy(
        moved.join(ROLLBACK_DIR).join(record.file_name().unwrap()),
        &record,
    )
    .unwrap();
    assert!(recover_update(&record, stopped).is_err());
    assert_eq!(
        fs::read(root.join("dolphin")).unwrap(),
        b"replacement root executable"
    );
    assert_eq!(fs::read(moved.join("dolphin")).unwrap(), NEW);
}

#[test]
fn safety_changed_staging_after_original_move_is_never_published() {
    let f = fixture();
    // Use the durable boundary fixture, then modify the owned inode in place.
    with_fault(
        Fault::CrashBoundary(MutationBoundary::OriginalMoved),
        || run(&f, stopped).unwrap_err(),
    );
    let entry = discover_update_records(&f.installation.installation_root).remove(0);
    let j = entry.journal.unwrap();
    let stage = j.staged_path.clone().unwrap();
    fs::write(&stage, b"externally modified stage").unwrap();
    let recovered = recover_update(&entry.path, stopped).unwrap();
    assert_eq!(target(&f), OLD);
    assert_eq!(
        fs::read(recovered.staged_path.unwrap()).unwrap(),
        b"externally modified stage"
    );
}

#[test]
fn safety_running_retained_original_blocks_even_when_new_executable_is_present() {
    use std::os::unix::fs::symlink;
    let f = fixture();
    let j = run(&f, stopped).unwrap();
    let proc = tempfile::tempdir().unwrap();
    fs::create_dir(proc.path().join("123")).unwrap();
    symlink(&j.rollback_path, proc.path().join("123/exe")).unwrap();
    fs::write(proc.path().join("123/cmdline"), b"old-emulator\0").unwrap();
    assert_eq!(
        process_probe::recorded(&j, proc.path()),
        QuiescenceEvidence::Running
    );
}

#[test]
fn safety_alternate_enclosing_roots_cannot_hide_newer_transaction_ordering() {
    let f = fixture();
    let j = run(&f, stopped).unwrap();
    let parent = f.installation.installation_root.parent().unwrap();
    assert!(
        safety::Paths::new(
            parent,
            &j.target_path,
            &j.transaction_id,
            j.emulator,
            &j.old_version,
            &j.new_version
        )
        .is_err()
    );
    assert_eq!(target(&f), NEW);
}

fn fresh_run(f: &Fixture) -> Result<UpdateJournal, UpdateExecutionError> {
    let plan = plan_staged_update(
        &f.installation,
        &f.update,
        artifact(NEW),
        QuiescenceEvidence::Stopped,
    );
    execute_staged_update(
        &plan,
        &f.installation,
        &f.update,
        stopped,
        &mut FixtureDownloader {
            bytes: NEW.to_vec(),
        },
    )
}
#[test]
fn safety_closed_transaction_requires_new_preview_instead_of_reusing_receipts() {
    let f = fixture();
    let mut calls = 0;
    run(&f, || {
        calls += 1;
        if calls == 1 {
            QuiescenceEvidence::Stopped
        } else {
            QuiescenceEvidence::Unknown
        }
    })
    .unwrap_err();
    let entry = discover_update_records(&f.installation.installation_root).remove(0);
    let before = fs::read(&entry.path).unwrap();
    assert!(matches!(
        run(&f, stopped),
        Err(UpdateExecutionError::Record(_))
    ));
    assert_eq!(fs::read(&entry.path).unwrap(), before);
    assert!(
        !f.plan
            .target_path
            .with_file_name(format!(".emuwiz-update-staging-{}", f.plan.transaction_id))
            .exists()
    );
    assert!(fresh_run(&f).is_ok());
}

#[test]
fn safety_crash_before_first_journal_preserves_orphan_and_releases_kernel_lock() {
    let f = fixture();
    with_fault(Fault::CrashBoundary(MutationBoundary::StageCreated), || {
        run(&f, stopped).unwrap_err()
    });
    assert!(discover_update_records(&f.installation.installation_root).is_empty());
    let stage = f
        .plan
        .target_path
        .with_file_name(format!(".emuwiz-update-staging-{}", f.plan.transaction_id));
    assert_eq!(fs::read(&stage).unwrap(), b"");
    assert_eq!(target(&f), OLD);
    assert!(fresh_run(&f).is_ok());
    assert_eq!(fs::read(stage).unwrap(), b"");
}
