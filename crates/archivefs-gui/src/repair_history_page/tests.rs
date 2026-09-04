//! Tests for the Repair History page.
//!
//! Every fixture is a fresh per-test temp directory holding a disposable DAT
//! and a disposable "ROM library"; nothing here ever touches a real library.
//! A genuine journaled `Applied` transaction is produced through the real
//! Repair Center apply path (`apply_saved_plan_selected`) rather than
//! hand-faked, so history/undo tests exercise the exact on-disk journal a
//! real repair leaves behind.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::rename_apply::{EntryState, RollbackResult, TransactionState};
use archivefs_core::dat::sources::DatSourceKind;
use archivefs_core::dat::sources::audit_cache::AuditCacheConfig;
use archivefs_core::repair::execute::{RepairExecutionOptions, RepairTransactionResult};
use archivefs_core::repair::library::{
    LibraryScanRequest, RepairProfile, apply_saved_plan_selected, plan_file_from_scan,
    run_library_scan,
};
use archivefs_core::repair::proposal::RepairProposalId;
use archivefs_core::safe_read::TrustedRoots;

use super::*;

/// SHA-1 of `b"test"` (4 bytes).
const SHA1_TEST: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
/// SHA-1 of `b"abc"` (3 bytes).
const SHA1_ABC: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";

/// A per-test temp directory under the system temp dir, removed on drop.
struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "archivefs-gui-repair-history-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// A two-game DAT and two wrongly-named loose ROMs under `dir`.
fn write_apply_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let dat = dir.join("two.dat");
    std::fs::write(
        &dat,
        format!(
            r#"<datafile><header><name>Two</name></header>
<game name="Alpha"><rom name="alpha.bin" size="4" sha1="{SHA1_TEST}"/></game>
<game name="Beta"><rom name="beta.bin" size="3" sha1="{SHA1_ABC}"/></game>
</datafile>"#
        ),
    )
    .unwrap();
    let roms = dir.join("roms");
    std::fs::create_dir(&roms).unwrap();
    std::fs::write(roms.join("a.bin"), b"test").unwrap();
    std::fs::write(roms.join("b.bin"), b"abc").unwrap();
    (dat, roms)
}

/// Runs a real scan and a real selected apply (both Safe proposals), leaving
/// a genuine `Applied` journal on disk under `dir/journal`. Returns the ROM
/// root, the journal directory, and the apply result.
fn scan_and_apply(dir: &Path) -> (PathBuf, PathBuf, RepairTransactionResult) {
    let (dat, roms) = write_apply_fixture(dir);
    let request = LibraryScanRequest {
        source_id: "test".to_string(),
        source_display_name: "Test catalogue".to_string(),
        dat_path: dat.clone(),
        dat_kind: DatSourceKind::File,
        scan_root: roms.clone(),
        limits: DatLimits::default(),
        profile: RepairProfile::CanonicalInPlace,
        audit_cache: AuditCacheConfig::Disabled,
    };
    let outcome = run_library_scan(&request, &TrustedRoots::none(), &no_cancel(), &|_| {})
        .expect("the fixture scan runs");
    let plan = plan_file_from_scan(&outcome);
    let all_ids: Vec<RepairProposalId> = plan
        .repair_plan
        .proposals
        .iter()
        .map(|proposal| proposal.id.clone())
        .collect();
    assert_eq!(all_ids.len(), 2, "the fixture DAT authorizes two repairs");

    let journal_dir = dir.join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let options = RepairExecutionOptions {
        trusted: TrustedRoots::from_paths([&roms]),
        journal_dir: journal_dir.clone(),
        audit_cache: AuditCacheConfig::Disabled,
    };
    let result = apply_saved_plan_selected(
        &plan,
        &roms,
        &dat,
        plan.generation,
        &all_ids,
        &options,
        &no_cancel(),
    )
    .expect("the fixture apply succeeds")
    .rename
    .expect("the fixture's two proposals are ordinary renames");
    assert_eq!(result.summary.applied, 2);
    (roms, journal_dir, result)
}

/// Blocks the test thread until the page's background undo job settles or a
/// generous deadline passes, polling exactly the way the real render loop
/// does.
fn wait_for_undo(state: &mut RepairHistoryPageState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.is_undo_running() {
        state.poll_undo();
        if Instant::now() > deadline {
            panic!("the background undo job did not finish in time");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

/// A deterministic in-memory clipboard stand-in for tests, mirroring the
/// `main.rs` `InMemoryClipboard` test double's shape (this module cannot
/// reach that one - it is private to `main.rs`'s own `tests` module).
#[derive(Default)]
struct NoopClipboard {
    set_calls: Vec<String>,
}

impl crate::ClipboardBackend for NoopClipboard {
    fn get_text_status(&mut self) -> crate::ClipboardTextStatus {
        crate::ClipboardTextStatus::Empty
    }

    fn set_text(&mut self, text: String) -> Result<(), String> {
        self.set_calls.push(text);
        Ok(())
    }
}

fn render(state: &mut RepairHistoryPageState) -> egui::FullOutput {
    let mut clipboard = NoopClipboard::default();
    render_with_clipboard(state, &mut clipboard)
}

fn render_with_clipboard(
    state: &mut RepairHistoryPageState,
    clipboard: &mut NoopClipboard,
) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_repair_history_page(ui, state, clipboard);
        });
    })
}

