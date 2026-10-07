//! History & Undo model tests. Synthetic receipts and temp directories only.

use super::*;
use archivefs_core::PersistedArchive;
use archivefs_core::dat::rename_apply::model::ObjectFreshness;

pub(in crate::gui_v2) fn identity(size: u64, mtime: i64, ino: u64, dev: u64) -> ObjectIdentity {
    ObjectIdentity {
        size_bytes: size,
        modified_unix: mtime,
        kind: ObjectKind::RegularFile,
        ino,
        dev,
        freshness: None,
    }
}

pub(in crate::gui_v2) fn item(from: &str, to: &str, state: EntryState) -> TransactionEntry {
    TransactionEntry {
        source_path: PathBuf::from(from),
        destination_path: PathBuf::from(to),
        original_basename: file_name(Path::new(from)),
        proposed_basename: file_name(Path::new(to)),
        identity: identity(10, 100, 1, 1),
        operation: TransactionOperation::RenameMove,
        preflight_passed: true,
        preflight_failures: Vec::new(),
        state,
        failure_reason: None,
        applied_at_unix: (state == EntryState::Applied).then_some(1_700_000_000),
        rolled_back_at_unix: None,
        unknown: Default::default(),
    }
}

pub(in crate::gui_v2) fn tx(
    id: &str,
    state: TransactionState,
    entries: Vec<TransactionEntry>,
) -> RenameTransaction {
    RenameTransaction {
        transaction_id: id.into(),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: 1_700_000_000,
        source_scan_root: "/library".into(),
        state,
        entries,
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    }
}

fn renames(count: usize, state: EntryState) -> Vec<TransactionEntry> {
    (0..count)
        .map(|n| {
            item(
                &format!("/library/old{n}.gba"),
                &format!("/library/New {n}.gba"),
                state,
            )
        })
        .collect()
}

fn entry_of(transaction: &RenameTransaction, source: Source) -> HistoryEntry {
    HistoryEntry::from_transaction(transaction, source, None)
}

fn sources<'a>(org: &'a [RenameTransaction], mame: &'a [RenameTransaction]) -> HistorySources<'a> {
    HistorySources {
        quarantine: &[],
        playing_library: &[],
        organisation: org,
        mame,
    }
}

fn archive(id: i64, title: &str, path: &str, platform: &str) -> PersistedArchive {
    PersistedArchive {
        id,
        source_folder_id: 1,
        relative_path: file_name(Path::new(path)).into(),
        absolute_path: path.into(),
        archive_kind: "direct_game_image".into(),
        display_name: title.into(),
        normalized_name: title.to_lowercase(),
        size_bytes: Some(10),
        modified_time_unix_seconds: Some(1),
        platform: Some(platform.into()),
        platform_source: Some("manual".into()),
        last_known_health: "pending".into(),
        last_seen_at: "2026-09-19".into(),
        last_verified_missing_at: None,
        identity_report: None,
    }
}

// ----------------------------------------------------- summaries and status

#[test]
fn a_completed_operation_gets_a_readable_summary_not_an_id() {
    let transaction = tx(
        "tx-1",
        TransactionState::Applied,
        renames(12, EntryState::Applied),
    );
    let entry = entry_of(&transaction, Source::CanonicalOrganisation);
    assert_eq!(entry.summary, "Renamed 12 files");
    assert_eq!(entry.outcome, Outcome::Completed);
    assert_eq!(entry.outcome.label(), "Completed");
    assert!(
        !entry.summary.contains("tx-1"),
        "the id is not the headline"
    );
    assert_eq!(entry.family, Family::VerificationRename);
    let one = entry_of(
        &tx(
            "tx-2",
            TransactionState::Applied,
            renames(1, EntryState::Applied),
        ),
        Source::CanonicalOrganisation,
    );
    assert_eq!(one.summary, "Renamed 1 file");
}

