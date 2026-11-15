//! Restore records (recovery, review, blocking) and retained storage.
//! Synthetic cards, journals and files in temporary directories only.

use super::*;
use crate::gui_v2::card_restore::{
    recovery::{self, RecordKind},
    storage::{self, Status},
};

/// One completed restore: (world, snapshot, guarded result, pre-restore card).
fn restored() -> (World, SaveSnapshot, Ps2GuardedRestore, Vec<u8>) {
    let world = World::new();
    let snapshot = world.snapshot();
    let played = world.play();
    let preview = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 1).unwrap();
    let done = apply_restore(&preview.plan, &CLOSED, &world.options).unwrap();
    (world, snapshot, done, played)
}

fn row(world: &World, journal: &Path) -> recovery::RecordRow {
    recovery::load_records(&world.options.journal_dir)
        .rows
        .into_iter()
        .find(|row| row.path == journal)
        .unwrap()
}

fn park(journal: &Path) {
    rewrite_journal(journal, |journal| {
        journal.phase = Ps2RestorePhase::NeedsAttention;
        journal.detail = Some("synthetic: neither expected state".into());
    });
}

fn garbage_journal(world: &World) -> PathBuf {
    let path = world
        .options
        .journal_dir
        .join("ps2-psu-restore-garbage.json");
    fs::write(&path, b"not a journal").unwrap();
    path
}

#[test]
fn records_are_classified_and_never_changed_by_listing() {
    let (world, _, done, _) = restored();
    assert_eq!(
        row(&world, &done.journal_path).kind,
        RecordKind::UndoAvailable
    );
    let before = fs::read(&done.journal_path).unwrap();
    rewrite_journal(&done.journal_path, |j| {
        j.phase = Ps2RestorePhase::Publishing
    });
    assert_eq!(row(&world, &done.journal_path).kind, RecordKind::Unfinished);
    park(&done.journal_path);
    assert_eq!(
        row(&world, &done.journal_path).kind,
        RecordKind::NeedsReview
    );
    let garbage = garbage_journal(&world);
    let unreadable = row(&world, &garbage);
    assert_eq!(unreadable.kind, RecordKind::Unreadable);
    assert!(unreadable.blocks(&world.card) && unreadable.blocks(Path::new("/elsewhere.ps2")));
    assert_eq!(fs::read(&garbage).unwrap(), b"not a journal");
    assert_ne!(
        fs::read(&done.journal_path).unwrap(),
        before,
        "only the test rewrote it"
    );
}

#[test]
fn recovery_needs_proven_closure_and_keeps_every_record() {
    let (world, _, done, _) = restored();
    rewrite_journal(&done.journal_path, |j| {
        j.phase = Ps2RestorePhase::Publishing
    });
    let card = world.card_bytes();
    let interrupted = fs::read(&done.journal_path).unwrap();
    for observer in [RUNNING, UNKNOWN] {
        assert!(recovery::recover(&world.options.journal_dir, &observer, 5).is_err());
        assert_eq!(fs::read(&done.journal_path).unwrap(), interrupted);
    }
    let messages = recovery::recover(&world.options.journal_dir, &CLOSED, 5).unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("finished and the card was verified")),
        "{messages:?}"
    );
    assert_eq!(
        row(&world, &done.journal_path).kind,
        RecordKind::UndoAvailable
    );
    assert_eq!(world.card_bytes(), card, "recovery never writes the card");
    // Blue can now Undo the recovered restore.
    undo_restore(&done.journal_path, &CLOSED, 6).unwrap();
    assert!(done.journal_path.exists());
}

