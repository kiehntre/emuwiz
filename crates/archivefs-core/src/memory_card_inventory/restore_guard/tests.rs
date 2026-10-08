//! Guarded PS2 PSU restore tests. Every card, PSU, backup and journal is a
//! disposable synthetic file in a temporary directory; no real memory card,
//! save or emulator profile is ever read or written.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::os::unix::fs::PermissionsExt;

use super::super::tests::{ps2_fixture, ps2_inventory_fixture};
use super::*;

const CLUSTER: usize = 1024;

struct Fixed(EmulatorQuiescence);
impl QuiescenceProvider for Fixed {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        self.0
    }
}

/// Reports each queued state once, then repeats the last one.
struct Sequence(RefCell<VecDeque<EmulatorQuiescence>>);
impl Sequence {
    fn new(states: &[EmulatorQuiescence]) -> Self {
        Self(RefCell::new(states.iter().copied().collect()))
    }
}
impl QuiescenceProvider for Sequence {
    fn observe(&self, _: &DirectorySaveBinding) -> EmulatorQuiescence {
        let mut queue = self.0.borrow_mut();
        if queue.len() > 1 {
            queue.pop_front().unwrap()
        } else {
            *queue.front().unwrap()
        }
    }
}

const CLOSED: Fixed = Fixed(EmulatorQuiescence::Closed);

struct World {
    dir: tempfile::TempDir,
    card: PathBuf,
    psu: PathBuf,
    backup: PathBuf,
    journals: PathBuf,
    original: Vec<u8>,
}

impl World {
    fn new() -> Self {
        Self::with_card(ps2_inventory_fixture())
    }

    fn with_card(original: Vec<u8>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let card = dir.path().join("Mcd001.ps2");
        fs::write(&card, &original).unwrap();
        let journals = dir.path().join("journals");
        fs::create_dir(&journals).unwrap();
        let inventory = inspect_memory_card(&card).unwrap();
        let psu = dir.path().join("save.psu");
        if let Some(inv) = &inventory.ps2_inventory
            && let Some(save) = inv.save_directories.first()
        {
            let export = plan_ps2_psu_export(&inventory, save, &psu).unwrap();
            apply_ps2_psu_export(&export).unwrap();
        }
        let backup = dir.path().join("before-restore.card");
        Self {
            dir,
            card,
            psu,
            backup,
            journals,
            original,
        }
    }

    fn plan(&self) -> Ps2PsuRestorePlan {
        let card = inspect_memory_card(&self.card).unwrap();
        plan_ps2_psu_restore(&card, &self.psu, &self.backup, true).unwrap()
    }

    fn options(&self) -> Ps2RestoreGuardOptions {
        Ps2RestoreGuardOptions {
            journal_dir: self.journals.clone(),
            unix_seconds: 1_700_000_000,
        }
    }

    fn apply(&self) -> Result<Ps2GuardedRestore, Ps2PsuRestoreError> {
        apply_ps2_psu_restore_guarded(&self.plan(), &CLOSED, &self.options())
    }

    fn apply_hooked(
        &self,
        provider: &dyn QuiescenceProvider,
        hook: &dyn Fn(Ps2RestoreStep) -> std::io::Result<()>,
    ) -> Result<Ps2GuardedRestore, Ps2PsuRestoreError> {
        apply_with_hook(&self.plan(), provider, &self.options(), hook)
    }

    fn card_bytes(&self) -> Vec<u8> {
        fs::read(&self.card).unwrap()
    }

    fn journal_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = fs::read_dir(&self.journals)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        files.sort();
        files
    }

    fn only_journal(&self) -> Ps2RestoreJournal {
        let files = self.journal_files();
        assert_eq!(files.len(), 1, "{files:?}");
        load_ps2_restore_journal(&files[0]).unwrap()
    }

    fn stray_temp_files(&self) -> Vec<String> {
        fs::read_dir(self.dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(STAGE_PREFIX) || n.contains(".tmp"))
            .collect()
    }
}

fn crash(step: Ps2RestoreStep, at: Ps2RestoreStep) -> std::io::Result<()> {
    if step == at {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            SIMULATED_CRASH,
        ))
    } else {
        Ok(())
    }
}

fn fail_at(step: Ps2RestoreStep, at: Ps2RestoreStep) -> std::io::Result<()> {
    if step == at {
        Err(std::io::Error::other("injected failure"))
    } else {
        Ok(())
    }
}

fn flip(path: &Path, offset: usize) {
    let mut bytes = fs::read(path).unwrap();
    bytes[offset] ^= 0xff;
    fs::write(path, bytes).unwrap();
}

