use super::*;
use archivefs_core::{
    persistent_state_inventory::StateEmulator,
    save_snapshots::{
        EmulatorUseStatus, SaveLocation, SaveProvenance, SaveSnapshotRequest, create_snapshot,
        tree_restore::safety::{DirectorySaveBinding, EmulatorQuiescence},
    },
};
use tempfile::tempdir;

struct Observed(EmulatorQuiescence);
impl QuiescenceProvider for Observed {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        self.0
    }
}

struct World {
    dir: tempfile::TempDir,
    target: SnapshotTarget,
    snapshot: SaveSnapshot,
}

fn world() -> World {
    let dir = tempdir().unwrap();
    let save = dir.path().join("saves/game.srm");
    fs::create_dir_all(save.parent().unwrap()).unwrap();
    fs::write(&save, b"good").unwrap();
    let location = SaveLocation {
        path: save.clone(),
        emulator: Some("RetroArch".into()),
        profile: Some("p".into()),
        artifact_type: SaveArtifactType::Sram,
        provenance: SaveProvenance::EmulatorProfile,
    };
    let snapshot = create_snapshot(&SaveSnapshotRequest {
        location: location.clone(),
        game_identity: Some("LooseRomSha256:abc".into()),
        platform: Some("Game Boy".into()),
        storage_root: dir.path().join("snapshots"),
        available_space_bytes: Some(1 << 20),
        snapshot_id: Some("s1".into()),
        now_unix_seconds: Some(100),
        emulator_use: EmulatorUseStatus::NotDetected,
    })
    .unwrap();
    fs::write(&save, b"later progress").unwrap();
    World {
        target: SnapshotTarget {
            location,
            emulator: StateEmulator::RetroArch,
            game_identity: "LooseRomSha256:abc".into(),
            platform: "Game Boy".into(),
        },
        snapshot,
        dir,
    }
}

fn dirs(w: &World) -> Dirs {
    Dirs {
        backup_root: w.dir.path().join("backups"),
        receipt_root: w.dir.path().join("receipts"),
    }
}

fn prepared(w: &World, state: EmulatorQuiescence) -> Result<Box<Preview>, String> {
    match run_with(
        Job::Prepare {
            snapshot: Box::new(w.snapshot.clone()),
            target: Box::new(w.target.clone()),
        },
        &Observed(state),
    ) {
        Outcome::Prepared(result) => result,
        _ => unreachable!(),
    }
}

#[test]
fn preview_describes_the_change_and_changes_nothing() {
    let w = world();
    let preview = prepared(&w, EmulatorQuiescence::Closed).unwrap();
    assert!(preview.preflight.ready());
    assert!(!preview.identical);
    assert_eq!(preview.current_bytes, Some(14));
    assert_eq!(preview.snapshot_bytes, 4);
    assert_eq!(
        fs::read(&w.target.location.path).unwrap(),
        b"later progress"
    );
    assert!(!dirs(&w).backup_root.exists());
}

#[test]
fn preview_shows_why_it_is_blocked_while_the_emulator_runs() {
    let w = world();
    let preview = prepared(&w, EmulatorQuiescence::Running).unwrap();
    assert!(!preview.preflight.ready());
    assert!(!preview.preflight.refusals.is_empty());
}

#[test]
fn preview_refuses_a_snapshot_of_another_game_or_a_damaged_snapshot() {
    let mut w = world();
    w.target.game_identity = "LooseRomSha256:other".into();
    assert!(
        prepared(&w, EmulatorQuiescence::Closed)
            .unwrap_err()
            .contains("not recorded")
    );
    let w = world();
    fs::write(w.snapshot.storage_path.join("files/game.srm"), b"tampered").unwrap();
    assert!(
        prepared(&w, EmulatorQuiescence::Closed)
            .unwrap_err()
            .contains("checksums")
    );
    let mut w = world();
    w.target.location.artifact_type = SaveArtifactType::Other;
    assert!(prepared(&w, EmulatorQuiescence::Closed).is_err());
}