#[test]
fn summaries_follow_the_recorded_operation_not_message_text() {
    let mut mame = tx(
        "m",
        TransactionState::Applied,
        renames(1, EntryState::Applied),
    );
    mame.unknown.insert("mame_parent".into(), "pacman".into());
    assert_eq!(
        entry_of(&mame, Source::MameReconstruction).summary,
        "Rebuilt MAME set pacman"
    );
    assert_eq!(
        entry_of(&mame, Source::MameReconstruction).family,
        Family::Mame
    );
    let library = tx(
        "p",
        TransactionState::Applied,
        renames(3, EntryState::Applied),
    );
    assert_eq!(
        entry_of(&library, Source::PlayingLibrary).summary,
        "Built a Playing Library (3 links)"
    );
    let quarantine = tx(
        "q",
        TransactionState::Applied,
        renames(2, EntryState::Applied),
    );
    let q = entry_of(&quarantine, Source::DuplicateQuarantine);
    assert_eq!(q.summary, "Moved 2 redundant files to quarantine");
    assert_eq!(q.family, Family::Repair);
    // A moved (different-folder) rename is "Moved", a same-folder one "Renamed".
    let mut moved = renames(2, EntryState::Applied);
    moved[0].destination_path = PathBuf::from("/elsewhere/New.gba");
    let moved = tx("mv", TransactionState::Applied, moved);
    assert_eq!(
        entry_of(&moved, Source::CanonicalOrganisation).summary,
        "Moved 2 files"
    );
    // Link operations are organisation, not rename.
    let mut linked = renames(1, EntryState::Applied);
    linked[0].operation = TransactionOperation::CreateSymlink {
        expected_target: PathBuf::from("/library/old0.gba"),
        destination_root: PathBuf::from("/links"),
    };
    let linked = entry_of(
        &tx("l", TransactionState::Applied, linked),
        Source::CanonicalOrganisation,
    );
    assert_eq!(linked.family, Family::Organisation);
}

#[test]
fn a_failed_operation_is_distinct_from_a_completed_one() {
    let completed = entry_of(
        &tx(
            "a",
            TransactionState::Applied,
            renames(2, EntryState::Applied),
        ),
        Source::CanonicalOrganisation,
    );
    let failed = entry_of(
        &tx(
            "b",
            TransactionState::ApplyFailed,
            renames(2, EntryState::ApplyFailed),
        ),
        Source::CanonicalOrganisation,
    );
    assert_eq!(failed.outcome, Outcome::Failed);
    assert_ne!(failed.outcome.label(), completed.outcome.label());
    assert!(matches!(failed.undo, UndoStatus::Unavailable(_)));
    let partial = entry_of(
        &tx(
            "c",
            TransactionState::ApplyFailed,
            vec![
                item("/library/a.gba", "/library/A.gba", EntryState::Applied),
                item("/library/b.gba", "/library/B.gba", EntryState::ApplyFailed),
            ],
        ),
        Source::CanonicalOrganisation,
    );
    assert_eq!(
        partial.outcome,
        Outcome::StoppedPartway {
            applied: 1,
            total: 2
        }
    );
    assert_eq!(partial.outcome.label(), "Stopped partway (1 of 2 done)");
}

#[test]
fn a_rolled_back_operation_is_already_undone_with_no_active_undo() {
    let mut entries = renames(2, EntryState::RolledBack);
    entries[0].rolled_back_at_unix = Some(1_700_100_000);
    let entry = entry_of(
        &tx("r", TransactionState::RolledBack, entries),
        Source::CanonicalOrganisation,
    );
    assert_eq!(entry.outcome, Outcome::Undone);
    assert_eq!(entry.undo, UndoStatus::AlreadyUndone);
    assert_eq!(entry.undo.label(), "Already undone");
    assert_eq!(entry.undone_when, Some(1_700_100_000));
}

#[test]
fn undo_status_comes_from_the_receipt_and_never_invents_availability() {
    let applied = entry_of(
        &tx(
            "a",
            TransactionState::Applied,
            renames(1, EntryState::Applied),
        ),
        Source::PlayingLibrary,
    );
    assert_eq!(applied.undo, UndoStatus::Available);
    // "Applied" with nothing actually applied cannot be undone.
    let empty = entry_of(
        &tx(
            "e",
            TransactionState::Applied,
            renames(1, EntryState::Skipped),
        ),
        Source::PlayingLibrary,
    );
    assert!(
        matches!(empty.undo, UndoStatus::Unavailable(ref why) if why.contains("no applied changes"))
    );
    let failed_undo = entry_of(
        &tx(
            "f",
            TransactionState::RollbackFailed,
            renames(1, EntryState::RollbackFailed),
        ),
        Source::PlayingLibrary,
    );
    assert!(matches!(failed_undo.undo, UndoStatus::NeedsReview(_)));
    assert_eq!(failed_undo.outcome, Outcome::NeedsAttention);
    let interrupted = entry_of(
        &tx(
            "i",
            TransactionState::Applying,
            renames(1, EntryState::Applying),
        ),
        Source::PlayingLibrary,
    );
    assert!(matches!(interrupted.undo, UndoStatus::NeedsReview(_)));
}