// ---------------------------------------------------------------- quiescence

#[test]
fn a_running_emulator_refuses_before_any_write() {
    let world = World::new();
    let result = apply_ps2_psu_restore_guarded(
        &world.plan(),
        &Fixed(EmulatorQuiescence::Running),
        &world.options(),
    );
    assert_eq!(result.unwrap_err(), Ps2PsuRestoreError::EmulatorRunning);
    assert_eq!(world.card_bytes(), world.original);
    assert!(!world.backup.exists());
    assert!(world.journal_files().is_empty());
}

#[test]
fn an_unknown_emulator_state_refuses_before_any_write() {
    let world = World::new();
    let result = apply_ps2_psu_restore_guarded(
        &world.plan(),
        &Fixed(EmulatorQuiescence::Unknown),
        &world.options(),
    );
    assert_eq!(
        result.unwrap_err(),
        Ps2PsuRestoreError::EmulatorStateUnknown
    );
    assert_eq!(world.card_bytes(), world.original);
    assert!(!world.backup.exists() && world.journal_files().is_empty());
}

#[test]
fn an_emulator_starting_during_staging_is_caught_at_the_final_recheck() {
    let world = World::new();
    // Observation 1 (entry) is Closed; observation 2 (just before the rename) is Running.
    let provider = Sequence::new(&[EmulatorQuiescence::Closed, EmulatorQuiescence::Running]);
    let result = world.apply_hooked(&provider, &|_| Ok(()));
    assert_eq!(result.unwrap_err(), Ps2PsuRestoreError::EmulatorRunning);
    assert_eq!(world.card_bytes(), world.original);
    // The verified backup is retained; the staged temp is cleaned; the journal says why.
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
    assert!(world.stray_temp_files().is_empty());
    assert_eq!(world.only_journal().phase, Ps2RestorePhase::Abandoned);
}

#[test]
fn undo_also_refuses_while_the_emulator_is_running_or_unknown() {
    let world = World::new();
    let applied = world.apply().unwrap();
    let post = world.card_bytes();
    for (state, error) in [
        (
            EmulatorQuiescence::Running,
            Ps2PsuRestoreError::EmulatorRunning,
        ),
        (
            EmulatorQuiescence::Unknown,
            Ps2PsuRestoreError::EmulatorStateUnknown,
        ),
    ] {
        let result = undo_ps2_psu_restore_guarded(&applied.journal_path, &Fixed(state), 1);
        assert_eq!(result.unwrap_err(), error);
        assert_eq!(world.card_bytes(), post);
        assert_eq!(
            load_ps2_restore_journal(&applied.journal_path)
                .unwrap()
                .phase,
            Ps2RestorePhase::Published
        );
    }
}

// ------------------------------------------------------------ external edits

#[test]
fn a_card_changed_between_review_and_apply_is_refused_without_side_effects() {
    let world = World::new();
    let plan = world.plan();
    flip(&world.card, 100);
    let edited = world.card_bytes();
    let result = apply_ps2_psu_restore_guarded(&plan, &CLOSED, &world.options());
    assert_eq!(result.unwrap_err(), Ps2PsuRestoreError::CardChanged);
    assert_eq!(world.card_bytes(), edited);
    assert!(!world.backup.exists() && world.journal_files().is_empty());
}

#[test]
fn a_card_changed_during_staging_is_refused_and_the_edit_survives() {
    let world = World::new();
    let card = world.card.clone();
    let result = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::AfterStaged {
            flip(&card, 5000);
        }
        Ok(())
    });
    assert_eq!(result.unwrap_err(), Ps2PsuRestoreError::CardChanged);
    let mut expected = world.original.clone();
    expected[5000] ^= 0xff;
    assert_eq!(
        world.card_bytes(),
        expected,
        "the external edit must survive"
    );
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
    assert!(world.stray_temp_files().is_empty());
    assert_eq!(world.only_journal().phase, Ps2RestorePhase::Abandoned);
}

#[test]
fn a_card_replaced_immediately_before_publication_is_never_overwritten() {
    let world = World::new();
    let card = world.card.clone();
    let replacement_path = world.dir.path().join("replacement.tmp");
    let mut replacement = world.original.clone();
    replacement[9000] = 0x42;
    let wanted = replacement.clone();
    // Fires after the final recheck: a different file takes the card's place.
    let result = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::BeforeRename {
            fs::write(&replacement_path, &replacement).unwrap();
            fs::rename(&replacement_path, &card).unwrap();
        }
        Ok(())
    });
    assert_eq!(result.unwrap_err(), Ps2PsuRestoreError::CardChanged);
    assert_eq!(
        world.card_bytes(),
        wanted,
        "the replacement must be preserved exactly"
    );
    assert!(world.stray_temp_files().is_empty());
    assert_ne!(world.only_journal().phase, Ps2RestorePhase::Published);
}

