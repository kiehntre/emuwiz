use super::*;
use crate::save_snapshots::{
    EmulatorUseStatus, SaveLocation, SaveProvenance, SaveSnapshotRequest, create_snapshot,
};
use std::cell::Cell;
use std::os::unix::fs::symlink;
use tempfile::{TempDir, tempdir};

type State = BTreeMap<String, Option<Vec<u8>>>;

struct Env {
    tmp: TempDir,
}

impl Env {
    fn new() -> Self {
        let tmp = tempdir().unwrap();
        fs::create_dir(tmp.path().join("saves")).unwrap();
        Self { tmp }
    }
    fn dest(&self) -> PathBuf {
        self.tmp.path().join("saves/game")
    }
    fn options(&self) -> TreeRestoreOptions {
        TreeRestoreOptions {
            journal_root: self.tmp.path().join("journal"),
            now_unix_seconds: 1_000,
        }
    }
    /// Seeds the destination with the given files (parents created).
    fn seed(&self, files: &[(&str, &str)]) {
        for (name, body) in files {
            let path = self.dest().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
    }
    fn snapshot(&self, id: &str, files: &[(&str, &str)]) -> SaveSnapshot {
        let source = self.tmp.path().join(format!("source-{id}"));
        for (name, body) in files {
            let path = source.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        create_snapshot(&SaveSnapshotRequest {
            location: SaveLocation {
                path: source,
                emulator: Some("Synthetic".into()),
                profile: None,
                artifact_type: SaveArtifactType::SaveDirectory,
                provenance: SaveProvenance::UserSpecified,
            },
            game_identity: Some("game".into()),
            platform: None,
            storage_root: self.tmp.path().join("snapshots"),
            available_space_bytes: None,
            snapshot_id: Some(id.into()),
            now_unix_seconds: Some(1),
            emulator_use: EmulatorUseStatus::NotDetected,
        })
        .unwrap()
    }
    fn plan(&self, snapshot: &SaveSnapshot) -> TreeRestorePlan {
        plan_tree_restore(
            snapshot,
            &self.dest(),
            SaveQuiescenceRequirement::ConfirmedClosed,
        )
    }
    /// Every file and directory under the whole `saves` parent, so leaked
    /// staging or preserved material is visible too.
    fn everything(&self) -> State {
        state_of(&self.tmp.path().join("saves"))
    }
    fn dest_state(&self) -> State {
        state_of(&self.dest())
    }
    fn leftovers(&self) -> Vec<String> {
        fs::read_dir(self.tmp.path().join("saves"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".emuwiz-restore"))
            .collect()
    }
}

fn state_of(root: &Path) -> State {
    let mut out = State::new();
    if !root.exists() {
        return out;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let key = path.strip_prefix(root).unwrap().display().to_string();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                out.insert(key, None);
                stack.push(path);
            } else if meta.is_file() {
                out.insert(key, Some(fs::read(&path).unwrap()));
            } else {
                out.insert(key, Some(b"<link-or-special>".to_vec()));
            }
        }
    }
    out
}

fn expect_files(env: &Env, files: &[(&str, &str)]) {
    let state = env.dest_state();
    for (name, body) in files {
        assert_eq!(
            state.get(*name),
            Some(&Some(body.as_bytes().to_vec())),
            "{name}"
        );
    }
}

#[test]
fn restore_into_an_absent_destination_creates_it_and_undo_removes_it() {
    let env = Env::new();
    let snapshot = env.snapshot("s1", &[("config.dat", "C"), ("sub/profile.bin", "P")]);
    let before = env.everything();
    let plan = env.plan(&snapshot);
    assert!(plan.ready(), "{:?}", plan.refusals);
    assert!(!plan.root_previously_existed);
    assert_eq!(plan.directories_to_create, vec![PathBuf::from("sub")]);
    assert_eq!(env.everything(), before, "planning writes nothing");

    let journal = apply_tree_restore(&snapshot, &plan, &env.options()).unwrap();
    assert_eq!(journal.status, TreeRestoreStatus::Published);
    expect_files(&env, &[("config.dat", "C"), ("sub/profile.bin", "P")]);
    assert!(
        journal
            .entries
            .iter()
            .all(|e| e.action == EntryAction::Create)
    );
    assert!(!journal.work_dir.join("stage").exists());

    let undone = undo_tree_restore(
        &journal.journal_path,
        SaveQuiescenceRequirement::ConfirmedClosed,
        2,
    )
    .unwrap();
    assert_eq!(undone.status, TreeRestoreStatus::Undone);
    assert_eq!(
        env.everything(),
        before,
        "undo removes the created tree and root"
    );
}