#[test]
fn time_is_formatted_only_when_recorded() {
    assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13 UTC");
    let mut transaction = tx(
        "t",
        TransactionState::Applied,
        renames(1, EntryState::Applied),
    );
    transaction.created_at_unix = 0;
    transaction.entries[0].applied_at_unix = None;
    let entry = entry_of(&transaction, Source::CanonicalOrganisation);
    assert_eq!(entry.when, None, "a missing timestamp is not fabricated");
    transaction.created_at_unix = 1_700_000_000;
    assert_eq!(
        entry_of(&transaction, Source::CanonicalOrganisation).when,
        Some(1_700_000_000)
    );
}

// ------------------------------------------------------------ failures

#[test]
fn a_failed_operation_keeps_source_safety_and_cleanup_evidence_it_actually_has() {
    let mut entries = vec![
        item("/library/a.gba", "/library/A.gba", EntryState::RolledBack),
        item("/library/b.gba", "/library/B.gba", EntryState::ApplyFailed),
    ];
    entries[1].failure_reason = Some("the destination appeared".into());
    let entry = entry_of(
        &tx("p", TransactionState::ApplyFailed, entries),
        Source::CanonicalOrganisation,
    );
    let facts = entry.failure.expect("a failure explains itself");
    assert!(facts.attempted.contains("2 items"));
    assert!(
        facts
            .changed
            .contains("1 of 2 were changed and then reversed"),
        "{}",
        facts.changed
    );
    assert!(
        facts.cleanup.contains("1 change reversed"),
        "{}",
        facts.cleanup
    );
    assert!(
        facts.cleanup.contains("not recorded"),
        "unrecorded cleanup is not claimed"
    );
    assert_eq!(facts.reason.as_deref(), Some("the destination appeared"));
    // A clean failure states nothing was changed, and invents no cleanup.
    let clean = entry_of(
        &tx(
            "c",
            TransactionState::ApplyFailed,
            renames(2, EntryState::ApplyFailed),
        ),
        Source::CanonicalOrganisation,
    );
    let facts = clean.failure.unwrap();
    assert!(facts.changed.contains("No file was changed"));
    assert!(facts.cleanup.contains("not recorded"));
    // A completed operation has no failure block.
    assert!(
        entry_of(
            &tx(
                "ok",
                TransactionState::Applied,
                renames(1, EntryState::Applied)
            ),
            Source::CanonicalOrganisation
        )
        .failure
        .is_none()
    );
}

// ------------------------------------------------------- game association

#[test]
fn only_an_exact_path_match_associates_a_receipt_with_a_game() {
    let library = Library::new(vec![
        archive(1, "Mario Kart", "/library/New 0.gba", "Game Boy Advance"),
        archive(2, "Zelda", "/library/Zelda.gba", "Game Boy Advance"),
    ]);
    let exact = tx(
        "exact",
        TransactionState::Applied,
        renames(1, EntryState::Applied),
    );
    // Same title words, different directory: resemblance is not evidence.
    let lookalike = tx(
        "lookalike",
        TransactionState::Applied,
        vec![item(
            "/other/zelda.gba",
            "/other/Zelda.gba",
            EntryState::Applied,
        )],
    );
    let unrelated = tx(
        "unrelated",
        TransactionState::Applied,
        vec![item("/x/a.gba", "/x/b.gba", EntryState::Applied)],
    );
    let org = vec![exact, lookalike, unrelated];
    let entries = build_entries(&sources(&org, &[]), Some(&library));
    let by_id = |id: &str| entries.iter().find(|e| e.transaction_id == id).unwrap();
    assert_eq!(by_id("exact").games.len(), 1);
    assert_eq!(by_id("exact").games[0].title, "Mario Kart");
    assert_eq!(by_id("exact").games[0].platform, "Game Boy Advance");
    assert!(
        by_id("lookalike").games.is_empty(),
        "a lookalike is never guessed"
    );
    assert!(by_id("unrelated").games.is_empty());
    // Without a library there is no association at all.
    assert!(build_entries(&sources(&org, &[]), None)[0].games.is_empty());
}