#[test]
fn confirmed_restore_then_undo_round_trips_and_is_found_after_restart() {
    let w = world();
    let preview = prepared(&w, EmulatorQuiescence::Closed).unwrap();
    let Outcome::Applied { result, .. } = run_with(
        Job::Apply {
            preview,
            dirs: dirs(&w),
        },
        &Observed(EmulatorQuiescence::Closed),
    ) else {
        unreachable!()
    };
    let (receipt, opened) = result.unwrap();
    assert!(!opened);
    let save = &w.target.location.path;
    assert_eq!(fs::read(save).unwrap(), b"good");
    // A fresh session finds the Undo from the journal alone.
    let found = undoable_receipt(&dirs(&w).receipt_root, &dirs(&w).backup_root, save).unwrap();
    assert_eq!(found.transaction_id, receipt.transaction_id);
    let Outcome::Undone { result, .. } = run_with(
        Job::Undo {
            receipt: found,
            target: Box::new(w.target.clone()),
        },
        &Observed(EmulatorQuiescence::Closed),
    ) else {
        unreachable!()
    };
    assert!(!result.unwrap());
    assert_eq!(fs::read(save).unwrap(), b"later progress");
}

#[test]
fn apply_rechecks_closure_and_writes_nothing_when_the_emulator_started() {
    let w = world();
    let preview = prepared(&w, EmulatorQuiescence::Closed).unwrap();
    let Outcome::Applied { result, .. } = run_with(
        Job::Apply {
            preview,
            dirs: dirs(&w),
        },
        &Observed(EmulatorQuiescence::Running),
    ) else {
        unreachable!()
    };
    assert!(result.unwrap_err().contains("Nothing was changed"));
    assert_eq!(
        fs::read(&w.target.location.path).unwrap(),
        b"later progress"
    );
}

#[test]
fn undo_is_not_offered_once_the_save_has_changed_again() {
    let w = world();
    let preview = prepared(&w, EmulatorQuiescence::Closed).unwrap();
    let _ = run_with(
        Job::Apply {
            preview,
            dirs: dirs(&w),
        },
        &Observed(EmulatorQuiescence::Closed),
    );
    fs::write(&w.target.location.path, b"played again").unwrap();
    assert!(
        undoable_receipt(
            &dirs(&w).receipt_root,
            &dirs(&w).backup_root,
            &w.target.location.path
        )
        .is_none()
    );
}

#[test]
fn an_unconfirmable_preview_cannot_be_applied() {
    let w = world();
    let preview = prepared(&w, EmulatorQuiescence::Unknown).unwrap();
    let Outcome::Applied { result, .. } = run_with(
        Job::Apply {
            preview,
            dirs: dirs(&w),
        },
        &Observed(EmulatorQuiescence::Closed),
    ) else {
        unreachable!()
    };
    assert!(result.is_err());
    assert_eq!(
        fs::read(&w.target.location.path).unwrap(),
        b"later progress"
    );
}

#[test]
fn an_orphan_receipt_does_not_hide_the_real_undo_and_a_forged_one_is_ignored() {
    let w = world();
    let preview = prepared(&w, EmulatorQuiescence::Closed).unwrap();
    let _ = run_with(
        Job::Apply {
            preview,
            dirs: dirs(&w),
        },
        &Observed(EmulatorQuiescence::Closed),
    );
    let d = dirs(&w);
    let real = fs::read_dir(&d.receipt_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    // A newer receipt for a restore that never published.
    let mut orphan: serde_json::Value = serde_json::from_slice(&fs::read(&real).unwrap()).unwrap();
    orphan["transaction_id"] = "save-restore-999-orphan".into();
    orphan["applied_unix_seconds"] = 999.into();
    orphan["post_restore_actual_sha256"] = "0".repeat(64).into();
    fs::write(
        d.receipt_root.join("save-restore-999-orphan.json"),
        orphan.to_string(),
    )
    .unwrap();
    // A forged receipt aimed at a backup outside the backup root.
    let mut forged = orphan.clone();
    forged["transaction_id"] = "save-restore-998-forged".into();
    forged["applied_unix_seconds"] = 998.into();
    forged["backup_snapshot_path"] = w
        .dir
        .path()
        .join("snapshots")
        .to_string_lossy()
        .to_string()
        .into();
    fs::write(
        d.receipt_root.join("save-restore-998-forged.json"),
        forged.to_string(),
    )
    .unwrap();
    let found = undoable_receipt(&d.receipt_root, &d.backup_root, &w.target.location.path).unwrap();
    assert_eq!(
        found.transaction_id,
        real.file_stem().unwrap().to_str().unwrap()
    );
    assert!(
        archivefs_core::save_snapshots::list_single_file_restore_receipts(
            &d.receipt_root,
            &d.backup_root
        )
        .iter()
        .all(|receipt| !receipt.transaction_id.contains("forged"))
    );
}