#[test]
fn a_parked_record_is_reviewable_only_while_the_pre_restore_card_provably_exists() {
    let (world, snapshot, done, _) = restored();
    park(&done.journal_path);
    let journal_bytes = fs::read(&done.journal_path).unwrap();
    assert!(row(&world, &done.journal_path).blocks(&world.card));

    // Backup intact: allowed, and recorded without touching the evidence.
    let preview = recovery::review_preview(&done.journal_path).unwrap();
    assert!(preview.allowed.is_ok(), "{:?}", preview.allowed);
    assert!(
        preview
            .facts
            .iter()
            .any(|f| f.contains("present and verified"))
    );
    recovery::mark_reviewed(&preview, 7).unwrap();
    let reviewed = row(&world, &done.journal_path);
    assert!(reviewed.review.is_some());
    assert!(
        !reviewed.blocks(&world.card),
        "reviewed record no longer blocks"
    );
    assert_eq!(fs::read(&done.journal_path).unwrap(), journal_bytes);
    assert!(done.result.backup_path.exists());
    // Marking twice is refused (create-only); nothing changes.
    assert!(recovery::mark_reviewed(&preview, 8).is_err());
    // Restores to the card work again.
    let again = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 9).unwrap();
    apply_restore(&again.plan, &CLOSED, &world.options).unwrap();

    // A changed journal invalidates the review: blocking returns.
    rewrite_journal(&done.journal_path, |j| j.detail = Some("edited".into()));
    let changed = row(&world, &done.journal_path);
    assert!(changed.review.is_none() && changed.blocks(&world.card));
}

#[test]
fn a_parked_record_without_provable_pre_restore_content_stays_blocking() {
    let (world, _, done, _) = restored();
    park(&done.journal_path);
    // Card changed after the restore and the backup is gone.
    fs::remove_file(&done.result.backup_path).unwrap();
    let mut card = world.card_bytes();
    card[ICON_DATA.start] = 0x33;
    fs::write(&world.card, &card).unwrap();
    let preview = recovery::review_preview(&done.journal_path).unwrap();
    assert!(preview.allowed.is_err());
    assert!(recovery::mark_reviewed(&preview, 7).is_err());
    let row = row(&world, &done.journal_path);
    assert!(row.review.is_none() && row.blocks(&world.card));
    assert!(done.journal_path.exists());
}

#[test]
fn an_unchanged_card_without_a_verified_backup_is_not_grounds_for_review() {
    // That evidence would vanish at the next restore and silently re-park the
    // card, including the Undo of that restore; only a backup lasts.
    let (world, _, done, played) = restored();
    park(&done.journal_path);
    fs::write(&world.card, &played).unwrap();
    fs::remove_file(&done.result.backup_path).unwrap();
    let preview = recovery::review_preview(&done.journal_path).unwrap();
    assert!(
        preview
            .facts
            .iter()
            .any(|f| f.contains("the same as before the restore"))
    );
    assert!(preview.allowed.is_err(), "{:?}", preview.allowed);
    assert!(recovery::mark_reviewed(&preview, 7).is_err());
    let row = row(&world, &done.journal_path);
    assert!(row.review.is_none() && row.blocks(&world.card));
    assert_eq!(world.card_bytes(), played);
}

#[test]
fn review_refuses_changed_evidence_and_unfinished_records() {
    let (world, _, done, _) = restored();
    park(&done.journal_path);
    let preview = recovery::review_preview(&done.journal_path).unwrap();
    // The backup changes between preview and confirm.
    let mut backup = fs::read(&done.result.backup_path).unwrap();
    backup[0] ^= 0xff;
    fs::write(&done.result.backup_path, &backup).unwrap();
    assert!(recovery::mark_reviewed(&preview, 7).is_err());
    assert!(row(&world, &done.journal_path).review.is_none());

    rewrite_journal(&done.journal_path, |j| j.phase = Ps2RestorePhase::Staged);
    assert!(recovery::review_preview(&done.journal_path).is_err());
    assert!(row(&world, &done.journal_path).blocks(Path::new("/any/other.ps2")));
}