#[test]
fn game_scoped_history_contains_only_explicitly_associated_receipts() {
    let library = Library::new(vec![archive(
        1,
        "Mario Kart",
        "/library/New 0.gba",
        "Game Boy Advance",
    )]);
    let org = vec![
        tx(
            "exact",
            TransactionState::Applied,
            renames(1, EntryState::Applied),
        ),
        tx(
            "other",
            TransactionState::Applied,
            vec![item("/x/a.gba", "/x/b.gba", EntryState::Applied)],
        ),
    ];
    let entries = build_entries(&sources(&org, &[]), Some(&library));
    let state = HistoryViewState::default();
    let scoped = filter_entries(&entries, &state, Some(1)).unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].transaction_id, "exact");
    // All History is still the whole list.
    assert_eq!(filter_entries(&entries, &state, None).unwrap().len(), 2);
    // A game with no receipts gets its own empty cause, not a generic one.
    assert_eq!(
        filter_entries(&entries, &state, Some(99)).unwrap_err(),
        EmptyCause::NoGameHistory
    );
}

// ------------------------------------------------------ filters and search

#[test]
fn filters_and_search_affect_presentation_only() {
    let org = vec![
        tx(
            "done",
            TransactionState::Applied,
            renames(2, EntryState::Applied),
        ),
        tx(
            "bad",
            TransactionState::ApplyFailed,
            renames(2, EntryState::ApplyFailed),
        ),
        tx(
            "undone",
            TransactionState::RolledBack,
            renames(2, EntryState::RolledBack),
        ),
    ];
    let mut mame = tx(
        "m",
        TransactionState::Applied,
        renames(1, EntryState::Applied),
    );
    mame.unknown.insert("mame_parent".into(), "pacman".into());
    let mame = vec![mame];
    let entries = build_entries(&sources(&org, &mame), None);
    let before = entries.clone();
    let ids = |state: &HistoryViewState| -> Vec<String> {
        filter_entries(&entries, state, None)
            .map(|rows| rows.iter().map(|e| e.transaction_id.clone()).collect())
            .unwrap_or_default()
    };
    let mut state = HistoryViewState::default();
    assert_eq!(ids(&state).len(), 4);
    state.status = StatusFilter::UndoAvailable;
    let mut available = ids(&state);
    available.sort();
    assert_eq!(available, vec!["done", "m"]);
    state.status = StatusFilter::Failed;
    assert_eq!(ids(&state), vec!["bad"]);
    state.status = StatusFilter::Completed;
    assert_eq!(ids(&state).len(), 2);
    state.status = StatusFilter::All;
    state.family = Some(Family::Mame);
    assert_eq!(ids(&state), vec!["m"]);
    state.family = None;
    state.search = "PACMAN".into();
    assert_eq!(ids(&state), vec!["m"]);
    state.search = "renamed".into();
    assert_eq!(ids(&state).len(), 3);
    assert_eq!(entries, before, "no filter or search changed any history");
}

#[test]
fn each_empty_cause_has_its_own_words() {
    let none: Vec<HistoryEntry> = Vec::new();
    let state = HistoryViewState::default();
    assert_eq!(
        filter_entries(&none, &state, None).unwrap_err(),
        EmptyCause::NoHistory
    );

    let org = vec![tx(
        "u",
        TransactionState::RolledBack,
        renames(1, EntryState::RolledBack),
    )];
    let entries = build_entries(&sources(&org, &[]), None);
    let status = |status| HistoryViewState {
        status,
        ..Default::default()
    };
    assert_eq!(
        filter_entries(&entries, &status(StatusFilter::UndoAvailable), None).unwrap_err(),
        EmptyCause::NoUndoable
    );
    assert_eq!(
        filter_entries(&entries, &status(StatusFilter::Failed), None).unwrap_err(),
        EmptyCause::NoFailed
    );
    let searching = HistoryViewState {
        search: "zzz".into(),
        ..Default::default()
    };
    assert_eq!(
        filter_entries(&entries, &searching, None).unwrap_err(),
        EmptyCause::NoMatches
    );

    let causes = [
        EmptyCause::NoHistory,
        EmptyCause::NoUndoable,
        EmptyCause::NoFailed,
        EmptyCause::NoMatches,
        EmptyCause::NoGameHistory,
    ];
    let titles: HashSet<&str> = causes.iter().map(|c| empty_copy(*c).0).collect();
    assert_eq!(titles.len(), causes.len(), "no two causes share a message");
    assert!(titles.iter().all(|t| *t != "Nothing here"));
}

