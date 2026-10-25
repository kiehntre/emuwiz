// Included inside the existing synthetic updater test module (P0 safety closure).
// Every test uses synthetic executables in private temp directories only.

fn lock_entries(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".emuwiz-update-lock"))
        })
        .collect()
}

/// A live updater in another process holds the target lock. Whatever its lock is
/// (a pathname, or none), no pathname rename/replace may let a second operation
/// obtain an independent lock and act.
#[test]
fn closure_renaming_or_replacing_a_live_lock_pathname_does_not_admit_another_updater() {
    let f = fixture();
    let mut child = safety_child(&f, true);
    let root = f.installation.installation_root.clone();
    // Hostile or accidental: move aside, then replace, every lock-like entry.
    for (index, entry) in lock_entries(&root).into_iter().enumerate() {
        let moved = root.join(format!("moved-lock-{index}"));
        fs::rename(&entry, &moved).unwrap();
        fs::write(&entry, b"replacement").unwrap();
    }
    let attempt = run(&f, stopped);
    assert!(
        matches!(attempt, Err(UpdateExecutionError::Concurrent(_))),
        "a second updater was admitted while another process holds the lock: {attempt:?}"
    );
    assert_eq!(target(&f), OLD);
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn closure_recovery_cannot_act_on_a_live_updaters_journal_after_lock_pathname_replacement() {
    let f = fixture();
    let mut child = safety_child(&f, false);
    let root = f.installation.installation_root.clone();
    let record = discover_update_records(&root).remove(0);
    let before = fs::read(&record.path).unwrap();
    for (index, entry) in lock_entries(&root).into_iter().enumerate() {
        fs::rename(&entry, root.join(format!("moved-lock-{index}"))).unwrap();
        fs::write(&entry, b"replacement").unwrap();
    }
    let attempt = recover_update(&record.path, stopped);
    assert!(
        matches!(attempt, Err(UpdateExecutionError::Concurrent(_))),
        "recovery acted on a live updater's journal: {attempt:?}"
    );
    assert_eq!(fs::read(&record.path).unwrap(), before);
    assert_eq!(target(&f), OLD);
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn closure_replacing_the_installation_directory_cannot_reach_the_live_updaters_evidence() {
    let f = fixture();
    let mut child = safety_child(&f, false);
    let root = f.installation.installation_root.clone();
    let record = discover_update_records(&root).remove(0);
    let moved = root.with_file_name(format!(
        "{}-moved",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(&root, &moved).unwrap();
    fs::create_dir(&root).unwrap();
    let moved_record = moved
        .join(ROLLBACK_DIR)
        .join(record.path.file_name().unwrap());
    let before = fs::read(&moved_record).unwrap();
    assert!(recover_update(&moved_record, stopped).is_err());
    assert!(recover_update(&record.path, stopped).is_err());
    assert_eq!(fs::read(&moved_record).unwrap(), before);
    child.kill().unwrap();
    child.wait().unwrap();
    fs::remove_dir(&root).unwrap();
    fs::rename(&moved, &root).unwrap();
}

#[test]
fn closure_hard_linked_executable_aliases_are_refused_before_any_mutation() {
    for alias_in_other_directory in [false, true] {
        let f = fixture();
        let root = f.installation.installation_root.clone();
        let alias_directory = if alias_in_other_directory {
            let other = tempfile::tempdir().unwrap();
            let path = other.path().to_path_buf();
            std::mem::forget(other); // freed with the OS temp area; keeps the alias alive
            path
        } else {
            root.clone()
        };
        let alias = alias_directory.join("alias-of-dolphin");
        fs::hard_link(&f.installation.executable_path, &alias).unwrap();
        let result = run(&f, stopped);
        assert!(
            result.is_err(),
            "hard-linked executable was updated: {result:?}"
        );
        assert_eq!(target(&f), OLD);
        assert_eq!(fs::read(&alias).unwrap(), OLD);
        assert!(
            !root.join(ROLLBACK_DIR).exists(),
            "refusal must precede any filesystem mutation"
        );
        assert!(discover_update_records(&root).is_empty());
        assert!(lock_entries(&root).is_empty());
    }
}

#[test]
fn closure_interrupted_undo_with_an_identical_content_substituted_backup_is_never_success() {
    let f = fixture();
    let j = run(&f, stopped).unwrap();
    with_fault(
        Fault::CrashBoundary(MutationBoundary::UndoDisplaced),
        || rollback_staged_update(&j, stopped).unwrap_err(),
    );
    assert!(!j.target_path.exists());
    // Same bytes, different inode.
    let substitute = j.target_path.with_file_name("substitute-backup");
    fs::copy(&j.rollback_path, &substitute).unwrap();
    fs::rename(&substitute, &j.rollback_path).unwrap();
    let record = j.record_path().unwrap();
    let before = fs::read(&record).unwrap();
    let result = recover_update(&record, stopped);
    assert!(
        matches!(result, Err(UpdateExecutionError::NeedsReconciliation(_))),
        "contradictory backup identity was accepted: {result:?}"
    );
    assert!(
        !j.target_path.exists(),
        "nothing may move on contradictory evidence"
    );
    assert_eq!(fs::read(&record).unwrap(), before);
    assert!(
        discover_update_records(&f.installation.installation_root)
            .iter()
            .any(UpdateRecordEntry::needs_attention)
    );
}

#[test]
fn closure_quiescence_turning_unknown_after_the_executable_moved_is_reported_as_partial() {
    let f = fixture();
    let j = run(&f, stopped).unwrap();
    let calls = std::cell::Cell::new(0);
    let evidence = || {
        calls.set(calls.get() + 1);
        if calls.get() >= 3 {
            QuiescenceEvidence::Unknown
        } else {
            QuiescenceEvidence::Stopped
        }
    };
    let error = rollback_staged_update(&j, evidence).unwrap_err();
    // The published executable has already been moved aside.
    assert!(!j.target_path.exists());
    let text = error.to_string();
    assert!(
        matches!(error, UpdateExecutionError::NeedsReconciliation(_))
            && text.contains("already moved"),
        "partial operation reported as: {error:?}"
    );
    let record = j.record_path().unwrap();
    let saved = load_journal(&record).unwrap();
    assert_eq!(saved.state, UpdateTransactionState::Undoing);
    assert!(
        saved
            .failure
            .as_deref()
            .is_some_and(|f| f.contains("already moved"))
    );
    // Restart + repeated recovery: completes once, then is idempotent.
    let first = recover_update(&record, stopped).unwrap();
    assert_eq!(first.state, UpdateTransactionState::Published);
    assert_eq!(target(&f), NEW);
    let second = recover_update(&record, stopped).unwrap();
    assert_eq!(first, second);
    assert_eq!(fs::read(&record).unwrap(), {
        let again = recover_update(&record, stopped).unwrap();
        assert_eq!(again, first);
        fs::read(&record).unwrap()
    });
}

fn strip_to_legacy(f: &Fixture, j: &UpdateJournal) {
    let mut old = serde_json::to_value(j).unwrap();
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
    let _ = f;
}

#[test]
fn closure_legacy_records_without_ownership_evidence_are_neither_actionable_nor_executable() {
    let f = fixture();
    let j = run(&f, stopped).unwrap();
    strip_to_legacy(&f, &j);
    let records = discover_update_records(&f.installation.installation_root);
    assert!(
        actionable_undo(&records, &j.target_path).is_none(),
        "a legacy record without identity evidence is offered as Undo"
    );
    let legacy = records[0].journal.clone().unwrap();
    let before_target = fs::read(&j.target_path).unwrap();
    let result = rollback_staged_update(&legacy, stopped);
    assert!(
        result.is_err(),
        "legacy Undo executed on hash equality alone: {result:?}"
    );
    assert_eq!(fs::read(&j.target_path).unwrap(), before_target);
    assert!(!legacy.displaced_path.as_ref().is_some_and(|p| p.exists()));
    assert!(
        fs::read_dir(f.installation.installation_root.join(ROLLBACK_DIR))
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".displaced"))
    );
}

#[test]
fn closure_legacy_interrupted_record_is_not_moved_on_hash_equality_alone() {
    let f = fixture();
    with_fault(
        Fault::CrashBoundary(MutationBoundary::OriginalMoved),
        || run(&f, stopped).unwrap_err(),
    );
    let record = discover_update_records(&f.installation.installation_root).remove(0);
    let legacy = record.journal.clone().unwrap();
    strip_to_legacy(&f, &legacy);
    let result = recover_update(&record.path, stopped);
    assert!(
        matches!(result, Err(UpdateExecutionError::NeedsReconciliation(_))),
        "{result:?}"
    );
    assert!(!f.installation.executable_path.exists());
    assert!(legacy.rollback_path.exists());
}

#[test]
fn closure_unknown_and_running_are_distinct_in_planning() {
    let f = fixture();
    let plan = |q| plan_staged_update(&f.installation, &f.update, artifact(NEW), q);
    assert_eq!(
        plan(QuiescenceEvidence::Running).eligibility,
        UpdateExecutionEligibility::RunningBlocked
    );
    assert_ne!(
        plan(QuiescenceEvidence::Unknown).eligibility,
        UpdateExecutionEligibility::RunningBlocked,
        "an unverifiable process view must not be labelled as a running emulator"
    );
    assert_ne!(
        plan(QuiescenceEvidence::Unknown).eligibility,
        UpdateExecutionEligibility::Ready
    );
}

#[test]
fn closure_unsupported_storage_is_refused_before_any_directory_is_created() {
    let f = fixture();
    safety::STORAGE_UNSUPPORTED.with(|flag| flag.set(true));
    let root = f.installation.installation_root.clone();
    let apply = run(&f, stopped);
    safety::STORAGE_UNSUPPORTED.with(|flag| flag.set(false));
    assert!(
        matches!(&apply, Err(UpdateExecutionError::Record(m)) if m.contains("updates require verified")),
        "{apply:?}"
    );
    assert_eq!(target(&f), OLD);
    assert!(
        !root.join(ROLLBACK_DIR).exists(),
        "the refusal created a directory"
    );
    assert!(lock_entries(&root).is_empty());
    // Undo and recovery refuse on the same ground before moving anything.
    let j = run(&f, stopped).unwrap();
    safety::STORAGE_UNSUPPORTED.with(|flag| flag.set(true));
    let undo = rollback_staged_update(&j, stopped);
    let recovery = recover_update(&j.record_path().unwrap(), stopped);
    safety::STORAGE_UNSUPPORTED.with(|flag| flag.set(false));
    assert!(undo.is_err() && recovery.is_err());
    assert_eq!(target(&f), NEW);
}

#[test]
fn closure_hard_linked_aliases_block_undo_and_recovery_before_moving_anything() {
    let f = fixture();
    let j = run(&f, stopped).unwrap();
    let alias = f.installation.installation_root.join("alias-of-published");
    fs::hard_link(&j.target_path, &alias).unwrap();
    let before = fs::read(j.record_path().unwrap()).unwrap();
    let undo = rollback_staged_update(&j, stopped);
    assert!(
        matches!(undo, Err(UpdateExecutionError::UnsafeTarget(_))),
        "{undo:?}"
    );
    assert_eq!(target(&f), NEW);
    assert_eq!(fs::read(j.record_path().unwrap()).unwrap(), before);

    let g = fixture();
    with_fault(
        Fault::CrashBoundary(MutationBoundary::OriginalMoved),
        || run(&g, stopped).unwrap_err(),
    );
    let record = discover_update_records(&g.installation.installation_root).remove(0);
    let backup = record.journal.clone().unwrap().rollback_path;
    fs::hard_link(
        &backup,
        g.installation.installation_root.join("alias-of-backup"),
    )
    .unwrap();
    let recovery = recover_update(&record.path, stopped);
    assert!(
        matches!(recovery, Err(UpdateExecutionError::UnsafeTarget(_))),
        "{recovery:?}"
    );
    assert!(!g.installation.executable_path.exists());
    assert!(backup.exists());
}

#[test]
fn closure_the_installation_lock_is_the_directory_inode_not_a_pathname() {
    let f = fixture();
    let first = safety::acquire(&f.plan.target_path).unwrap();
    assert!(
        lock_entries(&f.installation.installation_root).is_empty(),
        "no lock pathname exists for anyone to rename, replace or unlink"
    );
    assert!(matches!(
        safety::acquire(&f.plan.target_path),
        Err(UpdateExecutionError::Concurrent(_))
    ));
    drop(first);
    // flock belongs to the open file description. Another test thread that forks
    // (to spawn a worker) at this instant holds a CLOEXEC copy until its exec, so
    // the release can be delayed by microseconds. Wait for it; a lock that never
    // releases would still fail this test.
    let mut released = false;
    for _ in 0..200 {
        if safety::acquire(&f.plan.target_path).is_ok() {
            released = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        released,
        "the kernel lock was never released after the holder closed it"
    );
}