#[test]
fn replacing_one_file_preserves_the_rest_and_undo_is_exact() {
    let env = Env::new();
    env.seed(&[("slot1.sav", "OLD"), ("extra.txt", "KEEP")]);
    let snapshot = env.snapshot("s2", &[("slot1.sav", "NEW")]);
    let before = env.everything();
    let plan = env.plan(&snapshot);
    assert_eq!(plan.entries[0].action, EntryAction::ReplaceFile);
    assert_eq!(plan.untouched_files, vec![PathBuf::from("extra.txt")]);
    let journal = apply_tree_restore(&snapshot, &plan, &env.options()).unwrap();
    expect_files(&env, &[("slot1.sav", "NEW"), ("extra.txt", "KEEP")]);
    undo_tree_restore(
        &journal.journal_path,
        SaveQuiescenceRequirement::ConfirmedClosed,
        2,
    )
    .unwrap();
    assert_eq!(env.everything(), before);
    assert!(env.leftovers().is_empty());
}

#[test]
fn replacing_many_nested_files_with_a_create_replace_mixture_round_trips() {
    let env = Env::new();
    env.seed(&[
        ("a.sav", "a0"),
        ("d1/b.sav", "b0"),
        ("d1/d2/c.sav", "c0"),
        ("z.keep", "z"),
    ]);
    let snapshot = env.snapshot(
        "s3",
        &[
            ("a.sav", "a1"),
            ("d1/b.sav", "b1"),
            ("d1/d2/c.sav", "c1"),
            ("d1/d2/new.sav", "n1"),
            ("fresh/deep/er/x.bin", "x1"),
        ],
    );
    let before = env.everything();
    let plan = env.plan(&snapshot);
    assert_eq!(
        plan.entries
            .iter()
            .filter(|e| e.action == EntryAction::Create)
            .count(),
        2
    );
    assert_eq!(
        plan.directories_to_create,
        vec![
            PathBuf::from("fresh"),
            PathBuf::from("fresh/deep"),
            PathBuf::from("fresh/deep/er")
        ]
    );
    let journal = apply_tree_restore(&snapshot, &plan, &env.options()).unwrap();
    expect_files(
        &env,
        &[
            ("a.sav", "a1"),
            ("d1/b.sav", "b1"),
            ("d1/d2/c.sav", "c1"),
            ("d1/d2/new.sav", "n1"),
            ("fresh/deep/er/x.bin", "x1"),
            ("z.keep", "z"),
        ],
    );
    assert_eq!(journal.created_directories.len(), 3);
    undo_tree_restore(
        &journal.journal_path,
        SaveQuiescenceRequirement::ConfirmedClosed,
        2,
    )
    .unwrap();
    assert_eq!(env.everything(), before);
}

#[test]
fn directory_and_file_shape_changes_preserve_and_restore_the_old_objects() {
    let env = Env::new();
    // `cfg` is a file but the snapshot needs a directory; `old` is a directory
    // tree but the snapshot has a file there.
    env.seed(&[
        ("cfg", "I-am-a-file"),
        ("old/inner/a.bin", "A"),
        ("old/b.bin", "B"),
    ]);
    let snapshot = env.snapshot("s4", &[("cfg/new.dat", "N"), ("old", "now-a-file")]);
    let before = env.everything();
    let plan = env.plan(&snapshot);
    assert!(plan.ready(), "{:?}", plan.refusals);
    assert_eq!(plan.displaced_files.len(), 1);
    assert!(
        plan.entries
            .iter()
            .any(|e| e.action == EntryAction::ReplaceDirectoryWithFile)
    );
    let journal = apply_tree_restore(&snapshot, &plan, &env.options()).unwrap();
    expect_files(&env, &[("cfg/new.dat", "N"), ("old", "now-a-file")]);
    undo_tree_restore(
        &journal.journal_path,
        SaveQuiescenceRequirement::ConfirmedClosed,
        2,
    )
    .unwrap();
    assert_eq!(env.everything(), before);
}