#[test]
fn an_in_place_edit_after_the_final_recheck_is_detected_and_kept() {
    let world = World::new();
    let card = world.card.clone();
    let result = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::BeforeRename {
            flip(&card, 12_345);
        }
        Ok(())
    });
    assert_eq!(result.unwrap_err(), Ps2PsuRestoreError::CardChanged);
    let mut expected = world.original.clone();
    expected[12_345] ^= 0xff;
    assert_eq!(world.card_bytes(), expected);
    assert!(world.stray_temp_files().is_empty());
}

#[test]
fn a_changed_psu_is_refused_at_the_final_recheck() {
    let world = World::new();
    let psu = world.psu.clone();
    let result = world.apply_hooked(&CLOSED, &|step| {
        if step == Ps2RestoreStep::AfterStaged {
            let mut bytes = fs::read(&psu).unwrap();
            bytes[3000] ^= 1;
            fs::write(&psu, bytes).unwrap();
        }
        Ok(())
    });
    assert_eq!(result.unwrap_err(), Ps2PsuRestoreError::SourceChanged);
    assert_eq!(world.card_bytes(), world.original);
}

// -------------------------------------------------- partial failure / rollback

#[test]
fn a_failure_after_publication_rolls_the_card_back_exactly() {
    let world = World::new();
    let result = world.apply_hooked(&CLOSED, &|step| fail_at(step, Ps2RestoreStep::AfterRename));
    assert!(matches!(result, Err(Ps2PsuRestoreError::Io(_))));
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
    assert!(world.stray_temp_files().is_empty());
    let journal = world.only_journal();
    assert_eq!(journal.phase, Ps2RestorePhase::RolledBack);
    assert!(
        journal
            .detail
            .unwrap()
            .contains("returned to its pre-restore content")
    );
}

#[test]
fn a_failed_rollback_is_reported_with_the_backup_and_the_journal() {
    let world = World::new();
    let result = world.apply_hooked(&CLOSED, &|step| {
        if matches!(
            step,
            Ps2RestoreStep::AfterRename | Ps2RestoreStep::BeforeRollbackWrite
        ) {
            Err(std::io::Error::other("injected"))
        } else {
            Ok(())
        }
    });
    let Err(Ps2PsuRestoreError::RollbackFailed {
        backup_path,
        journal_path,
        ..
    }) = result
    else {
        panic!("rollback failure must be reported");
    };
    assert_eq!(backup_path, world.backup);
    assert_eq!(fs::read(&backup_path).unwrap(), world.original);
    let journal = load_ps2_restore_journal(&journal_path.unwrap()).unwrap();
    assert_eq!(journal.phase, Ps2RestorePhase::RollbackFailed);
    assert!(journal.phase.needs_attention());
    // Not a success: the card still holds the restored image, and says so.
    assert_ne!(world.card_bytes(), world.original);
}

#[test]
fn a_backup_that_already_exists_is_never_overwritten() {
    let world = World::new();
    let plan = world.plan();
    fs::write(&world.backup, b"precious").unwrap();
    let result = apply_ps2_psu_restore_guarded(&plan, &CLOSED, &world.options());
    assert!(matches!(result, Err(Ps2PsuRestoreError::BackupExists(_))));
    assert_eq!(fs::read(&world.backup).unwrap(), b"precious");
    assert_eq!(world.card_bytes(), world.original);
}

// ------------------------------------------------------------------- restart

#[test]
fn restart_after_a_crash_before_publication_abandons_cleanly() {
    let world = World::new();
    let result = world.apply_hooked(&CLOSED, &|step| crash(step, Ps2RestoreStep::AfterStaged));
    assert!(result.is_err());
    // The "process died": a staged temp and a Staged journal remain.
    assert_eq!(world.stray_temp_files().len(), 1);
    let found = discover_ps2_restore_journals(&world.journals);
    assert_eq!(found.len(), 1);
    assert!(found[0].needs_recovery && !found[0].undo_available);
    assert_eq!(found[0].phase, Some(Ps2RestorePhase::Staged));

    let outcome = recover_ps2_psu_restore(&found[0].path, 5).unwrap();
    assert_eq!(outcome, Ps2RecoveryOutcome::AbandonedBeforePublication);
    assert_eq!(world.card_bytes(), world.original);
    assert!(world.stray_temp_files().is_empty());
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
    assert_eq!(world.only_journal().phase, Ps2RestorePhase::Abandoned);
    assert!(!discover_ps2_restore_journals(&world.journals)[0].needs_recovery);
}

