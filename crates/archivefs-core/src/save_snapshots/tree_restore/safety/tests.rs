use super::*;
use crate::save_snapshots::{
    EmulatorUseStatus, SaveLocation, SaveProvenance, SaveSnapshotRequest, create_snapshot,
};
use std::{cell::Cell, os::unix::fs::symlink};
use tempfile::{TempDir, tempdir};

struct Status(Cell<EmulatorQuiescence>);
impl QuiescenceProvider for Status {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        self.0.get()
    }
}
struct Space(u64);
impl RestoreSpaceProvider for Space {
    fn available_bytes(&self, _: &Path) -> io::Result<u64> {
        Ok(self.0)
    }
}
struct Env {
    temp: TempDir,
    snapshot: SaveSnapshot,
    binding: DirectorySaveBinding,
    status: Status,
}
impl Env {
    fn new() -> Self {
        let temp = tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("slot.sav"), b"target save").unwrap();
        let snapshot = create_snapshot(&SaveSnapshotRequest {
            location: SaveLocation {
                path: source,
                emulator: Some("synthetic".into()),
                profile: Some("profile-one".into()),
                artifact_type: SaveArtifactType::SaveDirectory,
                provenance: SaveProvenance::UserSpecified,
            },
            game_identity: Some("identity:exact:one".into()),
            platform: None,
            storage_root: temp.path().join("snapshots"),
            available_space_bytes: None,
            snapshot_id: Some("reviewed".into()),
            now_unix_seconds: Some(100),
            emulator_use: EmulatorUseStatus::NotDetected,
        })
        .unwrap();
        let binding = DirectorySaveBinding {
            game: ResolvedSaveGame::Unique("identity:exact:one".into()),
            emulator: "synthetic".into(),
            profile: "profile-one".into(),
            artifact_type: SaveArtifactType::SaveDirectory,
        };
        Self {
            temp,
            snapshot,
            binding,
            status: Status(Cell::new(EmulatorQuiescence::Closed)),
        }
    }
    fn dest(&self) -> PathBuf {
        self.temp.path().join("destination")
    }
    fn options(&self) -> TreeRestoreOptions {
        TreeRestoreOptions {
            journal_root: self.temp.path().join("history"),
            now_unix_seconds: 200,
        }
    }
    fn preview(&self) -> DirectoryRestorePreview {
        preview_directory_restore(
            &self.snapshot,
            Some(&self.binding),
            &self.dest(),
            &self.status,
            &Space(u64::MAX),
        )
        .unwrap()
    }
    fn apply(&self, p: &DirectoryRestorePreview) -> Result<TreeRestoreJournal, TreeRestoreError> {
        apply_directory_restore(
            &self.snapshot,
            p,
            &self.status,
            &Space(u64::MAX),
            &self.options(),
        )
    }
    fn seed(&self) {
        fs::create_dir_all(self.dest().join("old/empty")).unwrap();
        fs::create_dir_all(self.dest().join("keepdir")).unwrap();
        fs::write(self.dest().join("slot.sav"), b"original").unwrap();
        fs::write(self.dest().join("old/file"), b"removed").unwrap();
        fs::write(self.dest().join("keepdir/extra"), b"removed too").unwrap();
    }
    fn disk_manifest(&self) {
        fs::write(
            self.snapshot.storage_path.join("manifest.json"),
            serde_json::to_vec(&self.snapshot.manifest).unwrap(),
        )
        .unwrap();
    }
}
fn is_refused<T: std::fmt::Debug>(
    result: Result<T, TreeRestoreError>,
    expected: TreeRestoreRefusal,
) {
    match result {
        Err(TreeRestoreError::Refused(r)) => assert!(r.contains(&expected), "{r:?}"),
        other => panic!("expected refusal {expected:?}: {other:?}"),
    }
}