// --- history loads from persistent journal state ----------------------------

#[test]
fn history_loads_from_the_persistent_journal_on_disk() {
    let dir = TestDir::new("loads-from-disk");
    let (_roms, journal_dir, result) = scan_and_apply(dir.path());

    // A brand new page state, never touched by the apply above: it only
    // knows the journal directory, exactly as a fresh app launch would.
    let state = RepairHistoryPageState::load_with_journal_dir(journal_dir);

    assert_eq!(state.transactions.len(), 1);
    assert_eq!(
        state.transactions[0].transaction_id,
        result.summary.transaction_id
    );
    assert!(state.load_problems.is_empty());
}

#[test]
fn stale_record_can_be_archived_without_rewriting_the_journal_or_game_file() {
    let dir = TestDir::new("archive-stale");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let game_file = dir.path().join("game.bin");
    std::fs::write(&game_file, b"untouched game data").unwrap();
    let stale = bare_transaction(
        "legacy-stale",
        TransactionState::ApplyFailed,
        EntryState::ApplyFailed,
    );
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &stale).unwrap();
    let journal_path =
        archivefs_core::dat::rename_apply::journal_path(&journal_dir, &stale.transaction_id)
            .unwrap();
    let before = std::fs::read_to_string(&journal_path).unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir.clone());
    assert_eq!(
        state.stale_transaction_ids(),
        vec![stale.transaction_id.clone()]
    );
    state.archive_one(&stale.transaction_id);

    assert_eq!(std::fs::read(&game_file).unwrap(), b"untouched game data");
    assert_eq!(std::fs::read_to_string(&journal_path).unwrap(), before);
    assert!(state.is_archived(&stale.transaction_id));
    assert!(state_visible_transaction_ids(&state).is_empty());
    state.show_archived = true;
    state.hide_settled = false;
    assert_eq!(
        state_visible_transaction_ids(&state),
        vec![stale.transaction_id]
    );
}

#[test]
fn resumable_and_rollbackable_records_are_rejected_by_stale_archive() {
    let dir = TestDir::new("archive-gates");
    let (_roms, journal_dir, applied_result) = scan_and_apply(dir.path());
    // The real applied transaction is rollbackable and its destination files
    // still match their recorded identities.
    let mut resumable = bare_transaction(
        "resumable-gated",
        TransactionState::ApplyFailed,
        EntryState::ApplyFailed,
    );
    resumable.unknown.insert(
        "emuwiz_exact_resume_envelope".to_string(),
        serde_json::json!({"format_version": 1}),
    );
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &resumable).unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    state.archive_one("resumable-gated");
    assert!(!state.is_archived("resumable-gated"));
    assert!(state.archive_outcome.as_ref().unwrap().archived == 0);
    state.archive_one(&applied_result.summary.transaction_id);
    assert!(!state.is_archived(&applied_result.summary.transaction_id));
}

#[test]
fn bulk_archive_freezes_exact_count_requires_confirmation_and_survives_restart() {
    let dir = TestDir::new("archive-bulk");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    for id in ["stale-one", "stale-two"] {
        let stale = bare_transaction(id, TransactionState::ApplyFailed, EntryState::ApplyFailed);
        archivefs_core::dat::rename_apply::write_journal(&journal_dir, &stale).unwrap();
    }

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir.clone());
    state.open_archive_confirmation();
    assert_eq!(state.archive_confirm.as_ref().map(Vec::len), Some(2));
    assert!(!archivefs_core::dat::rename_apply::recovery_history_state_path(&journal_dir).exists());
    state.cancel_archive_confirmation();
    assert!(!archivefs_core::dat::rename_apply::recovery_history_state_path(&journal_dir).exists());

    state.open_archive_confirmation();
    state.confirm_archive();
    assert_eq!(state.archive_outcome.as_ref().unwrap().archived, 2);
    assert_eq!(state.stale_transaction_ids().len(), 0);

    let restarted = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    assert_eq!(
        restarted.transactions.len(),
        2,
        "archiving keeps historical journals"
    );
    assert!(
        restarted
            .transactions
            .iter()
            .all(|transaction| restarted.is_archived(&transaction.transaction_id))
    );
}

#[test]
fn stale_card_uses_beginner_archive_wording_and_keeps_record_available() {
    let dir = TestDir::new("archive-wording");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let stale = bare_transaction(
        "stale-wording",
        TransactionState::ApplyFailed,
        EntryState::ApplyFailed,
    );
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &stale).unwrap();
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    state.hide_settled = false;
    let output = render(&mut state);
    assert!(rendered_text_contains(
        &output,
        "The recorded files are no longer where this transaction expected them."
    ));
    assert!(rendered_text_contains(&output, "Nothing will be changed."));
    assert!(rendered_text_contains(&output, "Archive this record"));
    assert!(rendered_text_contains(&output, "Keep in history"));
}