#[test]
fn restart_after_a_crash_just_before_the_rename_finds_the_card_unpublished() {
    let world = World::new();
    let _ = world.apply_hooked(&CLOSED, &|step| crash(step, Ps2RestoreStep::BeforeRename));
    assert_eq!(world.only_journal().phase, Ps2RestorePhase::Publishing);
    let path = world.journal_files().remove(0);
    assert_eq!(
        recover_ps2_psu_restore(&path, 5).unwrap(),
        Ps2RecoveryOutcome::NotPublished
    );
    assert_eq!(world.card_bytes(), world.original);
    assert!(world.stray_temp_files().is_empty());
}

#[test]
fn restart_after_a_crash_just_after_the_rename_confirms_publication_and_keeps_undo() {
    let world = World::new();
    let _ = world.apply_hooked(&CLOSED, &|step| crash(step, Ps2RestoreStep::AfterRename));
    let journal = world.only_journal();
    assert_eq!(journal.phase, Ps2RestorePhase::Publishing);
    let post = journal.post_sha256.clone().unwrap();
    assert_eq!(sha256_hex(&world.card_bytes()), post);
    let path = world.journal_files().remove(0);
    assert_eq!(
        recover_ps2_psu_restore(&path, 5).unwrap(),
        Ps2RecoveryOutcome::ConfirmedPublished
    );
    assert!(world.stray_temp_files().is_empty());
    // Undo is available again and restores the original exactly.
    undo_ps2_psu_restore_guarded(&path, &CLOSED, 6).unwrap();
    assert_eq!(world.card_bytes(), world.original);
}

#[test]
fn restart_never_writes_when_the_card_is_in_neither_expected_state() {
    let world = World::new();
    let _ = world.apply_hooked(&CLOSED, &|step| crash(step, Ps2RestoreStep::BeforeRename));
    flip(&world.card, 777);
    let edited = world.card_bytes();
    let path = world.journal_files().remove(0);
    assert!(matches!(
        recover_ps2_psu_restore(&path, 5).unwrap(),
        Ps2RecoveryOutcome::NeedsAttention(_)
    ));
    assert_eq!(world.card_bytes(), edited);
    assert_eq!(world.only_journal().phase, Ps2RestorePhase::NeedsAttention);
    // And it stays that way: a second recovery changes nothing either.
    assert!(matches!(
        recover_ps2_psu_restore(&path, 6).unwrap(),
        Ps2RecoveryOutcome::NeedsAttention(_)
    ));
}

#[test]
fn restart_after_an_interrupted_undo_is_judged_from_the_card() {
    let world = World::new();
    let applied = world.apply().unwrap();
    let post = world.card_bytes();
    // Crash before the undo rename: card still the restored image.
    let _ = undo_with_hook(&applied.journal_path, &CLOSED, 7, &|step| {
        crash(step, Ps2RestoreStep::BeforeUndoRename)
    });
    assert_eq!(
        load_ps2_restore_journal(&applied.journal_path)
            .unwrap()
            .phase,
        Ps2RestorePhase::UndoIntent
    );
    assert_eq!(world.card_bytes(), post);
    assert_eq!(
        world.stray_temp_files().len(),
        1,
        "the crash left the staged original"
    );
    assert_eq!(
        recover_ps2_psu_restore(&applied.journal_path, 8).unwrap(),
        Ps2RecoveryOutcome::UndoNotApplied
    );
    assert!(world.stray_temp_files().is_empty());
    // Crash after the undo rename: card already the original.
    let _ = undo_with_hook(&applied.journal_path, &CLOSED, 9, &|step| {
        crash(step, Ps2RestoreStep::AfterUndoRename)
    });
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(
        recover_ps2_psu_restore(&applied.journal_path, 10).unwrap(),
        Ps2RecoveryOutcome::UndoCompleted
    );
    assert!(
        world.stray_temp_files().is_empty(),
        "the displaced applied card is cleaned"
    );
}

// ---------------------------------------------------------------------- undo