#[test]
fn marking_an_unreadable_record_reviewed_can_never_unblock_restores() {
    let (world, snapshot, _, _) = restored();
    let garbage = garbage_journal(&world);
    let preview = recovery::review_preview(&garbage).unwrap();
    assert!(preview.allowed.is_err(), "no acknowledgement path");
    assert!(
        preview
            .facts
            .iter()
            .any(|f| f.contains("Evidence to inspect"))
    );
    assert!(
        preview
            .facts
            .iter()
            .any(|f| f.contains(&garbage.display().to_string()))
    );
    assert!(recovery::mark_reviewed(&preview, 7).is_err());
    // Even a forged, hash-matching review sidecar is ignored.
    let forged = recovery::Review {
        version: 1,
        journal_sha256: super::super::sha256_hex(b"not a journal"),
        reviewed_unix_seconds: 7,
        evidence: "forged".into(),
    };
    let mut sidecar = garbage.clone().into_os_string();
    sidecar.push(".reviewed");
    fs::write(&sidecar, serde_json::to_vec(&forged).unwrap()).unwrap();
    let row = row(&world, &garbage);
    assert!(row.review.is_none());
    assert!(row.blocks(&world.card) && row.blocks(Path::new("/any/other.ps2")));
    // The global block holds: no restore to any card.
    let card = world.card_bytes();
    let preview = prepare_restore(&snapshot, &world.location, &folder(), &world.dirs, 8).unwrap();
    assert!(apply_restore(&preview.plan, &CLOSED, &world.options).is_err());
    assert_eq!(world.card_bytes(), card);
    assert_eq!(fs::read(&garbage).unwrap(), b"not a journal", "record kept");
}

const LATER: u64 = 4_000_000_000;

fn old(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
}

#[test]
fn storage_keeps_referenced_uncertain_and_young_files() {
    let (world, _, done, _) = restored();
    let psu = done.result.source_psu_path.clone();
    let backup = done.result.backup_path.clone();
    let orphan = world.dirs.psu_dir.join("orphan.psu");
    old(&orphan, b"orphan psu");
    let leftover = world.dirs.psu_dir.join("x.snapshot-card");
    old(&leftover, b"leftover copy");
    let alias = world.dirs.backup_dir.join("alias.before-restore.ps2");
    fs::hard_link(&backup, &alias).unwrap();
    let foreign = world.dirs.backup_dir.join("notes.txt");
    old(&foreign, b"not ours");
    let link = world.dirs.psu_dir.join("link.psu");
    std::os::unix::fs::symlink(&orphan, &link).unwrap();
    let open = world.dirs.psu_dir.join("open.psu");
    old(&open, b"open preview");

    let report = storage::assess_at(
        &world.dirs,
        &world.options.journal_dir,
        std::slice::from_ref(&open),
        LATER,
    );
    let status = |path: &Path| {
        report
            .files
            .iter()
            .find(|f| f.path == path)
            .unwrap()
            .status
            .clone()
    };
    assert!(matches!(status(&psu), Status::Referenced(_)));
    assert!(matches!(status(&backup), Status::Referenced(_)));
    assert!(
        matches!(status(&alias), Status::Referenced(_)),
        "same inode as a backup"
    );
    assert!(matches!(status(&open), Status::Referenced(_)));
    assert!(matches!(status(&foreign), Status::Uncertain(_)));
    assert!(matches!(status(&link), Status::Uncertain(_)));
    assert_eq!(status(&orphan), Status::Unreferenced);
    assert_eq!(status(&leftover), Status::Unreferenced);
    assert!(report.total_bytes() > 0);

    // Young files are uncertain.
    let now = storage::assess_at(&world.dirs, &world.options.journal_dir, &[], 0);
    assert!(now.retirable().next().is_none());
}