#[test]
fn strict_binding_and_identity_refusals() {
    let e = Env::new();
    assert!(e.preview().ready());
    is_refused(
        preview_directory_restore(&e.snapshot, None, &e.dest(), &e.status, &Space(u64::MAX)),
        TreeRestoreRefusal::MissingBinding,
    );
    for (binding, reason) in [
        (
            DirectorySaveBinding {
                game: ResolvedSaveGame::Unique("other".into()),
                ..e.binding.clone()
            },
            TreeRestoreRefusal::WrongGameIdentity,
        ),
        (
            DirectorySaveBinding {
                game: ResolvedSaveGame::Ambiguous,
                ..e.binding.clone()
            },
            TreeRestoreRefusal::AmbiguousIdentity,
        ),
        (
            DirectorySaveBinding {
                game: ResolvedSaveGame::Unknown,
                ..e.binding.clone()
            },
            TreeRestoreRefusal::MissingBinding,
        ),
        (
            DirectorySaveBinding {
                emulator: "other".into(),
                ..e.binding.clone()
            },
            TreeRestoreRefusal::WrongEmulator,
        ),
        (
            DirectorySaveBinding {
                profile: "other".into(),
                ..e.binding.clone()
            },
            TreeRestoreRefusal::WrongProfile,
        ),
        (
            DirectorySaveBinding {
                profile: "".into(),
                ..e.binding.clone()
            },
            TreeRestoreRefusal::MissingBinding,
        ),
    ] {
        is_refused(
            preview_directory_restore(
                &e.snapshot,
                Some(&binding),
                &e.dest(),
                &e.status,
                &Space(u64::MAX),
            ),
            reason,
        );
    }
    for artifact_type in [
        SaveArtifactType::SaveState,
        SaveArtifactType::MemoryCard,
        SaveArtifactType::Unknown,
        SaveArtifactType::Sram,
    ] {
        let b = DirectorySaveBinding {
            artifact_type,
            ..e.binding.clone()
        };
        is_refused(
            preview_directory_restore(
                &e.snapshot,
                Some(&b),
                &e.dest(),
                &e.status,
                &Space(u64::MAX),
            ),
            TreeRestoreRefusal::WrongArtifactType,
        );
    }
}
#[test]
fn manifest_tamper_after_preview_and_oversize() {
    let e = Env::new();
    let p = e.preview();
    fs::write(e.snapshot.storage_path.join("manifest.json"), b"{}").unwrap();
    is_refused(e.apply(&p), TreeRestoreRefusal::ManifestChanged);
    assert!(!e.dest().exists());
    fs::write(
        e.snapshot.storage_path.join("manifest.json"),
        vec![b' '; MAX_MANIFEST_BYTES as usize + 1],
    )
    .unwrap();
    is_refused(
        preview_directory_restore(
            &e.snapshot,
            Some(&e.binding),
            &e.dest(),
            &e.status,
            &Space(u64::MAX),
        ),
        TreeRestoreRefusal::ManifestTooLarge,
    );
}
#[test]
fn supplied_metadata_tamper_refused() {
    let mut e = Env::new();
    e.snapshot.manifest.snapshot_unix_seconds += 1;
    is_refused(
        preview_directory_restore(
            &e.snapshot,
            Some(&e.binding),
            &e.dest(),
            &e.status,
            &Space(u64::MAX),
        ),
        TreeRestoreRefusal::ManifestChanged,
    );
}
#[test]
fn disjoint_storage_and_aliases() {
    let e = Env::new();
    for d in [
        &e.snapshot.storage_path,
        e.snapshot.storage_path.parent().unwrap(),
        &e.snapshot.storage_path.join("child"),
    ] {
        is_refused(
            preview_directory_restore(
                &e.snapshot,
                Some(&e.binding),
                d,
                &e.status,
                &Space(u64::MAX),
            ),
            TreeRestoreRefusal::SnapshotDestinationOverlap,
        );
    }
    let alias = e.temp.path().join("alias");
    symlink(&e.snapshot.storage_path, &alias).unwrap();
    assert!(
        preview_directory_restore(
            &e.snapshot,
            Some(&e.binding),
            &alias,
            &e.status,
            &Space(u64::MAX)
        )
        .is_err()
    );
}
#[test]
fn insufficient_space_preview_apply_and_opaque_confirmation() {
    let e = Env::new();
    let p = preview_directory_restore(
        &e.snapshot,
        Some(&e.binding),
        &e.dest(),
        &e.status,
        &Space(0),
    )
    .unwrap();
    assert!(!p.ready());
    assert!(
        p.refusals
            .iter()
            .any(|r| matches!(r, TreeRestoreRefusal::InsufficientSpace { .. }))
    );
    let mut p = e.preview();
    p.required_space_bytes = 0; // display fields cannot weaken reviewed requirements
    assert!(
        matches!(apply_directory_restore(&e.snapshot,&p,&e.status,&Space(0),&e.options()),Err(TreeRestoreError::Refused(r)) if matches!(r[0],TreeRestoreRefusal::InsufficientSpace{..}))
    );
    assert!(!e.dest().exists());
    assert!(!e.options().journal_root.exists());
}
#[test]
fn destination_content_and_empty_directory_changes_refused() {
    for add_dir in [false, true] {
        let e = Env::new();
        e.seed();
        let p = e.preview();
        if add_dir {
            fs::create_dir(e.dest().join("new-empty")).unwrap();
        } else {
            fs::write(e.dest().join("old/file"), b"newer user save").unwrap();
        }
        is_refused(e.apply(&p), TreeRestoreRefusal::DestinationChanged);
        assert!(e.dest().join("old/file").exists());
    }
}
#[test]
fn quiescence_running_unknown_and_transition() {
    let e = Env::new();
    let p = e.preview();
    for (state, reason) in [
        (
            EmulatorQuiescence::Running,
            TreeRestoreRefusal::EmulatorRunning,
        ),
        (
            EmulatorQuiescence::Unknown,
            TreeRestoreRefusal::EmulatorStateUnknown,
        ),
    ] {
        e.status.0.set(state);
        let denied = e.preview();
        assert!(!denied.ready());
        assert!(denied.refusals.contains(&reason));
        is_refused(e.apply(&p), reason);
        assert!(!e.dest().exists());
    }
    assert_eq!(
        UnavailableQuiescence.observe(&e.binding),
        EmulatorQuiescence::Unknown
    );
}
#[test]
fn happy_replace_preserves_removals_and_undo_verifies_preimage() {
    let e = Env::new();
    e.seed();
    let before = fingerprint(&e.dest()).unwrap();
    let p = e.preview();
    assert!(p.ready());
    assert_eq!(p.files_to_remove.len(), 2);
    assert_eq!(p.files_to_replace, vec![PathBuf::from("slot.sav")]);
    let j = e.apply(&p).unwrap();
    assert_eq!(j.status, TreeRestoreStatus::Published);
    assert!(!e.dest().join("old").exists());
    assert!(!e.dest().join("keepdir").exists());
    let loaded = load_tree_restore_journal(&j.journal_path).unwrap();
    assert_eq!(loaded, j);
    let undone = undo_directory_restore(&j.journal_path, &e.status, 300).unwrap();
    assert_eq!(undone.status, TreeRestoreStatus::Undone);
    assert_eq!(fingerprint(&e.dest()).unwrap(), before);
}
#[test]
fn missing_and_empty_destination_restore_and_undo() {
    for exists in [true, false] {
        let e = Env::new();
        if exists {
            fs::create_dir(e.dest()).unwrap();
        }
        let before = fingerprint(&e.dest()).unwrap();
        let j = e.apply(&e.preview()).unwrap();
        assert_eq!(fs::read(e.dest().join("slot.sav")).unwrap(), b"target save");
        undo_directory_restore(&j.journal_path, &e.status, 300).unwrap();
        assert_eq!(fingerprint(&e.dest()).unwrap(), before);
    }
}
#[test]
fn undo_modified_output_and_quiescence_refusal_record_history() {
    let e = Env::new();
    e.seed();
    let j = e.apply(&e.preview()).unwrap();
    for state in [EmulatorQuiescence::Running, EmulatorQuiescence::Unknown] {
        e.status.0.set(state);
        assert!(undo_directory_restore(&j.journal_path, &e.status, 300).is_err());
        assert!(
            load_tree_restore_journal(&j.journal_path)
                .unwrap()
                .detail
                .unwrap()
                .contains("refused")
        );
    }
    e.status.0.set(EmulatorQuiescence::Closed);
    fs::create_dir(e.dest().join("user-empty")).unwrap();
    assert!(matches!(
        undo_directory_restore(&j.journal_path, &e.status, 300),
        Err(TreeRestoreError::UndoConflict { .. })
    ));
    assert!(e.dest().join("user-empty").exists());
    assert!(
        load_tree_restore_journal(&j.journal_path)
            .unwrap()
            .detail
            .unwrap()
            .contains("undo refused")
    );
}
#[test]
fn unsafe_source_destination_traversal_and_special_refused() {
    let mut e = Env::new();
    e.seed();
    symlink("slot.sav", e.dest().join("unused-link")).unwrap();
    assert!(!e.preview().ready());
    fs::remove_file(e.dest().join("unused-link")).unwrap();
    let fifo = e.dest().join("fifo");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(!e.preview().ready());
    fs::remove_file(fifo).unwrap();
    e.snapshot.manifest.artifacts[0].relative_path = PathBuf::from("../escape");
    e.disk_manifest();
    is_refused(
        preview_directory_restore(
            &e.snapshot,
            Some(&e.binding),
            &e.dest(),
            &e.status,
            &Space(u64::MAX),
        ),
        TreeRestoreRefusal::UnsafeRelativePath(PathBuf::from("../escape")),
    );
}
#[test]
fn snapshot_symlink_ancestor_and_content_tamper_refused() {
    let e = Env::new();
    let files = e.snapshot.storage_path.join("files");
    fs::rename(&files, files.with_extension("old")).unwrap();
    symlink(files.with_extension("old"), &files).unwrap();
    assert!(
        preview_directory_restore(
            &e.snapshot,
            Some(&e.binding),
            &e.dest(),
            &e.status,
            &Space(u64::MAX)
        )
        .is_err()
    );
    fs::remove_file(&files).unwrap();
    fs::rename(files.with_extension("old"), &files).unwrap();
    fs::write(files.join("slot.sav"), b"tampered").unwrap();
    assert!(!e.preview().ready());
}
#[test]
fn deterministic_stage_publish_and_verification_failures_roll_back() {
    for phase in [
        Phase::Stage(0),
        Phase::Publish(0),
        Phase::Publish(1),
        Phase::Publish(2),
        Phase::Verify,
    ] {
        let e = Env::new();
        e.seed();
        let original = fingerprint(&e.dest()).unwrap();
        let hook = |p| {
            if p == phase {
                Err(io::Error::other("deterministic fault"))
            } else {
                Ok(())
            }
        };
        assert!(
            apply_reviewed(
                &e.snapshot,
                &e.preview(),
                &e.status,
                &Space(u64::MAX),
                &e.options(),
                &hook
            )
            .is_err()
        );
        assert_eq!(fingerprint(&e.dest()).unwrap(), original, "{phase:?}");
    }
}
#[test]
fn receipt_failure_recovery_restores_exact_original() {
    let e = Env::new();
    e.seed();
    let original = fingerprint(&e.dest()).unwrap();
    let hook = |p| {
        if p == Phase::Receipt {
            Err(io::Error::other("receipt unavailable"))
        } else {
            Ok(())
        }
    };
    let path = match apply_reviewed(
        &e.snapshot,
        &e.preview(),
        &e.status,
        &Space(u64::MAX),
        &e.options(),
        &hook,
    ) {
        Err(TreeRestoreError::RecoveryRequired { journal_path, .. }) => journal_path,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        load_tree_restore_journal(&path).unwrap().status,
        TreeRestoreStatus::Publishing
    );
    let j = recover_directory_restore(&path, &e.status, 300).unwrap();
    assert_eq!(j.status, TreeRestoreStatus::RolledBack);
    assert_eq!(fingerprint(&e.dest()).unwrap(), original);
    assert_eq!(
        recover_directory_restore(&path, &e.status, 301)
            .unwrap()
            .status,
        TreeRestoreStatus::RolledBack
    );
}
#[test]
fn interrupted_rollback_recovery_is_quiescence_gated() {
    let e = Env::new();
    e.seed();
    let original = fingerprint(&e.dest()).unwrap();
    let hook = |p| {
        if p == Phase::Verify || p == Phase::Rollback(0) {
            Err(io::Error::other("crash seam"))
        } else {
            Ok(())
        }
    };
    let path = match apply_reviewed(
        &e.snapshot,
        &e.preview(),
        &e.status,
        &Space(u64::MAX),
        &e.options(),
        &hook,
    ) {
        Err(TreeRestoreError::RecoveryRequired { journal_path, .. }) => journal_path,
        other => panic!("{other:?}"),
    };
    e.status.0.set(EmulatorQuiescence::Running);
    assert!(recover_directory_restore(&path, &e.status, 300).is_err());
    e.status.0.set(EmulatorQuiescence::Closed);
    recover_directory_restore(&path, &e.status, 301).unwrap();
    assert_eq!(fingerprint(&e.dest()).unwrap(), original);
}
#[test]
fn replace_inside_kept_directory_and_repeated_restore_determinism() {
    let mut e = Env::new();
    let files = e.snapshot.storage_path.join("files");
    fs::create_dir(files.join("sub")).unwrap();
    fs::rename(files.join("slot.sav"), files.join("sub/slot.sav")).unwrap();
    e.snapshot.manifest.artifacts[0].relative_path = "sub/slot.sav".into();
    e.disk_manifest();
    fs::create_dir_all(e.dest().join("sub/old-empty")).unwrap();
    fs::write(e.dest().join("sub/extra"), b"old").unwrap();
    let p = e.preview();
    assert_eq!(p.files_to_remove, vec![PathBuf::from("sub/extra")]);
    e.apply(&p).unwrap();
    assert!(!e.dest().join("sub/extra").exists());
    assert!(!e.dest().join("sub/old-empty").exists());
    let target = fingerprint(&e.dest()).unwrap();
    let p = e.preview();
    e.apply(&p).unwrap();
    assert_eq!(fingerprint(&e.dest()).unwrap(), target);
}
#[test]
fn large_synthetic_tree_uses_streaming_copy_and_verifies() {
    let mut e = Env::new();
    let files = e.snapshot.storage_path.join("files");
    let source = files.join("slot.sav");
    let mut file = File::create(&source).unwrap();
    let block = [0x5au8; 64 * 1024];
    for _ in 0..512 {
        file.write_all(&block).unwrap();
    }
    file.sync_all().unwrap();
    let (size, sha, _) = hash_and_metadata(&source).unwrap();
    e.snapshot.manifest.artifacts[0].size_bytes = size;
    e.snapshot.manifest.artifacts[0].sha256 = sha;
    e.snapshot.manifest.source_size_bytes = size;
    e.disk_manifest();
    let p = e.preview();
    assert_eq!(p.restored_bytes, 32 * 1024 * 1024);
    let j = e.apply(&p).unwrap();
    verify_published(&j).unwrap();
    undo_directory_restore(&j.journal_path, &e.status, 300).unwrap();
    assert!(!e.dest().exists());
}