#[test]
fn external_changes_before_undo_are_refused_and_nothing_is_overwritten() {
    let world = World::new();
    let applied = world.apply().unwrap();
    flip(&world.card, 4242);
    let edited = world.card_bytes();
    assert_eq!(
        undo_ps2_psu_restore_guarded(&applied.journal_path, &CLOSED, 2).unwrap_err(),
        Ps2PsuRestoreError::StaleUndo
    );
    assert_eq!(world.card_bytes(), edited);
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
}

#[test]
fn undo_is_bound_to_the_applied_card_not_merely_to_its_bytes() {
    let world = World::new();
    let applied = world.apply().unwrap();
    // A different file with byte-identical content replaces the applied card.
    let post = world.card_bytes();
    let swap = world.dir.path().join("swap.tmp");
    fs::write(&swap, &post).unwrap();
    fs::rename(&swap, &world.card).unwrap();
    assert_eq!(
        undo_ps2_psu_restore_guarded(&applied.journal_path, &CLOSED, 2).unwrap_err(),
        Ps2PsuRestoreError::StaleUndo
    );
    assert_eq!(world.card_bytes(), post);
}

#[test]
fn undo_refuses_when_the_backup_was_tampered_with() {
    let world = World::new();
    let applied = world.apply().unwrap();
    let post = world.card_bytes();
    flip(&world.backup, 10);
    assert_eq!(
        undo_ps2_psu_restore_guarded(&applied.journal_path, &CLOSED, 2).unwrap_err(),
        Ps2PsuRestoreError::BackupVerificationFailed
    );
    assert_eq!(world.card_bytes(), post);
}

#[test]
fn a_second_undo_is_refused() {
    let world = World::new();
    let applied = world.apply().unwrap();
    undo_ps2_psu_restore_guarded(&applied.journal_path, &CLOSED, 2).unwrap();
    assert_eq!(
        undo_ps2_psu_restore_guarded(&applied.journal_path, &CLOSED, 3).unwrap_err(),
        Ps2PsuRestoreError::StaleUndo
    );
    assert_eq!(world.card_bytes(), world.original);
}

// ------------------------------------------------------------------- journal

#[test]
fn corrupt_journals_are_typed_errors_and_never_touch_the_card() {
    let world = World::new();
    let applied = world.apply().unwrap();
    let good = fs::read(&applied.journal_path).unwrap();
    let post = world.card_bytes();
    let mut flipped = good.clone();
    let last = flipped.len() - 5;
    flipped[last] ^= 1;
    let variants: Vec<(&str, Vec<u8>)> = vec![
        ("truncated", good[..good.len() / 2].to_vec()),
        ("flipped body byte", flipped),
        ("garbage", b"not a journal at all".to_vec()),
        ("empty", Vec::new()),
        (
            "header only",
            good[..good.iter().position(|b| *b == b'\n').unwrap()].to_vec(),
        ),
    ];
    for (label, bytes) in variants {
        fs::write(&applied.journal_path, &bytes).unwrap();
        assert!(
            matches!(
                load_ps2_restore_journal(&applied.journal_path),
                Err(Ps2PsuRestoreError::JournalCorrupt(_))
            ),
            "{label}"
        );
        assert!(
            matches!(
                undo_ps2_psu_restore_guarded(&applied.journal_path, &CLOSED, 2),
                Err(Ps2PsuRestoreError::JournalCorrupt(_))
            ),
            "{label}"
        );
        assert!(
            matches!(
                recover_ps2_psu_restore(&applied.journal_path, 2),
                Err(Ps2PsuRestoreError::JournalCorrupt(_))
            ),
            "{label}"
        );
        let found = discover_ps2_restore_journals(&world.journals);
        assert!(
            found[0].error.is_some() && found[0].needs_attention,
            "{label}"
        );
        assert_eq!(world.card_bytes(), post, "{label}");
    }
    // The checksum covers real content: restoring the bytes makes it valid again.
    fs::write(&applied.journal_path, &good).unwrap();
    assert!(load_ps2_restore_journal(&applied.journal_path).is_ok());
}

#[test]
fn journals_must_live_in_a_real_directory() {
    let world = World::new();
    let plan = world.plan();
    let link = world.dir.path().join("link");
    std::os::unix::fs::symlink(&world.journals, &link).unwrap();
    for dir in [
        link,
        PathBuf::from("relative/dir"),
        world.dir.path().join("missing"),
    ] {
        let options = Ps2RestoreGuardOptions {
            journal_dir: dir,
            unix_seconds: 1,
        };
        assert!(apply_ps2_psu_restore_guarded(&plan, &CLOSED, &options).is_err());
    }
    assert_eq!(world.card_bytes(), world.original);
    assert!(!world.backup.exists());
}