#[test]
fn retiring_needs_a_matching_preview_and_never_removes_needed_files() {
    let (world, _, done, _) = restored();
    let orphan = world.dirs.psu_dir.join("orphan.psu");
    old(&orphan, b"orphan psu");
    let changed = world.dirs.psu_dir.join("changed.psu");
    old(&changed, b"before");
    let preview = storage::assess_at(&world.dirs, &world.options.journal_dir, &[], LATER);
    let mut confirmed: Vec<_> = preview.retirable().cloned().collect();
    assert_eq!(confirmed.len(), 2);
    // Forged confirmation of referenced files is ignored.
    for path in [&done.result.backup_path, &done.result.source_psu_path] {
        let mut forged = confirmed[0].clone();
        forged.path = path.clone();
        confirmed.push(forged);
    }
    old(&changed, b"after the preview");
    let result = storage::retire_at(
        &confirmed,
        &world.dirs,
        &world.options.journal_dir,
        &[],
        LATER,
    );
    assert_eq!(result.removed, vec![orphan.clone()]);
    assert!(!orphan.exists());
    assert!(changed.exists());
    assert!(done.result.backup_path.exists() && done.result.source_psu_path.exists());
    assert_eq!(result.kept.len(), 3);
    // Undo still works: nothing it needs was removed.
    undo_restore(&done.journal_path, &CLOSED, 9).unwrap();
}

#[test]
fn unreadable_records_make_every_file_uncertain() {
    let (world, _, _, _) = restored();
    let orphan = world.dirs.psu_dir.join("orphan.psu");
    old(&orphan, b"orphan psu");
    let preview = storage::assess_at(&world.dirs, &world.options.journal_dir, &[], LATER);
    let confirmed: Vec<_> = preview.retirable().cloned().collect();
    assert_eq!(confirmed.len(), 1);
    garbage_journal(&world);
    let report = storage::assess_at(&world.dirs, &world.options.journal_dir, &[], LATER);
    assert!(!report.problems.is_empty());
    assert!(report.retirable().next().is_none());
    let result = storage::retire_at(
        &confirmed,
        &world.dirs,
        &world.options.journal_dir,
        &[],
        LATER,
    );
    assert!(result.removed.is_empty());
    assert!(orphan.exists());
}

#[test]
fn undo_passes_the_same_records_gate_as_apply() {
    let (world, _, done, _) = restored();
    let restored_card = world.card_bytes();
    // Another restore of the card was interrupted (copy of the record).
    let other = world
        .options
        .journal_dir
        .join("ps2-psu-restore-zz-interrupted.json");
    fs::copy(&done.journal_path, &other).unwrap();
    rewrite_journal(&other, |j| {
        j.phase = Ps2RestorePhase::Staged;
        j.operation_id = "zz-interrupted".into();
    });
    assert!(undo_restore(&done.journal_path, &CLOSED, 5).is_err());
    assert_eq!(world.card_bytes(), restored_card);
    fs::remove_file(&other).unwrap();
    // An unreadable record blocks Undo too.
    let garbage = garbage_journal(&world);
    assert!(undo_restore(&done.journal_path, &CLOSED, 6).is_err());
    assert_eq!(world.card_bytes(), restored_card);
    fs::remove_file(&garbage).unwrap();
    undo_restore(&done.journal_path, &CLOSED, 7).unwrap();
}

#[test]
fn a_review_lifts_the_block_only_while_its_evidence_still_holds() {
    let (world, _, done, _) = restored();
    park(&done.journal_path);
    let preview = recovery::review_preview(&done.journal_path).unwrap();
    recovery::mark_reviewed(&preview, 7).unwrap();
    assert!(!row(&world, &done.journal_path).blocks(&world.card));
    // The backup disappears and the card changes: the block returns.
    fs::remove_file(&done.result.backup_path).unwrap();
    let mut card = world.card_bytes();
    card[ICON_DATA.start] = 0x21;
    fs::write(&world.card, &card).unwrap();
    assert!(row(&world, &done.journal_path).blocks(&world.card));
}

#[test]
fn a_forged_review_of_a_readable_record_proves_nothing() {
    let (world, _, done, _) = restored();
    park(&done.journal_path);
    fs::remove_file(&done.result.backup_path).unwrap();
    let mut card = world.card_bytes();
    card[ICON_DATA.start] = 0x22;
    fs::write(&world.card, &card).unwrap();
    let forged = recovery::Review {
        version: 1,
        journal_sha256: super::super::sha256_hex(&fs::read(&done.journal_path).unwrap()),
        reviewed_unix_seconds: 7,
        evidence: "forged".into(),
    };
    let mut sidecar = done.journal_path.clone().into_os_string();
    sidecar.push(".reviewed");
    fs::write(&sidecar, serde_json::to_vec(&forged).unwrap()).unwrap();
    let row = row(&world, &done.journal_path);
    assert!(row.review.is_some(), "sidecar is well-formed");
    assert!(row.blocks(&world.card), "but its evidence does not hold");
}