// ------------------------------------------------------------ handoffs

#[test]
fn workflow_handoffs_resolve_to_the_canonical_destination() {
    let route_of = |source, family| handoff_for(source, family).route;
    assert_eq!(
        route_of(Source::CanonicalOrganisation, Family::VerificationRename),
        Route::Section(Section::DatVerification)
    );
    assert_eq!(
        route_of(Source::MameReconstruction, Family::Mame),
        Route::MameWorkflow
    );
    assert_eq!(
        route_of(Source::DuplicateQuarantine, Family::Repair),
        Route::Section(Section::Duplicates)
    );
    assert_eq!(
        route_of(Source::PlayingLibrary, Family::Organisation),
        Route::Section(Section::Build)
    );
}

// -------------------------------------------------------------- advanced

#[test]
fn advanced_keeps_ids_paths_hashes_and_provenance() {
    let mut entries = renames(1, EntryState::Applied);
    entries[0].identity.freshness = Some(ObjectFreshness {
        version: 1,
        modified: std::time::SystemTime::UNIX_EPOCH,
        sha256: [0xab; 32],
    });
    let mut transaction = tx("tx-adv", TransactionState::Applied, entries);
    transaction
        .unknown
        .insert("mame_parent".into(), "pacman".into());
    let entry = HistoryEntry::from_transaction(
        &transaction,
        Source::MameReconstruction,
        Some(Path::new("/journal")),
    );
    let flat: String = entry
        .advanced
        .iter()
        .map(|(label, value)| format!("{label}={value}\n"))
        .collect();
    for expected in [
        "Transaction ID=tx-adv",
        "Journal folder=/journal",
        "Provenance: mame_parent=",
        "Item 1 from=/library/old0.gba",
        "Item 1 to=/library/New 0.gba",
        &format!("Item 1 SHA-256={}", "ab".repeat(32)),
        "Created (unix)=1700000000",
    ] {
        assert!(
            flat.contains(expected),
            "Advanced is missing {expected}\n{flat}"
        );
    }
}

#[test]
fn advanced_per_item_detail_is_bounded() {
    let transaction = tx(
        "big",
        TransactionState::Applied,
        renames(500, EntryState::Applied),
    );
    let entry = entry_of(&transaction, Source::CanonicalOrganisation);
    assert!(
        entry
            .advanced
            .iter()
            .any(|(label, _)| label == "More items")
    );
    assert!(entry.advanced.len() < 400);
}

// -------------------------------------------------------- undo preview

fn real_output(directory: &Path) -> (PathBuf, PathBuf, TransactionEntry) {
    use std::os::unix::fs::MetadataExt;
    let source = directory.join("old.gba");
    let destination = directory.join("New.gba");
    std::fs::write(&destination, b"renamed bytes").unwrap();
    let metadata = std::fs::metadata(&destination).unwrap();
    let mut entry = item(
        &source.to_string_lossy(),
        &destination.to_string_lossy(),
        EntryState::Applied,
    );
    entry.identity = identity(
        metadata.len(),
        metadata.mtime(),
        metadata.ino(),
        metadata.dev(),
    );
    (source, destination, entry)
}

#[test]
fn an_unchanged_output_previews_as_safe_and_names_what_undo_would_do() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, entry) = real_output(dir.path());
    let history = entry_of(
        &tx("t", TransactionState::Applied, vec![entry]),
        Source::CanonicalOrganisation,
    );
    let preview = check_undo_safety(&history);
    assert!(preview.safe(), "{:?}", preview.blockers);
    assert_eq!(preview.checked, 1);
    assert!(preview.will_change[0].contains("back at the original name"));
    assert!(preview.receipt.contains("t"));
    assert_eq!(preview.operation, "Renamed 1 file");
}