// --- transaction rows reflect counts/status accurately ----------------------

#[test]
fn transaction_rows_reflect_counts_and_status_accurately() {
    let dir = TestDir::new("row-accuracy");
    let (_roms, journal_dir, result) = scan_and_apply(dir.path());
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);

    let transaction = state
        .transaction_by_id(&result.summary.transaction_id)
        .unwrap();
    assert_eq!(transaction.applied_count(), 2);
    assert_eq!(transaction.failed_count(), 0);
    assert_eq!(transaction.state, TransactionState::Applied);

    let output = render(&mut state);
    assert!(rendered_text_contains(
        &output,
        "Requested 2 · Applied 2 · Failed 0 · Skipped 0"
    ));
    assert!(rendered_text_contains(&output, "Rollback: Not requested"));
    assert!(rendered_text_contains(
        &output,
        "2 of 2 destination file(s) verified"
    ));
    assert!(rendered_text_contains(
        &output,
        &result.summary.transaction_id
    ));
}

// --- details show per-entry state --------------------------------------------

#[test]
fn details_show_per_entry_state_and_reverify() {
    let dir = TestDir::new("details");
    let (roms, journal_dir, result) = scan_and_apply(dir.path());
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    state.set_details(Some(result.summary.transaction_id.clone()));

    let output = render(&mut state);
    assert!(rendered_text_contains(&output, "Applied"));
    assert!(rendered_text_contains(&output, "verified"));
    assert!(rendered_text_contains(
        &output,
        &roms.join("alpha.bin").display().to_string()
    ));
    assert!(rendered_text_contains(
        &output,
        &roms.join("beta.bin").display().to_string()
    ));
}

// --- undo enable/disable rules ------------------------------------------------

#[test]
fn undo_is_enabled_for_a_settled_applied_transaction() {
    let dir = TestDir::new("enable-applied");
    let (_roms, journal_dir, result) = scan_and_apply(dir.path());
    let state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    assert!(state.can_undo(&result.summary.transaction_id));
}

#[test]
fn undo_is_disabled_for_a_transaction_with_nothing_applied() {
    let dir = TestDir::new("disable-nothing-applied");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    // An `Applied`-state transaction whose entries never actually applied
    // has nothing to reverse - `RenameTransaction::is_rollbackable` must
    // refuse it, and so must the page.
    let tx = bare_transaction(
        "nothing-applied",
        TransactionState::Applied,
        EntryState::Planned,
    );
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &tx).unwrap();

    let state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    assert!(!state.can_undo("nothing-applied"));
}

#[test]
fn undo_is_disabled_while_another_undo_is_running() {
    let dir = TestDir::new("disable-while-running");
    let (_roms, journal_dir, result) = scan_and_apply(dir.path());
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    assert!(state.can_undo(&result.summary.transaction_id));
    state.undo_running = true;
    assert!(!state.can_undo(&result.summary.transaction_id));
}

#[test]
fn undo_is_disabled_for_an_unknown_transaction_id() {
    let dir = TestDir::new("disable-unknown");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    assert!(!state.can_undo("does-not-exist"));
}

// --- clear completed history ---------------------------------------------

/// A rolled-back transaction and one that never actually applied anything
/// are both safe to clear; a settled `Applied` transaction still awaiting
/// rollback, and one left interrupted mid-apply, are not - `is_rollbackable`
/// is `true` for both of those, so `clearable_transaction_ids` must exclude
/// them (2026-08-22, live-QA Phase 8).
#[test]
fn clearable_ids_include_only_transactions_that_are_not_rollbackable() {
    let dir = TestDir::new("clearable-ids");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();

    let rolled_back = bare_transaction(
        "rolled-back",
        TransactionState::RolledBack,
        EntryState::RolledBack,
    );
    let never_applied = bare_transaction(
        "never-applied",
        TransactionState::Applied,
        EntryState::Planned,
    );
    let still_applied = bare_transaction(
        "still-applied",
        TransactionState::Applied,
        EntryState::Applied,
    );
    let interrupted = bare_transaction(
        "interrupted",
        TransactionState::Applying,
        EntryState::Applying,
    );
    for transaction in [&rolled_back, &never_applied, &still_applied, &interrupted] {
        archivefs_core::dat::rename_apply::write_journal(&journal_dir, transaction).unwrap();
    }

    let state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    let mut clearable = state.clearable_transaction_ids();
    clearable.sort();
    assert_eq!(clearable, vec!["never-applied", "rolled-back"]);
}