#[test]
fn staging_quiescence_transition_preserves_destination() {
    let e = Env::new();
    e.seed();
    let original = fingerprint(&e.dest()).unwrap();
    let p = e.preview();
    let hook = |phase| {
        if phase == Phase::BeforePublish {
            e.status.0.set(EmulatorQuiescence::Running);
        }
        Ok(())
    };
    assert!(
        apply_reviewed(
            &e.snapshot,
            &p,
            &e.status,
            &Space(u64::MAX),
            &e.options(),
            &hook
        )
        .is_err()
    );
    assert_eq!(fingerprint(&e.dest()).unwrap(), original);
}
#[test]
fn staging_destination_change_never_runs_rollback_over_user_data() {
    let e = Env::new();
    e.seed();
    let p = e.preview();
    let hook = |phase| {
        if phase == Phase::BeforePublish {
            fs::write(e.dest().join("slot.sav"), b"target save")?;
        }
        Ok(())
    };
    assert!(
        apply_reviewed(
            &e.snapshot,
            &p,
            &e.status,
            &Space(u64::MAX),
            &e.options(),
            &hook
        )
        .is_err()
    );
    assert_eq!(fs::read(e.dest().join("slot.sav")).unwrap(), b"target save");
    assert!(e.dest().join("old/file").exists());
}
#[test]
fn journal_failure_before_publish_recovers_without_touching_newer_saves() {
    for status in [TreeRestoreStatus::Staged, TreeRestoreStatus::Publishing] {
        let e = Env::new();
        e.seed();
        let hook = |p| {
            if p == Phase::Journal(status) {
                Err(io::Error::other("journal failure"))
            } else {
                Ok(())
            }
        };
        let path = match apply_reviewed(
            &e.snapshot,
            &e.preview(),
            &e.status,
            &Space(u64::MAX),
            &e.options(),
            &hook,
        ) {
            Err(TreeRestoreError::RecoveryRequired { journal_path, .. }) => journal_path,
            other => panic!("{other:?}"),
        };
        fs::write(e.dest().join("slot.sav"), b"target save").unwrap();
        recover_directory_restore(&path, &e.status, 300).unwrap();
        assert_eq!(fs::read(e.dest().join("slot.sav")).unwrap(), b"target save");
        assert!(e.dest().join("old/file").exists());
    }
}
#[test]
fn replacement_failure_rolls_back_after_preservation() {
    let e = Env::new();
    e.seed();
    let before = fingerprint(&e.dest()).unwrap();
    let hook = |p| {
        if p == Phase::Replace(0) {
            Err(io::Error::other("replacement failed"))
        } else {
            Ok(())
        }
    };
    assert!(matches!(
        apply_reviewed(
            &e.snapshot,
            &e.preview(),
            &e.status,
            &Space(u64::MAX),
            &e.options(),
            &hook
        ),
        Err(TreeRestoreError::RolledBack { .. })
    ));
    assert_eq!(fingerprint(&e.dest()).unwrap(), before);
}
#[test]
fn undo_refuses_tampered_preimage_and_rechecks_provider() {
    let e = Env::new();
    e.seed();
    let j = e.apply(&e.preview()).unwrap();
    fs::write(
        j.work_dir.join("preserved/removed/old/file"),
        b"tampered backup",
    )
    .unwrap();
    assert!(matches!(
        undo_directory_restore(&j.journal_path, &e.status, 300),
        Err(TreeRestoreError::UndoConflict { .. })
    ));
    struct Transition(Cell<u8>);
    impl QuiescenceProvider for Transition {
        fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
            let n = self.0.get();
            self.0.set(n + 1);
            if n == 0 {
                EmulatorQuiescence::Closed
            } else {
                EmulatorQuiescence::Running
            }
        }
    }
    let e = Env::new();
    e.seed();
    let j = e.apply(&e.preview()).unwrap();
    let post = fingerprint(&e.dest()).unwrap();
    assert!(undo_directory_restore(&j.journal_path, &Transition(Cell::new(0)), 300).is_err());
    assert_eq!(fingerprint(&e.dest()).unwrap(), post);
}
#[test]
fn legacy_journal_deserializes_but_cannot_bypass_binding() {
    let e = Env::new();
    let j = e.apply(&e.preview()).unwrap();
    let mut v = serde_json::to_value(&j).unwrap();
    v.as_object_mut().unwrap().remove("safety");
    fs::write(&j.journal_path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(
        load_tree_restore_journal(&j.journal_path)
            .unwrap()
            .safety
            .is_none()
    );
    is_refused(
        undo_directory_restore(&j.journal_path, &e.status, 300),
        TreeRestoreRefusal::MissingBinding,
    );
}
#[test]
fn preview_is_read_only_and_space_is_actual_same_filesystem_observation() {
    let e = Env::new();
    let before = fingerprint(e.temp.path()).unwrap();
    let p = e.preview();
    let after = fingerprint(e.temp.path()).unwrap();
    assert_eq!(before, after);
    assert!(p.ready());
    assert!(FilesystemSpace.available_bytes(e.temp.path()).unwrap() > 0);
}

#[test]
fn abrupt_interruption_during_partial_publication_recovers_from_disk_journal() {
    let e = Env::new();
    e.seed();
    let before = fingerprint(&e.dest()).unwrap();
    let hook = |p| {
        if p == Phase::Publish(1) {
            panic!("simulated process termination");
        }
        Ok(())
    };
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        apply_reviewed(
            &e.snapshot,
            &e.preview(),
            &e.status,
            &Space(u64::MAX),
            &e.options(),
            &hook,
        )
    }));
    assert!(interrupted.is_err());
    let path = fs::read_dir(e.options().journal_root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|s| s == "json"))
        .unwrap();
    assert_eq!(
        load_tree_restore_journal(&path).unwrap().status,
        TreeRestoreStatus::Publishing
    );
    assert_ne!(fingerprint(&e.dest()).unwrap(), before);
    recover_directory_restore(&path, &e.status, 300).unwrap();
    assert_eq!(fingerprint(&e.dest()).unwrap(), before);
}
#[test]
fn destination_root_replacement_and_manifest_byte_change_refused() {
    let e = Env::new();
    e.seed();
    let p = e.preview();
    fs::rename(e.dest(), e.temp.path().join("moved")).unwrap();
    e.seed();
    is_refused(e.apply(&p), TreeRestoreRefusal::DestinationChanged);
    let e = Env::new();
    let p = e.preview();
    let path = e.snapshot.storage_path.join("manifest.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
    is_refused(e.apply(&p), TreeRestoreRefusal::ManifestChanged);
}
#[test]
fn snapshot_artifact_type_and_absent_profile_refused() {
    let mut e = Env::new();
    e.snapshot.manifest.artifact_type = SaveArtifactType::MemoryCard;
    e.disk_manifest();
    is_refused(
        preview_directory_restore(
            &e.snapshot,
            Some(&e.binding),
            &e.dest(),
            &e.status,
            &Space(u64::MAX),
        ),
        TreeRestoreRefusal::WrongArtifactType,
    );
    e.snapshot.manifest.artifact_type = SaveArtifactType::SaveDirectory;
    e.snapshot.manifest.emulator_profile = None;
    e.disk_manifest();
    is_refused(
        preview_directory_restore(
            &e.snapshot,
            Some(&e.binding),
            &e.dest(),
            &e.status,
            &Space(u64::MAX),
        ),
        TreeRestoreRefusal::WrongProfile,
    );
}

#[test]
fn resolved_profile_seam_requires_selection_without_discovery() {
    use crate::patch_manager::{
        EmulatorDestinationDirectories, EmulatorInstallationType, EmulatorProfileConfidence,
    };
    let e = Env::new();
    let mut p = ResolvedEmulatorProfile {
        emulator_executable: None,
        installation_type: EmulatorInstallationType::NativeSystem,
        configuration_root: e.temp.path().join("resolved-profile"),
        data_user_root: e.temp.path().join("user"),
        active_explicit_profile: None,
        destinations: EmulatorDestinationDirectories::default(),
        discovery_evidence: vec![],
        confidence: EmulatorProfileConfidence::KnownPath,
        priority: 1,
        writable: true,
    };
    is_refused(
        DirectorySaveBinding::from_resolved_profile(
            e.binding.game.clone(),
            e.binding.emulator.clone(),
            &p,
        ),
        TreeRestoreRefusal::MissingBinding,
    );
    p.confidence = EmulatorProfileConfidence::UserConfirmed;
    let b = DirectorySaveBinding::from_resolved_profile(
        e.binding.game.clone(),
        e.binding.emulator.clone(),
        &p,
    )
    .unwrap();
    assert_eq!(b.profile, p.configuration_root.to_str().unwrap());
    assert_eq!(b.artifact_type, SaveArtifactType::SaveDirectory);
    assert!(!p.configuration_root.exists());
}
#[test]
fn many_file_synthetic_tree_restore_and_verified_undo() {
    let mut e = Env::new();
    let files = e.snapshot.storage_path.join("files");
    let block = vec![0x34u8; 256 * 1024];
    for i in 0..128 {
        let relative = PathBuf::from(format!("profile-{}/slot-{i}.sav", i % 8));
        let path = files.join(&relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &block).unwrap();
        let (size_bytes, sha256, modified_unix_seconds) = hash_and_metadata(&path).unwrap();
        e.snapshot.manifest.source_size_bytes += size_bytes;
        e.snapshot
            .manifest
            .artifacts
            .push(crate::save_snapshots::SaveArtifact {
                relative_path: relative,
                size_bytes,
                sha256,
                modified_unix_seconds,
            });
    }
    e.disk_manifest();
    let p = e.preview();
    assert_eq!(p.files_to_create.len(), 129);
    assert!(p.restored_bytes > 32 * 1024 * 1024);
    let j = e.apply(&p).unwrap();
    verify_published(&j).unwrap();
    undo_directory_restore(&j.journal_path, &e.status, 300).unwrap();
    assert!(!e.dest().exists());
}

#[test]
fn completed_receipt_recovery_cleans_only_its_own_stale_lock() {
    let e = Env::new();
    let j = e.apply(&e.preview()).unwrap();
    fs::write(&j.lock_path, &j.transaction_id).unwrap();
    assert_eq!(
        recover_directory_restore(&j.journal_path, &e.status, 300)
            .unwrap()
            .status,
        TreeRestoreStatus::Published
    );
    assert!(!j.lock_path.exists());
    fs::write(&j.lock_path, "other-operation").unwrap();
    recover_directory_restore(&j.journal_path, &e.status, 301).unwrap();
    assert_eq!(fs::read_to_string(&j.lock_path).unwrap(), "other-operation");
}

#[test]
fn maximum_depth_save_paths_allow_preservation_and_undo() {
    let mut e = Env::new();
    let mut relative = PathBuf::new();
    for _ in 0..MAX_SNAPSHOT_DEPTH - 1 {
        relative.push("level");
    }
    relative.push("slot.sav");
    let files = e.snapshot.storage_path.join("files");
    let target = files.join(&relative);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::rename(files.join("slot.sav"), &target).unwrap();
    e.snapshot.manifest.artifacts[0].relative_path = relative.clone();
    e.disk_manifest();
    let destination = e.dest().join(&relative);
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(&destination, b"original deepest save").unwrap();
    let before = fingerprint(&e.dest()).unwrap();
    let j = e.apply(&e.preview()).unwrap();
    undo_directory_restore(&j.journal_path, &e.status, 300).unwrap();
    assert_eq!(fingerprint(&e.dest()).unwrap(), before);
}

#[test]
fn target_directory_fanout_bound_is_checked_before_publication() {
    let e = Env::new();
    let mut plan = plan_tree_restore(
        &e.snapshot,
        &e.dest(),
        SaveQuiescenceRequirement::ConfirmedClosed,
    );
    plan.entries.clear();
    for i in 0..2048 {
        plan.entries.push(TreeRestoreEntry {
            relative_path: PathBuf::from(format!("{i}/a/b/c/d/e/f/g/h/i/file")),
            size_bytes: 0,
            sha256: "unused".into(),
            previous: PreviousObject::Absent,
            action: EntryAction::Create,
        });
    }
    augment_plan(&mut plan, e.binding.clone());
    assert!(plan.refusals.contains(&TreeRestoreRefusal::TooManyEntries));
    assert!(!e.dest().exists());
}

#[test]
fn recovery_refuses_a_lock_owned_by_another_transaction() {
    let e = Env::new();
    e.seed();
    let before = fingerprint(&e.dest()).unwrap();
    let hook = |p| {
        if p == Phase::Receipt {
            Err(io::Error::other("lost receipt"))
        } else {
            Ok(())
        }
    };
    let path = match apply_reviewed(
        &e.snapshot,
        &e.preview(),
        &e.status,
        &Space(u64::MAX),
        &e.options(),
        &hook,
    ) {
        Err(TreeRestoreError::RecoveryRequired { journal_path, .. }) => journal_path,
        other => panic!("{other:?}"),
    };
    let j = load_tree_restore_journal(&path).unwrap();
    let output = fingerprint(&e.dest()).unwrap();
    fs::write(&j.lock_path, "other transaction").unwrap();
    is_refused(
        recover_directory_restore(&path, &e.status, 300),
        TreeRestoreRefusal::TransactionInProgress,
    );
    assert_eq!(fingerprint(&e.dest()).unwrap(), output);
    fs::write(&j.lock_path, &j.transaction_id).unwrap();
    recover_directory_restore(&path, &e.status, 301).unwrap();
    assert_eq!(fingerprint(&e.dest()).unwrap(), before);
}