#[test]
fn a_changed_output_is_a_visible_blocker_with_a_plain_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (_, destination, entry) = real_output(dir.path());
    let history = entry_of(
        &tx("t", TransactionState::Applied, vec![entry]),
        Source::CanonicalOrganisation,
    );
    std::fs::write(&destination, b"edited after the rename, longer").unwrap();
    let preview = check_undo_safety(&history);
    assert!(!preview.safe());
    assert!(matches!(preview.blockers[0], Blocker::OutputChanged(_)));
    assert_eq!(
        preview.unavailable_because().as_deref(),
        Some("Undo is unavailable because the output has changed.")
    );
    // A missing destination and an occupied original are distinct blockers.
    std::fs::remove_file(&destination).unwrap();
    assert!(matches!(
        check_undo_safety(&history).blockers[0],
        Blocker::OutputMissing(_)
    ));
}

#[test]
fn an_occupied_original_path_blocks_the_preview() {
    let dir = tempfile::tempdir().unwrap();
    let (source, _, entry) = real_output(dir.path());
    let history = entry_of(
        &tx("t", TransactionState::Applied, vec![entry]),
        Source::CanonicalOrganisation,
    );
    std::fs::write(&source, b"someone made this").unwrap();
    let preview = check_undo_safety(&history);
    assert!(matches!(
        preview.blockers[0],
        Blocker::OriginalPathOccupied(_)
    ));
    assert!(
        preview
            .unavailable_because()
            .unwrap()
            .contains("original location")
    );
}

#[test]
fn the_preview_is_read_only_and_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let (source, destination, entry) = real_output(dir.path());
    let before = std::fs::read(&destination).unwrap();
    let mut many = vec![entry.clone()];
    for _ in 0..(MAX_EVIDENCE_ENTRIES + 25) {
        many.push(entry.clone());
    }
    let history = entry_of(
        &tx("t", TransactionState::Applied, many),
        Source::CanonicalOrganisation,
    );
    let preview = check_undo_safety(&history);
    assert_eq!(preview.checked, MAX_EVIDENCE_ENTRIES);
    assert_eq!(preview.not_checked, 26);
    assert!(
        preview
            .warnings
            .iter()
            .any(|w| w.contains("re-verified when you undo"))
    );
    assert_eq!(std::fs::read(&destination).unwrap(), before);
    assert!(!source.exists(), "previewing restored nothing");
}

#[test]
fn model_building_does_no_filesystem_access() {
    // Receipts that point at paths which cannot exist still build instantly and
    // identically: the model reads only the receipt.
    let org = vec![tx(
        "ghost",
        TransactionState::Applied,
        vec![item(
            "/definitely/not/here/a.gba",
            "/definitely/not/here/B.gba",
            EntryState::Applied,
        )],
    )];
    let first = build_entries(&sources(&org, &[]), None);
    let second = build_entries(&sources(&org, &[]), None);
    assert_eq!(first, second);
    assert_eq!(
        fingerprint(&sources(&org, &[]), 0),
        fingerprint(&sources(&org, &[]), 0)
    );
    assert_ne!(
        fingerprint(&sources(&org, &[]), 0),
        fingerprint(&sources(&org, &[]), 5)
    );
}

#[test]
fn row_ids_are_the_unique_transaction_ids() {
    let org = vec![
        tx(
            "same-words-1",
            TransactionState::Applied,
            renames(2, EntryState::Applied),
        ),
        tx(
            "same-words-2",
            TransactionState::Applied,
            renames(2, EntryState::Applied),
        ),
    ];
    let entries = build_entries(&sources(&org, &[]), None);
    assert_eq!(entries[0].summary, entries[1].summary, "identical wording");
    let ids: HashSet<&str> = entries.iter().map(|e| e.transaction_id.as_str()).collect();
    assert_eq!(ids.len(), 2, "rows are told apart by id, not by wording");
    let source = include_str!("../history_view.rs");
    assert!(source.contains("ui.push_id((\"history-row\", &entry.transaction_id)"));
    assert!(source.contains("egui::Grid::new((\"history-advanced\", &entry.transaction_id))"));
}

#[test]
fn history_view_contains_no_undo_engine_or_mutation() {
    let source = include_str!("../history_view.rs");
    let code = source.split("#[cfg(test)]").next().unwrap();
    for forbidden in [
        "rollback_transaction",
        "apply_transaction",
        "fs::rename",
        "fs::remove",
        "fs::write",
        "fs::copy",
        "rename_noreplace",
        "write_journal",
        "Force",
        "force_undo",
    ] {
        assert!(
            !code.contains(forbidden),
            "history view must not contain {forbidden}"
        );
    }
}