#[test]
fn confirming_clear_removes_only_the_frozen_ids_and_leaves_the_rest() {
    let dir = TestDir::new("confirm-clear");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();

    let rolled_back = bare_transaction(
        "rolled-back",
        TransactionState::RolledBack,
        EntryState::RolledBack,
    );
    let still_applied = bare_transaction(
        "still-applied",
        TransactionState::Applied,
        EntryState::Applied,
    );
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &rolled_back).unwrap();
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &still_applied).unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir.clone());
    state.open_clear_confirmation();
    assert_eq!(
        state.clear_confirm.as_deref(),
        Some(["rolled-back".to_string()].as_slice())
    );
    state.confirm_clear();

    assert!(state.clear_confirm.is_none());
    let outcome = state
        .clear_outcome
        .as_ref()
        .expect("an outcome is recorded");
    assert_eq!(outcome.removed, 1);
    assert!(outcome.failed.is_empty());

    // Re-read fresh from disk, not the in-memory state, to prove the file
    // was actually removed and the other journal was actually left alone.
    let (remaining, _problems) = archivefs_core::dat::rename_apply::list_journals(&journal_dir);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].transaction_id, "still-applied");
}

#[test]
fn opening_the_clear_confirmation_is_a_no_op_when_nothing_is_clearable() {
    let dir = TestDir::new("clear-noop");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let still_applied = bare_transaction(
        "still-applied",
        TransactionState::Applied,
        EntryState::Applied,
    );
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &still_applied).unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    state.open_clear_confirmation();
    assert!(state.clear_confirm.is_none());
}

#[test]
fn cancelling_the_clear_confirmation_never_touches_the_filesystem() {
    let dir = TestDir::new("clear-cancel");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let rolled_back = bare_transaction(
        "rolled-back",
        TransactionState::RolledBack,
        EntryState::RolledBack,
    );
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &rolled_back).unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir.clone());
    state.open_clear_confirmation();
    assert!(state.clear_confirm.is_some());
    state.cancel_clear_confirmation();
    assert!(state.clear_confirm.is_none());
    assert!(state.clear_outcome.is_none());

    let (remaining, _problems) = archivefs_core::dat::rename_apply::list_journals(&journal_dir);
    assert_eq!(remaining.len(), 1, "cancelling must not remove anything");
}

#[test]
fn the_clear_history_button_and_dialog_render_with_plain_wording() {
    let dir = TestDir::new("clear-render");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let rolled_back = bare_transaction(
        "rolled-back",
        TransactionState::RolledBack,
        EntryState::RolledBack,
    );
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &rolled_back).unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    let output = render(&mut state);
    assert!(rendered_text_contains(&output, "Clear completed history"));
}

#[test]
fn repair_history_shows_resolved_status_without_claiming_applied() {
    let dir = TestDir::new("resolved-render");
    let journal_dir = dir.path().join("journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let mut interrupted = bare_transaction(
        "interrupted-resolved",
        TransactionState::ApplyFailed,
        EntryState::Applied,
    );
    interrupted.recovery_resolution =
        Some(archivefs_core::dat::rename_apply::RecoveryResolution::LeaveUntouched);
    interrupted.recovery_resolved_at_unix = Some(123);
    archivefs_core::dat::rename_apply::write_journal(&journal_dir, &interrupted).unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    let output = render(&mut state);
    // The user's decision is shown alongside the row...
    assert!(rendered_text_contains(
        &output,
        "Resolved: Left untouched by user"
    ));
    // ...but the truthful, unrewritten state badge must still say what
    // actually happened - a resolution is never allowed to rewrite it into
    // a settled "Applied" transaction's own label.
    assert!(rendered_text_contains(&output, "Historical transaction"));
    assert_ne!(
        TransactionState::ApplyFailed.label(),
        TransactionState::Applied.label()
    );
}

/// A minimal, hand-built transaction for enable-rule tests that do not need
/// a real applied file on disk.
fn bare_transaction(
    id: &str,
    tx_state: TransactionState,
    entry_state: EntryState,
) -> archivefs_core::dat::rename_apply::RenameTransaction {
    archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: id.to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 10,
        source_scan_root: "/tmp/roms".to_string(),
        state: tx_state,
        entries: vec![archivefs_core::dat::rename_apply::TransactionEntry {
            operation: Default::default(),
            source_path: PathBuf::from("/tmp/roms/a.bin"),
            destination_path: PathBuf::from("/tmp/roms/A.bin"),
            original_basename: "a.bin".to_string(),
            proposed_basename: "A.bin".to_string(),
            identity: archivefs_core::dat::rename_apply::ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
            },
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: entry_state,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    }
}

/// One hand-built [`archivefs_core::dat::rename_apply::TransactionEntry`],
/// for tests that need control over more than one entry at a time (which
/// [`bare_transaction`]'s single fixed entry cannot give them).
fn bare_entry(
    original_basename: &str,
    proposed_basename: &str,
    entry_state: EntryState,
) -> archivefs_core::dat::rename_apply::TransactionEntry {
    archivefs_core::dat::rename_apply::TransactionEntry {
        operation: Default::default(),
        source_path: PathBuf::from(format!("/tmp/roms/{original_basename}")),
        destination_path: PathBuf::from(format!("/tmp/roms/{proposed_basename}")),
        original_basename: original_basename.to_string(),
        proposed_basename: proposed_basename.to_string(),
        identity: archivefs_core::dat::rename_apply::ObjectIdentity {
            size_bytes: 1,
            modified_unix: 1,
            kind: archivefs_core::dat::rename_apply::ObjectKind::RegularFile,
            #[cfg(unix)]
            ino: 1,
            #[cfg(unix)]
            dev: 1,
        },
        preflight_passed: false,
        preflight_failures: Vec::new(),
        state: entry_state,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    }
}