fn refusals(env: &Env, snapshot: &SaveSnapshot) -> Vec<TreeRestoreRefusal> {
    let plan = env.plan(snapshot);
    assert!(!plan.ready());
    plan.refusals
}

#[test]
fn duplicate_traversal_and_collision_entries_are_refused() {
    let env = Env::new();
    let good = env.snapshot("s5", &[("a.sav", "1"), ("b.sav", "2")]);

    let mut duplicate = good.clone();
    duplicate
        .manifest
        .artifacts
        .push(good.manifest.artifacts[0].clone());
    assert!(
        refusals(&env, &duplicate)
            .iter()
            .any(|r| matches!(r, TreeRestoreRefusal::DuplicateDestination(_)))
    );

    for bad in ["../escape.sav", "/abs/path.sav", "a/../../b", ""] {
        let mut traversal = good.clone();
        traversal.manifest.artifacts[0].relative_path = PathBuf::from(bad);
        assert!(
            refusals(&env, &traversal)
                .iter()
                .any(|r| matches!(r, TreeRestoreRefusal::UnsafeRelativePath(_))),
            "{bad}"
        );
    }
    assert!(!env.dest().exists() && !env.tmp.path().join("escape.sav").exists());

    // `a.sav` as a file and `a.sav/x` as a directory member cannot coexist.
    let mut clash = good.clone();
    let mut inner = good.manifest.artifacts[1].clone();
    inner.relative_path = PathBuf::from("a.sav/x");
    clash.manifest.artifacts.push(inner);
    assert!(
        refusals(&env, &clash)
            .iter()
            .any(|r| matches!(r, TreeRestoreRefusal::PathCollision(_)))
    );
}

#[test]
fn case_colliding_names_are_refused() {
    let env = Env::new();
    let both = env.snapshot("s6", &[("Slot.SAV", "1"), ("slot.sav", "2")]);
    assert!(
        refusals(&env, &both)
            .iter()
            .any(|r| matches!(r, TreeRestoreRefusal::CaseCollision(_)))
    );

    let env = Env::new();
    env.seed(&[("SLOT.SAV", "existing")]);
    let snapshot = env.snapshot("s7", &[("slot.sav", "1")]);
    assert!(
        refusals(&env, &snapshot)
            .iter()
            .any(|r| matches!(r, TreeRestoreRefusal::CaseCollision(_)))
    );
    assert_eq!(
        fs::read_to_string(env.dest().join("SLOT.SAV")).unwrap(),
        "existing"
    );
}