// ---------------------------------------------------------------- round trip

#[test]
fn a_verified_round_trip_restores_and_undoes_exactly_and_keeps_the_card_mode() {
    let world = World::new();
    fs::set_permissions(&world.card, fs::Permissions::from_mode(0o600)).unwrap();
    let applied = world.apply().unwrap();
    let journal = load_ps2_restore_journal(&applied.journal_path).unwrap();
    assert_eq!(journal.phase, Ps2RestorePhase::Published);
    assert_eq!(
        journal.history.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        [
            Ps2RestorePhase::Intent,
            Ps2RestorePhase::BackupVerified,
            Ps2RestorePhase::Staged,
            Ps2RestorePhase::Publishing,
            Ps2RestorePhase::Published,
        ]
    );
    assert_eq!(fs::read(&world.backup).unwrap(), world.original);
    assert_eq!(
        sha256_hex(&world.card_bytes()),
        applied.result.post_restore_card_sha256
    );
    assert_eq!(world.card_bytes().len(), world.original.len());
    assert_eq!(
        fs::metadata(&world.card).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let after = inspect_memory_card(&world.card).unwrap();
    assert_eq!(after.health, MemoryCardHealth::Healthy);
    assert!(world.stray_temp_files().is_empty());
    assert!(discover_ps2_restore_journals(&world.journals)[0].undo_available);

    undo_ps2_psu_restore_guarded(&applied.journal_path, &CLOSED, 2).unwrap();
    assert_eq!(world.card_bytes(), world.original);
    assert_eq!(
        fs::metadata(&world.card).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        load_ps2_restore_journal(&applied.journal_path)
            .unwrap()
            .phase,
        Ps2RestorePhase::Undone
    );
    assert_eq!(
        fs::read(&world.backup).unwrap(),
        world.original,
        "backup is retained"
    );
}

#[test]
fn restoring_a_new_save_preserves_every_unrelated_save_and_cluster() {
    let world = World::new();
    // A PSU for a different save name: the existing save is unrelated.
    let mut psu = fs::read(&world.psu).unwrap();
    psu[0x40..0x40 + 16].copy_from_slice(b"BASLUS-00002NEWS");
    fs::write(&world.psu, &psu).unwrap();
    let card = inspect_memory_card(&world.card).unwrap();
    let plan = plan_ps2_psu_restore(&card, &world.psu, &world.backup, false).unwrap();
    assert!(!plan.existing_save);
    apply_ps2_psu_restore_guarded(&plan, &CLOSED, &world.options()).unwrap();

    let after = inspect_memory_card(&world.card).unwrap();
    let saves = &after.ps2_inventory.as_ref().unwrap().save_directories;
    assert_eq!(saves.len(), 2, "the original save must still be there");
    let before_inventory = card.ps2_inventory.as_ref().unwrap();
    let kept = &before_inventory.save_directories[0];
    let now = fs::read(&world.card).unwrap();
    let alloc = card.ps2_geometry.as_ref().unwrap().alloc_offset as usize;
    // Existing save: directory cluster and file clusters are byte-identical.
    for cluster in std::iter::once(&kept.chain_health.clusters)
        .chain(kept.files.iter().map(|f| &f.chain_health.clusters))
        .flatten()
    {
        let start = (alloc + *cluster as usize) * CLUSTER;
        assert_eq!(
            now[start..start + CLUSTER],
            world.original[start..start + CLUSTER],
            "cluster {cluster} of the unrelated save changed"
        );
    }
    // Only the FAT, the root directory and newly allocated clusters changed.
    let occupied: HashSet<usize> = [0usize, 2, 3, 4, 9].into_iter().collect();
    for index in 0..now.len() / CLUSTER {
        if now[index * CLUSTER..(index + 1) * CLUSTER]
            != world.original[index * CLUSTER..(index + 1) * CLUSTER]
        {
            let fat_or_root = index == 8 || index == alloc || index == alloc + 9;
            let fresh = index > alloc && !occupied.contains(&(index - alloc));
            assert!(fat_or_root || fresh, "unexpected change in cluster {index}");
        }
    }
    assert_eq!(now.len(), world.original.len());
    // Header/superblock bytes are untouched.
    assert_eq!(now[..0x200], world.original[..0x200]);
}

// ----------------------------------------------------- ECC / spare invariants

#[test]
fn cards_with_spare_ecc_bytes_are_refused_not_written_with_stale_ecc() {
    // A healthy 528-byte-page card and a PSU exported from the data-only fixture.
    let source = World::new();
    let dir = tempfile::tempdir().unwrap();
    let card_path = dir.path().join("ecc.ps2");
    fs::write(&card_path, ps2_fixture(PS2_PHYSICAL_PAGE_BYTES)).unwrap();
    let card = inspect_memory_card(&card_path).unwrap();
    assert_eq!(
        card.ps2_geometry.as_ref().unwrap().representation,
        Ps2PageRepresentation::RawWithSpare
    );
    let backup = dir.path().join("b.card");
    let error = plan_ps2_psu_restore(&card, &source.psu, &backup, true).unwrap_err();
    assert!(
        matches!(error, Ps2PsuRestoreError::UnsupportedRepresentation(_)),
        "{error:?}"
    );
    assert!(!backup.exists());
    assert_eq!(
        fs::read(&card_path).unwrap(),
        ps2_fixture(PS2_PHYSICAL_PAGE_BYTES)
    );
}

#[test]
fn only_a_data_only_geometry_passes_the_representation_gate() {
    let world = World::new();
    let mut geometry = world.plan().geometry.clone();
    assert!(restore_require_data_only_pages(&geometry).is_ok());
    for mutate in [
        (|g: &mut Ps2Geometry| g.representation = Ps2PageRepresentation::RawWithSpare)
            as fn(&mut Ps2Geometry),
        |g| g.representation = Ps2PageRepresentation::Unknown,
        |g| g.spare_bytes = 16,
        |g| g.page_stride_bytes = 528,
    ] {
        let mut changed = geometry.clone();
        mutate(&mut changed);
        assert!(matches!(
            restore_require_data_only_pages(&changed),
            Err(Ps2PsuRestoreError::UnsupportedRepresentation(_))
        ));
    }
    geometry.representation = Ps2PageRepresentation::RawDataOnly;
    assert!(restore_require_data_only_pages(&geometry).is_ok());
}

#[test]
fn the_writer_does_not_touch_spare_bytes_which_is_why_ecc_cards_are_refused() {
    // Documents the hazard behind the refusal: the pure page writer updates only
    // the 512 data bytes of each page, so on a 528-byte-stride image the spare
    // (ECC) bytes of every rewritten page would stay stale.
    let mut card = ps2_fixture(PS2_PHYSICAL_PAGE_BYTES);
    for page in 0..card.len() / PS2_PHYSICAL_PAGE_BYTES {
        card[page * PS2_PHYSICAL_PAGE_BYTES + PS2_PAGE_DATA_BYTES
            ..(page + 1) * PS2_PHYSICAL_PAGE_BYTES]
            .fill(0xA5);
    }
    let geometry = {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.ps2");
        fs::write(&path, &card).unwrap();
        inspect_memory_card(&path).unwrap().ps2_geometry.unwrap()
    };
    let mut written = card.clone();
    restore_write_logical_cluster(&mut written, &geometry, 41, &vec![0x11; CLUSTER]).unwrap();
    let changed_data = card != written;
    assert!(changed_data);
    for page in 0..written.len() / PS2_PHYSICAL_PAGE_BYTES {
        let spare = page * PS2_PHYSICAL_PAGE_BYTES + PS2_PAGE_DATA_BYTES;
        assert_eq!(
            written[spare..spare + PS2_SPARE_BYTES],
            [0xA5; PS2_SPARE_BYTES]
        );
    }
}

// ----------------------------------------------------------- legacy primitive

#[test]
#[allow(deprecated)]
fn the_unguarded_primitive_still_round_trips_for_existing_callers() {
    let world = World::new();
    let plan = world.plan();
    let result = apply_ps2_psu_restore(&plan).unwrap();
    undo_ps2_psu_restore(&result).unwrap();
    assert_eq!(world.card_bytes(), world.original);
}

// ----------------------------------------------------------------- detector

fn fake_proc(entries: &[(&str, &str, &str, Option<&Path>)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (pid, comm, cmdline, fd_target) in entries {
        let dir = root.path().join(pid);
        fs::create_dir_all(dir.join("fd")).unwrap();
        fs::write(dir.join("comm"), comm).unwrap();
        fs::write(dir.join("cmdline"), cmdline.replace(' ', "\0")).unwrap();
        if let Some(target) = fd_target {
            std::os::unix::fs::symlink(target, dir.join("fd/7")).unwrap();
        }
    }
    root
}

fn observe(root: &Path, card: &Path) -> EmulatorQuiescence {
    ProcScanQuiescence::with_root(root.to_path_buf()).observe(&ps2_card_binding(card, "SAVE"))
}

#[test]
fn the_proc_detector_distinguishes_closed_running_and_unknown() {
    let world = World::new();
    let other = world.dir.path().join("other.bin");
    // Nothing relevant running.
    let quiet = fake_proc(&[
        ("100", "bash", "bash -l", None),
        ("101", "firefox", "firefox", Some(&other)),
    ]);
    assert_eq!(
        observe(quiet.path(), &world.card),
        EmulatorQuiescence::Closed
    );
    // Emulator by name (comm and command line, any case, Flatpak style too).
    for (comm, cmdline) in [
        ("pcsx2-qt", "/opt/pcsx2-qt"),
        ("PCSX2", "x"),
        ("bwrap", "flatpak run net.pcsx2.PCSX2"),
        ("AetherSX2", "x"),
    ] {
        let running = fake_proc(&[("200", comm, cmdline, None)]);
        assert_eq!(
            observe(running.path(), &world.card),
            EmulatorQuiescence::Running,
            "{comm}"
        );
    }
    // Any process holding this card open, whatever its name.
    let holder = fake_proc(&[("300", "someapp", "someapp", Some(&world.card))]);
    assert_eq!(
        observe(holder.path(), &world.card),
        EmulatorQuiescence::Running
    );
    // Unreadable or missing /proc: cannot say.
    assert_eq!(
        observe(&world.dir.path().join("no-proc"), &world.card),
        EmulatorQuiescence::Unknown
    );
}

#[test]
fn an_unreadable_process_of_our_own_user_makes_the_state_unknown() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // permissions do not constrain root
    }
    let world = World::new();
    let proc = fake_proc(&[("400", "bash", "bash", None)]);
    let fd = proc.path().join("400/fd");
    fs::set_permissions(&fd, fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        observe(proc.path(), &world.card),
        EmulatorQuiescence::Unknown
    );
    fs::set_permissions(&fd, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn the_detector_drives_a_real_refusal_end_to_end() {
    let world = World::new();
    let plan = world.plan();
    let running = fake_proc(&[("500", "pcsx2-qt", "pcsx2-qt", None)]);
    let result = apply_ps2_psu_restore_guarded(
        &plan,
        &ProcScanQuiescence::with_root(running.path().to_path_buf()),
        &world.options(),
    );
    assert_eq!(result.unwrap_err(), Ps2PsuRestoreError::EmulatorRunning);
    let quiet = fake_proc(&[("501", "bash", "bash", None)]);
    apply_ps2_psu_restore_guarded(
        &plan,
        &ProcScanQuiescence::with_root(quiet.path().to_path_buf()),
        &world.options(),
    )
    .unwrap();
}

#[test]
fn hooks_observe_every_phase_in_order() {
    let world = World::new();
    let seen = RefCell::new(Vec::new());
    let count = Cell::new(0);
    world
        .apply_hooked(&CLOSED, &|step| {
            seen.borrow_mut().push(step);
            count.set(count.get() + 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        *seen.borrow(),
        [
            Ps2RestoreStep::AfterIntent,
            Ps2RestoreStep::AfterBackup,
            Ps2RestoreStep::AfterStaged,
            Ps2RestoreStep::BeforeRename,
            Ps2RestoreStep::AfterRename,
        ]
    );
}

#[test]
fn an_undo_target_replaced_at_the_last_instant_is_swapped_back_untouched() {
    let world = World::new();
    let applied = world.apply().unwrap();
    let card = world.card.clone();
    let swap = world.dir.path().join("late.tmp");
    let mut late = world.card_bytes();
    late[2000] ^= 0x55;
    let wanted = late.clone();
    let result = undo_with_hook(&applied.journal_path, &CLOSED, 2, &|step| {
        if step == Ps2RestoreStep::BeforeUndoRename {
            fs::write(&swap, &late).unwrap();
            fs::rename(&swap, &card).unwrap();
        }
        Ok(())
    });
    assert_eq!(result.unwrap_err(), Ps2PsuRestoreError::StaleUndo);
    assert_eq!(
        world.card_bytes(),
        wanted,
        "the late replacement must survive"
    );
    assert!(world.stray_temp_files().is_empty());
    assert_eq!(
        load_ps2_restore_journal(&applied.journal_path)
            .unwrap()
            .phase,
        Ps2RestorePhase::Published,
        "nothing changed, so the (now stale) undo is simply refused"
    );
}