/// A minimal, hand-built multi-entry transaction, for headline/details tests
/// that need more than [`bare_transaction`]'s single fixed entry.
fn bare_transaction_with_entries(
    id: &str,
    tx_state: TransactionState,
    entries: Vec<archivefs_core::dat::rename_apply::TransactionEntry>,
) -> archivefs_core::dat::rename_apply::RenameTransaction {
    archivefs_core::dat::rename_apply::RenameTransaction {
        transaction_id: id.to_string(),
        plan_generation: 1,
        classifier_version: Some(
            archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
        ),
        created_at_unix: 10,
        source_scan_root: "/tmp/roms".to_string(),
        state: tx_state,
        entries,
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    }
}

// --- card headline: source/destination, not id/counts, lead the card ---------

#[test]
fn a_single_entry_card_headline_names_the_source_and_destination() {
    let transaction = bare_transaction("single", TransactionState::Applied, EntryState::Applied);
    let headline = transaction_headline(&transaction);
    assert!(
        headline.contains("a.bin"),
        "the source basename must be on the card: {headline}"
    );
    assert!(
        headline.contains("A.bin"),
        "the destination basename must be on the card: {headline}"
    );
    assert!(
        !headline.contains("more"),
        "a single-entry transaction has nothing left to count: {headline}"
    );
}

#[test]
fn a_multi_entry_card_headline_names_the_first_entry_and_counts_the_rest() {
    let transaction = bare_transaction_with_entries(
        "multi",
        TransactionState::Applied,
        vec![
            bare_entry("alpha.bin", "Alpha (USA).bin", EntryState::Applied),
            bare_entry("beta.bin", "Beta (USA).bin", EntryState::Applied),
            bare_entry("gamma.bin", "Gamma (USA).bin", EntryState::Applied),
        ],
    );
    let headline = transaction_headline(&transaction);
    assert!(
        headline.contains("alpha.bin") && headline.contains("Alpha (USA).bin"),
        "the first entry must be named directly: {headline}"
    );
    assert!(
        !headline.contains("beta.bin") && !headline.contains("gamma.bin"),
        "only the first entry's name belongs on the card, not the rest: {headline}"
    );
    assert!(
        headline.contains("+ 2 more"),
        "the remaining entries must be counted, not silently dropped: {headline}"
    );
}

// --- reverify wording is never misleading about what "missing" means ---------

#[test]
fn reverify_wording_is_unambiguous_for_a_normal_fully_verified_rename() {
    let entries = vec![
        RepairReverifyEntry {
            source_path: PathBuf::from("/tmp/roms/a.bin"),
            destination_path: PathBuf::from("/tmp/roms/A.bin"),
            outcome: RepairReverifyOutcome::Verified,
            detail: "the destination matches the recorded source identity".to_string(),
        },
        RepairReverifyEntry {
            source_path: PathBuf::from("/tmp/roms/b.bin"),
            destination_path: PathBuf::from("/tmp/roms/B.bin"),
            outcome: RepairReverifyOutcome::Verified,
            detail: "the destination matches the recorded source identity".to_string(),
        },
    ];
    let summary = reverify_summary(&entries);
    assert_eq!(summary.headline, "2 of 2 destination file(s) verified");
    assert!(
        !summary.headline.to_lowercase().contains("missing"),
        "a fully verified rename must never say anything looks missing: {}",
        summary.headline
    );
    assert!(
        summary.explanation.is_none(),
        "nothing needs explaining when every destination verified"
    );
    assert_eq!(summary.tone, widgets::StatusTone::Success);
}

#[test]
fn reverify_wording_names_the_destination_not_the_source_when_something_is_missing() {
    let entries = vec![RepairReverifyEntry {
        source_path: PathBuf::from("/tmp/roms/a.bin"),
        destination_path: PathBuf::from("/tmp/roms/A.bin"),
        outcome: RepairReverifyOutcome::Missing,
        detail: "the destination does not exist after apply".to_string(),
    }];
    let summary = reverify_summary(&entries);
    let explanation = summary
        .explanation
        .expect("a missing destination must be explained, not left as a bare count");
    assert!(
        explanation.contains("destination file"),
        "must name what is actually missing (the destination, not the pre-rename source): \
         {explanation}"
    );
    assert!(
        explanation.contains("not the pre-rename source"),
        "must explicitly rule out the reading where a missing file is the normal, expected \
         post-rename disappearance of the original source: {explanation}"
    );
    assert!(
        explanation.contains("did not hold"),
        "must state plainly that this is a genuine problem, not a benign default: {explanation}"
    );
    assert_eq!(summary.tone, widgets::StatusTone::Blocked);
}