#[test]
fn symlinks_and_special_files_in_the_destination_are_refused() {
    // Symlink at a path the snapshot would write.
    let env = Env::new();
    let outside = env.tmp.path().join("outside.txt");
    fs::write(&outside, "outside").unwrap();
    fs::create_dir_all(env.dest()).unwrap();
    symlink(&outside, env.dest().join("slot.sav")).unwrap();
    let snapshot = env.snapshot("s8", &[("slot.sav", "x")]);
    assert!(
        refusals(&env, &snapshot)
            .iter()
            .any(|r| matches!(r, TreeRestoreRefusal::SymlinkOrSpecialInDestination(_)))
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");

    // Symlinked intermediate directory.
    let env = Env::new();
    let target_dir = env.tmp.path().join("elsewhere");
    fs::create_dir(&target_dir).unwrap();
    fs::create_dir_all(env.dest()).unwrap();
    symlink(&target_dir, env.dest().join("sub")).unwrap();
    let snapshot = env.snapshot("s9", &[("sub/x.bin", "x")]);
    assert!(!env.plan(&snapshot).ready());
    assert!(!target_dir.join("x.bin").exists());

    // Destination root itself a symlink.
    let env = Env::new();
    let real = env.tmp.path().join("real-root");
    fs::create_dir(&real).unwrap();
    symlink(&real, env.dest()).unwrap();
    let snapshot = env.snapshot("s10", &[("a", "1")]);
    assert!(
        refusals(&env, &snapshot)
            .iter()
            .any(|r| matches!(r, TreeRestoreRefusal::DestinationUnsafe(_)))
    );

    // FIFO (special file) at a planned path.
    let env = Env::new();
    fs::create_dir_all(env.dest()).unwrap();
    let fifo =
        std::ffi::CString::new(env.dest().join("slot.sav").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let snapshot = env.snapshot("s11", &[("slot.sav", "x")]);
    assert!(
        refusals(&env, &snapshot)
            .iter()
            .any(|r| matches!(r, TreeRestoreRefusal::SymlinkOrSpecialInDestination(_)))
    );
}

#[test]
fn running_or_unknown_emulator_and_save_states_are_refused() {
    let env = Env::new();
    let snapshot = env.snapshot("s12", &[("a", "1")]);
    for (q, want) in [
        (
            SaveQuiescenceRequirement::Running,
            TreeRestoreRefusal::EmulatorRunning,
        ),
        (
            SaveQuiescenceRequirement::Unknown,
            TreeRestoreRefusal::EmulatorStateUnknown,
        ),
    ] {
        let plan = plan_tree_restore(&snapshot, &env.dest(), q);
        assert!(plan.refusals.contains(&want));
        assert!(apply_tree_restore(&snapshot, &plan, &env.options()).is_err());
    }
    let mut state = snapshot.clone();
    state.manifest.artifact_type = SaveArtifactType::SaveState;
    assert!(refusals(&env, &state).contains(&TreeRestoreRefusal::SaveStateNotSupported));
    assert!(!env.dest().exists());
}

#[test]
fn staging_failure_leaves_the_destination_and_parent_untouched() {
    let env = Env::new();
    env.seed(&[("a.sav", "old")]);
    let snapshot = env.snapshot("s13", &[("a.sav", "new"), ("b.sav", "new2")]);
    let before = env.everything();
    let plan = env.plan(&snapshot);
    let hook = |phase: Phase| match phase {
        Phase::Stage(1) => Err(io::Error::other("disk full")),
        _ => Ok(()),
    };
    let error = apply_with_hook(&snapshot, &plan, &env.options(), &hook).unwrap_err();
    assert!(matches!(error, TreeRestoreError::Io { .. }), "{error}");
    assert_eq!(
        env.everything(),
        before,
        "no live change, no leaked staging"
    );
    // The lock was released: a clean retry succeeds.
    apply_tree_restore(&snapshot, &env.plan(&snapshot), &env.options()).unwrap();
}

fn publish_steps(env: &Env, snapshot: &SaveSnapshot) -> usize {
    let counter = Cell::new(0usize);
    let hook = |phase: Phase| {
        if let Phase::Publish(n) = phase {
            counter.set(counter.get().max(n + 1));
        }
        Ok(())
    };
    let journal = apply_with_hook(snapshot, &env.plan(snapshot), &env.options(), &hook).unwrap();
    undo_tree_restore(
        &journal.journal_path,
        SaveQuiescenceRequirement::ConfirmedClosed,
        2,
    )
    .unwrap();
    counter.get()
}

fn mixed_seed(env: &Env) {
    env.seed(&[
        ("a.sav", "a0"),
        ("d/b.sav", "b0"),
        ("cfg", "file"),
        ("old/x.bin", "X"),
    ]);
}

fn mixed_snapshot(env: &Env, id: &str) -> SaveSnapshot {
    env.snapshot(
        id,
        &[
            ("a.sav", "a1"),
            ("d/b.sav", "b1"),
            ("d/new.sav", "n1"),
            ("cfg/inner.dat", "I"),
            ("old", "was-dir"),
            ("fresh/f.bin", "F"),
        ],
    )
}

#[test]
fn a_publish_failure_at_every_single_step_rolls_back_to_the_exact_original() {
    let probe = Env::new();
    mixed_seed(&probe);
    let steps = publish_steps(&probe, &mixed_snapshot(&probe, "probe"));
    assert!(steps >= 8, "{steps}");

    for fail_at in 0..steps {
        let env = Env::new();
        mixed_seed(&env);
        let snapshot = mixed_snapshot(&env, "fault");
        let before = env.everything();
        let plan = env.plan(&snapshot);
        let hook = |phase: Phase| match phase {
            Phase::Publish(n) if n == fail_at => Err(io::Error::other("injected")),
            _ => Ok(()),
        };
        let error = apply_with_hook(&snapshot, &plan, &env.options(), &hook).unwrap_err();
        let TreeRestoreError::RolledBack { journal_path, .. } = error else {
            panic!("step {fail_at}: {error}");
        };
        assert_eq!(
            env.everything(),
            before,
            "step {fail_at} left a half-restored tree"
        );
        let journal = load_tree_restore_journal(&journal_path).unwrap();
        assert_eq!(journal.status, TreeRestoreStatus::RolledBack);
        // And the destination is usable again.
        apply_tree_restore(&snapshot, &env.plan(&snapshot), &env.options()).unwrap();
    }
}

#[test]
fn a_real_publish_collision_rolls_back_and_a_new_file_is_never_clobbered() {
    let env = Env::new();
    env.seed(&[("a.sav", "a0")]);
    let snapshot = env.snapshot("s14", &[("a.sav", "a1"), ("n.sav", "n1")]);
    let plan = env.plan(&snapshot);
    // A game writes n.sav between planning and publishing.
    let dest = env.dest();
    let hook = |phase: Phase| {
        if phase == Phase::Publish(1) {
            fs::write(dest.join("n.sav"), "GAME-WROTE-THIS").unwrap();
        }
        Ok(())
    };
    let error = apply_with_hook(&snapshot, &plan, &env.options(), &hook).unwrap_err();
    assert!(
        matches!(error, TreeRestoreError::RolledBack { .. }),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(dest.join("n.sav")).unwrap(),
        "GAME-WROTE-THIS"
    );
    assert_eq!(fs::read_to_string(dest.join("a.sav")).unwrap(), "a0");
}

#[test]
fn a_failed_rollback_keeps_all_evidence_and_recovery_finishes_it() {
    let env = Env::new();
    mixed_seed(&env);
    let snapshot = mixed_snapshot(&env, "s15");
    let before = env.everything();
    let plan = env.plan(&snapshot);
    let hook = |phase: Phase| match phase {
        Phase::Publish(5) => Err(io::Error::other("publish broke")),
        Phase::Rollback(2) => Err(io::Error::other("rollback broke too")),
        _ => Ok(()),
    };
    let error = apply_with_hook(&snapshot, &plan, &env.options(), &hook).unwrap_err();
    let TreeRestoreError::RecoveryRequired {
        journal_path,
        detail,
    } = error
    else {
        panic!("{error}");
    };
    assert!(detail.contains("rollback incomplete"), "{detail}");
    let journal = load_tree_restore_journal(&journal_path).unwrap();
    assert_eq!(journal.status, TreeRestoreStatus::RecoveryRequired);
    assert!(journal.work_dir.join("preserved").exists(), "evidence kept");
    assert!(
        journal.lock_path.exists(),
        "hard stop: no new restore may start"
    );
    let blocked = apply_tree_restore(&snapshot, &plan, &env.options()).unwrap_err();
    assert!(matches!(blocked, TreeRestoreError::Refused(_)), "{blocked}");

    // A fresh process can finish the job from the journal alone.
    let recovered = recover_tree_restore(&journal_path, 3).unwrap();
    assert_eq!(recovered.status, TreeRestoreStatus::RolledBack);
    assert_eq!(env.everything(), before);
}

#[test]
fn undo_after_the_game_modified_a_restored_save_refuses_and_changes_nothing() {
    let env = Env::new();
    env.seed(&[("slot1.sav", "OLD")]);
    let snapshot = env.snapshot("s16", &[("slot1.sav", "RESTORED"), ("new/p.bin", "P")]);
    let journal = apply_tree_restore(&snapshot, &env.plan(&snapshot), &env.options()).unwrap();

    fs::write(env.dest().join("slot1.sav"), "PLAYED-AND-SAVED").unwrap();
    let after_play = env.everything();
    let error = undo_tree_restore(
        &journal.journal_path,
        SaveQuiescenceRequirement::ConfirmedClosed,
        2,
    )
    .unwrap_err();
    let TreeRestoreError::UndoConflict { conflicts, .. } = error else {
        panic!("{error}");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, PathBuf::from("slot1.sav"));
    assert_eq!(env.everything(), after_play, "newer save untouched");
    assert_eq!(
        load_tree_restore_journal(&journal.journal_path)
            .unwrap()
            .status,
        TreeRestoreStatus::Published
    );

    // A new file added into a directory the restore created also blocks undo.
    fs::write(env.dest().join("slot1.sav"), "RESTORED").unwrap();
    fs::write(env.dest().join("new/extra.bin"), "user data").unwrap();
    let error = undo_tree_restore(
        &journal.journal_path,
        SaveQuiescenceRequirement::ConfirmedClosed,
        2,
    )
    .unwrap_err();
    assert!(matches!(error, TreeRestoreError::UndoConflict { .. }));
    assert!(env.dest().join("new/extra.bin").exists());
}

#[test]
fn undo_refuses_when_preserved_originals_are_gone_and_when_emulator_is_running() {
    let env = Env::new();
    env.seed(&[("a.sav", "OLD")]);
    let snapshot = env.snapshot("s17", &[("a.sav", "NEW")]);
    let journal = apply_tree_restore(&snapshot, &env.plan(&snapshot), &env.options()).unwrap();
    assert!(matches!(
        undo_tree_restore(&journal.journal_path, SaveQuiescenceRequirement::Running, 2),
        Err(TreeRestoreError::Refused(_))
    ));
    fs::remove_dir_all(journal.work_dir.join("preserved")).unwrap();
    assert!(matches!(
        undo_tree_restore(
            &journal.journal_path,
            SaveQuiescenceRequirement::ConfirmedClosed,
            2
        ),
        Err(TreeRestoreError::UndoConflict { .. })
    ));
    assert_eq!(fs::read_to_string(env.dest().join("a.sav")).unwrap(), "NEW");
}

#[test]
fn undo_failure_midway_is_recoverable_from_the_journal() {
    let env = Env::new();
    mixed_seed(&env);
    let snapshot = mixed_snapshot(&env, "s18");
    let before = env.everything();
    let journal = apply_tree_restore(&snapshot, &env.plan(&snapshot), &env.options()).unwrap();
    let hook = |phase: Phase| match phase {
        Phase::Rollback(3) => Err(io::Error::other("undo interrupted")),
        _ => Ok(()),
    };
    let error = undo_with_hook(
        &journal.journal_path,
        SaveQuiescenceRequirement::ConfirmedClosed,
        2,
        &hook,
    )
    .unwrap_err();
    assert!(
        matches!(error, TreeRestoreError::RecoveryRequired { .. }),
        "{error}"
    );
    let recovered = recover_tree_restore(&journal.journal_path, 3).unwrap();
    assert_eq!(recovered.status, TreeRestoreStatus::Undone);
    assert_eq!(env.everything(), before);
}

#[test]
fn a_stale_plan_is_refused_when_the_destination_changed_after_planning() {
    let env = Env::new();
    env.seed(&[("a.sav", "OLD")]);
    let snapshot = env.snapshot("s19", &[("a.sav", "NEW")]);
    let plan = env.plan(&snapshot);
    fs::write(env.dest().join("a.sav"), "CHANGED-AFTER-PLAN").unwrap();
    let error = apply_tree_restore(&snapshot, &plan, &env.options()).unwrap_err();
    assert!(matches!(
        error,
        TreeRestoreError::Refused(ref r) if r == &vec![TreeRestoreRefusal::DestinationChanged]
    ));
    assert_eq!(
        fs::read_to_string(env.dest().join("a.sav")).unwrap(),
        "CHANGED-AFTER-PLAN"
    );
    assert!(env.leftovers().is_empty());
}

#[test]
fn a_tampered_snapshot_is_refused_before_anything_is_written() {
    let env = Env::new();
    env.seed(&[("a.sav", "OLD")]);
    let snapshot = env.snapshot("s20", &[("a.sav", "NEW")]);
    fs::write(snapshot.storage_path.join("files/a.sav"), "TAMPERED").unwrap();
    let before = env.everything();
    assert!(refusals(&env, &snapshot).contains(&TreeRestoreRefusal::SnapshotUnreadable));
    assert_eq!(env.everything(), before);
}

#[test]
fn two_restores_to_different_destinations_are_isolated_and_the_same_one_is_locked() {
    let env = Env::new();
    let snapshot = env.snapshot("s21", &[("a.sav", "1")]);
    let other_dest = env.tmp.path().join("saves/game2");
    let plan_a = env.plan(&snapshot);
    let plan_b = plan_tree_restore(
        &snapshot,
        &other_dest,
        SaveQuiescenceRequirement::ConfirmedClosed,
    );

    // While A is mid-publish, B (other destination) completes, and a second
    // restore of A's destination is refused by the lock.
    let nested = Cell::new(false);
    let second_same = Cell::new(false);
    let hook = |phase: Phase| {
        if phase == Phase::Publish(0) && !nested.replace(true) {
            apply_tree_restore(&snapshot, &plan_b, &env.options()).expect("isolated restore");
            let again = apply_tree_restore(&snapshot, &plan_a, &env.options());
            second_same.set(matches!(
                again,
                Err(TreeRestoreError::Refused(ref r)) if r == &vec![TreeRestoreRefusal::TransactionInProgress]
            ));
        }
        Ok(())
    };
    let a = apply_with_hook(&snapshot, &plan_a, &env.options(), &hook).unwrap();
    assert!(second_same.get(), "same destination must be locked");
    assert_eq!(fs::read_to_string(env.dest().join("a.sav")).unwrap(), "1");
    assert_eq!(fs::read_to_string(other_dest.join("a.sav")).unwrap(), "1");
    assert_ne!(a.journal_path.file_name(), None);
    assert_eq!(
        fs::read_dir(env.tmp.path().join("journal"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn the_journal_is_one_durable_record_that_survives_a_restart() {
    let env = Env::new();
    env.seed(&[("a.sav", "OLD")]);
    let snapshot = env.snapshot("s22", &[("a.sav", "NEW"), ("b/c.sav", "C")]);
    let journal = apply_tree_restore(&snapshot, &env.plan(&snapshot), &env.options()).unwrap();
    // "Restart": nothing but the path is carried over.
    let reloaded = load_tree_restore_journal(&journal.journal_path).unwrap();
    assert_eq!(reloaded, journal);
    assert_eq!(reloaded.snapshot_id, "s22");
    assert_eq!(reloaded.destination_root, env.dest());
    assert_eq!(reloaded.entries.len(), 2);
    assert_eq!(reloaded.created_directories, vec![PathBuf::from("b")]);
    assert_eq!(
        reloaded.entries[0].previous,
        PreviousObject::File {
            size_bytes: 3,
            sha256: reloaded.entries[0].previous.clone().file_sha()
        }
    );
    assert_eq!(
        fs::read_dir(env.tmp.path().join("journal"))
            .unwrap()
            .count(),
        1
    );
    let undone = undo_tree_restore(
        &journal.journal_path,
        SaveQuiescenceRequirement::ConfirmedClosed,
        9,
    )
    .unwrap();
    assert_eq!(undone.finished_unix_seconds, Some(9));
    assert_eq!(fs::read_to_string(env.dest().join("a.sav")).unwrap(), "OLD");
    // Undoing twice is refused rather than repeated.
    assert!(
        undo_tree_restore(
            &journal.journal_path,
            SaveQuiescenceRequirement::ConfirmedClosed,
            9
        )
        .is_err()
    );
}

impl PreviousObject {
    fn file_sha(self) -> String {
        match self {
            PreviousObject::File { sha256, .. } => sha256,
            other => panic!("{other:?}"),
        }
    }
}