#[test]
fn a_symlinked_storage_folder_is_never_cleaned() {
    let world = World::new();
    let outside = world.dirs.psu_dir.parent().unwrap().join("users-own-psus");
    fs::create_dir_all(&outside).unwrap();
    let precious = outside.join("my-save.psu");
    fs::write(&precious, b"user's own PSU").unwrap();
    fs::create_dir_all(world.dirs.psu_dir.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &world.dirs.psu_dir).unwrap();
    let report = storage::assess_at(&world.dirs, &world.options.journal_dir, &[], LATER);
    assert!(!report.problems.is_empty());
    assert!(report.retirable().next().is_none());
    // Even a forged confirmation removes nothing.
    let mut forged = report
        .files
        .iter()
        .find(|f| f.path.ends_with("my-save.psu"))
        .unwrap()
        .clone();
    forged.status = Status::Unreferenced;
    forged.identity = Some((0, 0, String::new()));
    let result = storage::retire_at(
        &[forged],
        &world.dirs,
        &world.options.journal_dir,
        &[],
        LATER,
    );
    assert!(result.removed.is_empty());
    assert_eq!(fs::read(&precious).unwrap(), b"user's own PSU");
}

/// Review of NW2-B-03: a recovery pass that could not list, check or resolve
/// a record is reported as needing attention, not as a finished job; a pass
/// with nothing in progress is not.
#[test]
fn recovery_run_flags_attention_for_unlisted_or_unresolved_records() {
    let (world, _, _done, _) = restored();
    let quiet = recovery::recover_run(&world.options.journal_dir, &CLOSED, 5).unwrap();
    assert!(!quiet.attention, "{:?}", quiet.messages);
    let garbage = garbage_journal(&world);
    let run = recovery::recover_run(&world.options.journal_dir, &CLOSED, 6).unwrap();
    assert!(run.attention, "{:?}", run.messages);
    assert!(
        run.messages
            .iter()
            .any(|m| m.contains("could not be listed") || m.contains("could not be checked")),
        "{:?}",
        run.messages
    );
    assert_eq!(fs::read(&garbage).unwrap(), b"not a journal");
    assert_eq!(
        recovery::recover(&world.options.journal_dir, &CLOSED, 7).unwrap(),
        run.messages
    );
}

/// Review of #387 condition 1: an interrupted restore whose card is in
/// neither expected state resolves as NeedsAttention; that Recover pass
/// settles as a failure ("Needs attention") in Activity, not Finished.
#[test]
fn recover_needing_manual_review_settles_as_needs_attention() {
    let (world, _, done, _) = restored();
    rewrite_journal(&done.journal_path, |j| {
        j.phase = Ps2RestorePhase::Publishing
    });
    let mut card = world.card_bytes();
    let last = card.len() - 1;
    card[last] ^= 0x5a;
    fs::write(&world.card, &card).unwrap();
    let run = recovery::recover_run(&world.options.journal_dir, &CLOSED, 5).unwrap();
    assert!(run.attention, "{:?}", run.messages);
    assert!(
        run.messages
            .iter()
            .any(|m| m.contains("Needs manual review")),
        "{:?}",
        run.messages
    );
    assert_eq!(world.card_bytes(), card, "recovery never writes the card");
    let mut state = crate::gui_v2::card_restore::panel::PanelState::default();
    let (summary, ok) = crate::gui_v2::card_restore::panel::receive(
        &mut state,
        crate::gui_v2::card_restore::panel::PanelOutcome::Records {
            report: recovery::load_records(&world.options.journal_dir),
            messages: run.messages.clone(),
            failed: run.attention,
        },
    );
    assert!(!ok, "settles as Needs attention, not Finished: {summary}");
}