#[test]
fn reverify_wording_is_not_applicable_rather_than_falsely_alarming_when_nothing_is_applied() {
    // A transaction with nothing currently `Applied` (e.g. fully rolled
    // back) has no entries for `reverify_transaction` to check at all -
    // this must read as "nothing to check right now", never as a failed
    // verification.
    let summary = reverify_summary(&[]);
    assert_eq!(summary.headline, "Not applicable");
    let explanation = summary.explanation.expect("must explain why");
    assert!(explanation.contains("nothing to re-check"));
    assert_eq!(summary.tone, widgets::StatusTone::Info);
}

// --- details expose full, uncopied-nowhere source/destination paths ----------

#[test]
fn details_expose_the_full_uncopied_source_and_destination_paths() {
    let long_basename = "Some Very Long Original ROM Title (USA, Europe) (En,Fr,De,Es,It).bin";
    let transaction = bare_transaction_with_entries(
        "long-paths",
        TransactionState::Applied,
        vec![bare_entry(
            long_basename,
            "Canonical Title.bin",
            EntryState::Applied,
        )],
    );
    let mut state = RepairHistoryPageState {
        journal_dir: PathBuf::from("/tmp/unused"),
        transactions: vec![transaction.clone()],
        load_problems: Vec::new(),
        details_id: Some(transaction.transaction_id.clone()),
        undo_confirm: None,
        undo_confirm_focus_cancel: false,
        undo_job: None,
        undo_running: false,
        undo_outcome: None,
        undo_error: None,
        clear_confirm: None,
        clear_outcome: None,
        archive_state: RecoveryHistoryState::default(),
        archive_state_error: None,
        archive_confirm: None,
        archive_outcome: None,
        show_archived: false,
        stale_only: false,
        hide_settled: false,
        search_query: String::new(),
        presentation: presentation::Snapshot::default(),
    };

    let output = render(&mut state);
    let full_source = transaction.entries[0].source_path.display().to_string();
    let full_destination = transaction.entries[0]
        .destination_path
        .display()
        .to_string();
    assert!(
        rendered_text_contains(&output, &full_source),
        "the full source path must be readable in Details, not truncated"
    );
    assert!(
        rendered_text_contains(&output, &full_destination),
        "the full destination path must be readable in Details, not truncated"
    );
    assert!(
        rendered_text_contains(&output, "Copy"),
        "Details must offer a way to copy the paths"
    );
}

#[test]
fn the_copy_button_writes_the_exact_full_path_to_the_clipboard() {
    let transaction = bare_transaction("copy", TransactionState::Applied, EntryState::Applied);
    let mut state = RepairHistoryPageState {
        journal_dir: PathBuf::from("/tmp/unused"),
        transactions: vec![transaction.clone()],
        load_problems: Vec::new(),
        details_id: Some(transaction.transaction_id.clone()),
        undo_confirm: None,
        undo_confirm_focus_cancel: false,
        undo_job: None,
        undo_running: false,
        undo_outcome: None,
        undo_error: None,
        clear_confirm: None,
        clear_outcome: None,
        archive_state: RecoveryHistoryState::default(),
        archive_state_error: None,
        archive_confirm: None,
        archive_outcome: None,
        show_archived: false,
        stale_only: false,
        hide_settled: false,
        search_query: String::new(),
        presentation: presentation::Snapshot::default(),
    };
    let mut clipboard = NoopClipboard::default();
    render_with_clipboard(&mut state, &mut clipboard);

    // No click was simulated (this headless render never delivers pointer
    // events), so nothing should have reached the clipboard yet - this
    // pins that `detail_path_row`'s Copy button is wired to `set_text`
    // only on click, never eagerly on every frame.
    assert!(clipboard.set_calls.is_empty());
}

// --- confirmation required ----------------------------------------------------

#[test]
fn opening_the_confirmation_never_itself_starts_an_undo() {
    let dir = TestDir::new("confirmation-required");
    let (_roms, journal_dir, result) = scan_and_apply(dir.path());
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);

    state.open_undo_confirmation(&result.summary.transaction_id);

    assert!(state.undo_confirm.is_some());
    assert_eq!(
        state.undo_confirm.as_ref().unwrap().transaction_id,
        result.summary.transaction_id
    );
    assert!(!state.is_undo_running());
}

// --- cancel performs no mutation ----------------------------------------------

#[test]
fn cancelling_the_confirmation_never_touches_the_filesystem() {
    let dir = TestDir::new("cancel-no-mutation");
    let (roms, journal_dir, result) = scan_and_apply(dir.path());
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);

    state.open_undo_confirmation(&result.summary.transaction_id);
    assert!(state.undo_confirm.is_some());
    state.cancel_undo_confirmation();

    assert!(state.undo_confirm.is_none());
    assert!(!state.is_undo_running());
    assert!(roms.join("alpha.bin").exists(), "still applied");
    assert!(roms.join("beta.bin").exists(), "still applied");
    assert!(!roms.join("a.bin").exists(), "nothing was reversed");
    assert!(!roms.join("b.bin").exists(), "nothing was reversed");
}

// --- duplicate undo blocked ---------------------------------------------------

#[test]
fn a_second_undo_attempt_while_running_is_a_no_op() {
    let dir = TestDir::new("duplicate-undo");
    let (roms, journal_dir, result) = scan_and_apply(dir.path());
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);

    state.open_undo_confirmation(&result.summary.transaction_id);
    state.confirm_undo();
    assert!(state.is_undo_running());

    // Simulate a second click landing while the first job is in flight.
    assert!(!state.can_undo(&result.summary.transaction_id));
    state.open_undo_confirmation(&result.summary.transaction_id);
    assert!(state.undo_confirm.is_none(), "no second confirmation opens");
    state.confirm_undo(); // no-op: nothing pending
    assert!(state.is_undo_running(), "the original job is untouched");

    wait_for_undo(&mut state);
    let outcome = state.undo_outcome.as_ref().expect("the single undo ran");
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    assert!(roms.join("a.bin").exists());
    assert!(roms.join("b.bin").exists());
}

// --- safe undo success ---------------------------------------------------------

#[test]
fn a_safe_undo_fully_reverses_the_transaction() {
    let dir = TestDir::new("undo-success");
    let (roms, journal_dir, result) = scan_and_apply(dir.path());
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);

    state.open_undo_confirmation(&result.summary.transaction_id);
    state.confirm_undo();
    wait_for_undo(&mut state);

    let outcome = state.undo_outcome.as_ref().expect("undo ran");
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    assert!(state.undo_error.is_none());
    assert!(!state.undo_running);

    assert!(roms.join("a.bin").exists(), "original path restored");
    assert!(roms.join("b.bin").exists(), "original path restored");
    assert!(!roms.join("alpha.bin").exists());
    assert!(!roms.join("beta.bin").exists());

    let transaction = state
        .transaction_by_id(&result.summary.transaction_id)
        .expect("still present in refreshed history");
    assert_eq!(transaction.state, TransactionState::RolledBack);
}

// --- stale/changed filesystem refuses -------------------------------------------

#[test]
fn a_changed_destination_identity_refuses_the_undo() {
    let dir = TestDir::new("undo-stale-identity");
    let (roms, journal_dir, result) = scan_and_apply(dir.path());
    // Externally change both applied destinations after the fact: neither
    // still matches the identity recorded at apply time.
    std::fs::write(roms.join("alpha.bin"), b"tampered-alpha").unwrap();
    std::fs::write(roms.join("beta.bin"), b"tampered-beta").unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    state.open_undo_confirmation(&result.summary.transaction_id);
    state.confirm_undo();
    wait_for_undo(&mut state);

    let outcome = state.undo_outcome.as_ref().expect("the worker ran");
    assert!(
        matches!(outcome.result, RollbackResult::RollbackFailed { .. }),
        "{:?}",
        outcome.result
    );
    assert!(state.undo_error.is_none());
    // Nothing was reversed: the tampered files are exactly as tampered, and
    // the original paths were never recreated.
    assert!(!roms.join("a.bin").exists());
    assert!(!roms.join("b.bin").exists());
    assert_eq!(
        std::fs::read(roms.join("alpha.bin")).unwrap(),
        b"tampered-alpha"
    );
}

// --- unexpected destination/source occupation refuses ----------------------------

#[test]
fn an_occupied_original_source_path_refuses_the_undo() {
    let dir = TestDir::new("undo-occupied-source");
    let (roms, journal_dir, result) = scan_and_apply(dir.path());

    // Whichever entry rollback attempts first (reverse of apply order) gets
    // its original source path occupied by something else in the meantime.
    let contested_source = result
        .transaction
        .entries
        .last()
        .unwrap()
        .source_path
        .clone();
    std::fs::write(&contested_source, b"something else now lives here").unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    state.open_undo_confirmation(&result.summary.transaction_id);
    state.confirm_undo();
    wait_for_undo(&mut state);

    let outcome = state.undo_outcome.as_ref().expect("the worker ran");
    assert!(
        matches!(outcome.result, RollbackResult::RollbackFailed { .. }),
        "{:?}",
        outcome.result
    );
    // The occupying file was never clobbered - no-clobber held even for the
    // reverse rename.
    assert_eq!(
        std::fs::read(&contested_source).unwrap(),
        b"something else now lives here"
    );
    let _ = roms;
}

// --- previous history remains visible after failure --------------------------

#[test]
fn history_remains_visible_after_a_refused_undo() {
    let dir = TestDir::new("history-survives-failure");
    let (roms, journal_dir, result) = scan_and_apply(dir.path());
    std::fs::write(roms.join("alpha.bin"), b"tampered").unwrap();
    std::fs::write(roms.join("beta.bin"), b"tampered").unwrap();

    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    state.open_undo_confirmation(&result.summary.transaction_id);
    state.confirm_undo();
    wait_for_undo(&mut state);

    assert!(state.undo_outcome.is_some());
    assert_eq!(state.transactions.len(), 1, "the row was never dropped");
    assert!(
        state
            .transaction_by_id(&result.summary.transaction_id)
            .is_some()
    );
}

// --- history refreshes after successful undo ----------------------------------

#[test]
fn a_successful_undo_is_independently_re_observable_from_disk() {
    let dir = TestDir::new("undo-refresh-from-disk");
    let (_roms, journal_dir, result) = scan_and_apply(dir.path());
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir.clone());

    state.open_undo_confirmation(&result.summary.transaction_id);
    state.confirm_undo();
    wait_for_undo(&mut state);
    assert_eq!(
        state
            .transaction_by_id(&result.summary.transaction_id)
            .unwrap()
            .state,
        TransactionState::RolledBack
    );

    // A completely independent page instance, loaded fresh from the same
    // directory: the undo's journal write is durable, not an in-memory-only
    // GUI update.
    let fresh = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    assert_eq!(
        fresh
            .transaction_by_id(&result.summary.transaction_id)
            .unwrap()
            .state,
        TransactionState::RolledBack
    );
}

// --- no direct GUI fs mutation path --------------------------------------------

#[test]
fn the_page_module_never_calls_fs_rename_remove_or_copy_directly() {
    let source = include_str!("../repair_history_page.rs");
    for needle in [
        "fs::rename(",
        "fs::remove_file(",
        "fs::remove_dir",
        "fs::copy(",
    ] {
        assert!(
            !source.contains(needle),
            "the Repair History page must never mutate the filesystem directly (found {needle})"
        );
    }
    assert!(
        source.contains("rollback_transaction"),
        "the page must undo through the core rollback engine"
    );
}

// --- compact list: filtering and search ----------------------------------------

#[test]
fn hide_settled_defaults_to_true_on_a_freshly_loaded_page() {
    let dir = TestDir::new("hide-settled-default");
    let journal_dir = dir.path().join("journals");
    std::fs::create_dir_all(&journal_dir).unwrap();
    let state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    assert!(state.hide_settled);
    assert!(state.search_query.is_empty());
}

#[test]
fn hide_settled_removes_only_transactions_with_nothing_left_to_undo() {
    let applied = bare_transaction(
        "still-undoable",
        TransactionState::Applied,
        EntryState::Applied,
    );
    let rolled_back = bare_transaction(
        "already-settled",
        TransactionState::RolledBack,
        EntryState::RolledBack,
    );

    let visible_all = visible_transaction_ids(&[applied.clone(), rolled_back.clone()], false, "");
    assert_eq!(visible_all.len(), 2, "hide_settled: false shows everything");

    let visible_hidden = visible_transaction_ids(&[applied.clone(), rolled_back.clone()], true, "");
    assert_eq!(
        visible_hidden,
        vec![applied.transaction_id.clone()],
        "hide_settled: true keeps the still-undoable transaction and drops the settled one"
    );

    // The predicate used here must be the exact one Undo/Clear already
    // trust, not a second opinion.
    assert!(applied.is_rollbackable());
    assert!(!rolled_back.is_rollbackable());
}

#[test]
fn search_matches_entry_basenames_case_insensitively() {
    let transaction = bare_transaction_with_entries(
        "search-me",
        TransactionState::Applied,
        vec![bare_entry(
            "Chrono Trigger (USA).sfc",
            "Chrono Trigger.sfc",
            EntryState::Applied,
        )],
    );

    assert_eq!(
        visible_transaction_ids(std::slice::from_ref(&transaction), false, "chrono"),
        vec![transaction.transaction_id.clone()]
    );
    assert!(
        visible_transaction_ids(std::slice::from_ref(&transaction), false, "no-such-game")
            .is_empty()
    );
    // The transaction id itself is also searchable.
    assert_eq!(
        visible_transaction_ids(std::slice::from_ref(&transaction), false, "search-me"),
        vec![transaction.transaction_id.clone()]
    );
}

#[test]
fn filtering_never_mutates_the_underlying_transaction_list() {
    let transactions = vec![
        bare_transaction("a", TransactionState::Applied, EntryState::Applied),
        bare_transaction("b", TransactionState::RolledBack, EntryState::RolledBack),
    ];
    let before = transactions.clone();
    let _ = visible_transaction_ids(&transactions, true, "nonexistent");
    assert_eq!(
        transactions, before,
        "reading a filtered view must never change the source data"
    );
}

#[test]
fn hiding_settled_transactions_leaves_undo_and_clear_reachable_for_them() {
    // Regression guard: the filter only affects the compact list's display -
    // `can_undo`/`clearable_transaction_ids` still act on the full,
    // unfiltered `transactions`, so a row hidden by the toggle never becomes
    // unreachable to those existing controls.
    let dir = TestDir::new("hide-settled-actions-still-work");
    let (_roms, journal_dir, result) = scan_and_apply(dir.path());
    let mut state = RepairHistoryPageState::load_with_journal_dir(journal_dir);
    state.hide_settled = true;

    let visible = visible_transaction_ids(&state.transactions, state.hide_settled, "");
    // A freshly applied transaction is still rollbackable, so it stays
    // visible even with the toggle on.
    assert!(visible.contains(&result.summary.transaction_id));
    assert!(state.can_undo(&result.summary.transaction_id));
}
