//! History aggregates sealed and legacy Undo records. Legacy, unsealed,
//! changed and unreadable records stay visible, read-only, under their own
//! status; opening History writes nothing.
use super::*;
use archivefs_core::dat::rename_apply::model::{
    EntryState, RenameTransaction, TransactionEntry, TransactionState,
};
use archivefs_core::dat::rename_apply::{journal_path, seal, write_journal};
use std::path::PathBuf;

fn record(id: &str, at: u64, state: TransactionState, file: &str) -> RenameTransaction {
    let entry = TransactionEntry {
        source_path: PathBuf::from(format!("/library/old/{file}")),
        destination_path: PathBuf::from(format!("/library/new/{file}")),
        original_basename: file.into(),
        proposed_basename: file.into(),
        identity: archivefs_core::dat::rename_apply::model::ObjectIdentity {
            size_bytes: 1,
            modified_unix: 1,
            kind: archivefs_core::dat::rename_apply::model::ObjectKind::RegularFile,
            ino: 1,
            dev: 1,
            freshness: None,
        },
        operation: Default::default(),
        preflight_passed: true,
        preflight_failures: Vec::new(),
        state: if state == TransactionState::RolledBack {
            EntryState::RolledBack
        } else {
            EntryState::Applied
        },
        failure_reason: None,
        applied_at_unix: Some(at),
        rolled_back_at_unix: None,
        unknown: Default::default(),
    };
    RenameTransaction {
        transaction_id: id.into(),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: at,
        source_scan_root: "/library".into(),
        state,
        entries: vec![entry],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    }
}

struct Estate {
    _dir: tempfile::TempDir,
    sealed: PathBuf,
    legacy: PathBuf,
}

/// A sealed folder (two sealed records, one unsealed file, one changed
/// record) and a legacy folder (137 records, one malformed file, and one
/// record whose id also exists in the sealed folder).
fn estate() -> Estate {
    let dir = tempfile::tempdir().unwrap();
    let sealed = dir.path().join("rename-transactions-sealed-v1");
    let legacy = dir.path().join("rename-transactions");
    fs::create_dir_all(&sealed).unwrap();
    fs::create_dir_all(&legacy).unwrap();
    for (id, at) in [("shared-id", 2_000_000_000), ("sealed-two", 2_000_000_001)] {
        write_journal(
            &sealed,
            &record(id, at, TransactionState::Applied, &format!("{id}.bin")),
        )
        .unwrap();
    }
    let changed = record(
        "changed",
        2_000_000_002,
        TransactionState::Applied,
        "changed.bin",
    );
    write_journal(&sealed, &changed).unwrap();
    let mut edited = changed.clone();
    edited.entries[0].failure_reason = Some("edited elsewhere".into());
    fs::write(
        journal_path(&sealed, "changed").unwrap(),
        serde_json::to_string(&edited).unwrap(),
    )
    .unwrap();
    fs::write(
        journal_path(&sealed, "no-seal").unwrap(),
        serde_json::to_string(&record("no-seal", 3, TransactionState::Applied, "n.bin")).unwrap(),
    )
    .unwrap();
    for index in 0..137u64 {
        let state = match index % 3 {
            0 => TransactionState::Applied,
            1 => TransactionState::RolledBack,
            _ => TransactionState::RollbackFailed,
        };
        let id = format!("legacy-{index:03}");
        fs::write(
            journal_path(&legacy, &id).unwrap(),
            serde_json::to_string_pretty(&record(
                &id,
                1_600_000_000 + index,
                state,
                &format!("legacy-game-{index:03}.zip"),
            ))
            .unwrap(),
        )
        .unwrap();
    }
    fs::write(legacy.join("broken.json"), b"{").unwrap();
    fs::write(
        journal_path(&legacy, "shared-id").unwrap(),
        serde_json::to_string(&record(
            "shared-id",
            1_500_000_000,
            TransactionState::Applied,
            "older-shared.bin",
        ))
        .unwrap(),
    )
    .unwrap();
    Estate {
        _dir: dir,
        sealed,
        legacy,
    }
}

/// Every file in this test's folders, byte for byte, and how each record in
/// them verifies against the (shared) seal store. Other tests may write their
/// own seals concurrently, so the store file itself is not compared.
fn snapshot(estate: &Estate) -> Vec<(PathBuf, Vec<u8>, String)> {
    let mut files: Vec<(PathBuf, Vec<u8>, String)> = [&estate.sealed, &estate.legacy]
        .iter()
        .flat_map(|dir| fs::read_dir(dir).unwrap().map(move |entry| (dir, entry)))
        .map(|(dir, entry)| {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            let id = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let check = format!("{:?}", seal::verify(dir, &id));
            (path, bytes, check)
        })
        .collect();
    files.sort();
    files
}

fn loaded(estate: &Estate) -> super::super::backend::Payload {
    super::super::backend::load_repair_history(&estate.sealed, &[estate.legacy.clone()])
}

#[test]
fn history_shows_sealed_and_all_legacy_records_and_writes_nothing() {
    let estate = estate();
    let before = snapshot(&estate);
    let super::super::backend::Payload::RepairHistory {
        organisations,
        evidence,
        ..
    } = loaded(&estate)
    else {
        panic!("history payload");
    };
    assert_eq!(snapshot(&estate), before, "opening History wrote nothing");

    // Only verified sealed records can offer Undo.
    let mut trusted: Vec<_> = organisations
        .iter()
        .map(|t| t.transaction_id.as_str())
        .collect();
    trusted.sort();
    assert_eq!(trusted, vec!["sealed-two", "shared-id"]);

    let status = |predicate: fn(&seal::EvidenceStatus) -> bool| {
        evidence
            .iter()
            .filter(|item| predicate(&item.status))
            .count()
    };
    assert_eq!(
        status(|s| matches!(s, seal::EvidenceStatus::Legacy)),
        138,
        "137 legacy records plus the legacy copy of a shared id"
    );
    assert_eq!(status(|s| matches!(s, seal::EvidenceStatus::Corrupt(_))), 1);
    assert_eq!(
        status(|s| matches!(s, seal::EvidenceStatus::MissingSeal)),
        1
    );
    assert_eq!(
        status(|s| matches!(s, seal::EvidenceStatus::IntegrityMismatch(_))),
        1
    );
    // The same id in both folders is two separate records.
    let shared: Vec<_> = evidence
        .iter()
        .filter(|item| item.file_id == "shared-id")
        .collect();
    assert_eq!(shared.len(), 1);
    assert!(shared[0].legacy_namespace);
}

#[test]
fn legacy_records_keep_their_facts_are_searchable_and_never_offer_undo() {
    let estate = estate();
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let super::super::backend::Payload::RepairHistory {
        organisations,
        evidence,
        ..
    } = loaded(&estate)
    else {
        panic!("history payload");
    };
    app.canonical_organisation_history = organisations;
    app.journal_evidence = evidence;
    app.refresh_history_model();
    let entries = app.history_view.entries.clone();
    assert_eq!(entries.len(), 2 + 141);

    let legacy: Vec<_> = entries
        .iter()
        .filter(|entry| {
            entry
                .summary
                .starts_with("Legacy recovery record — automatic Undo unavailable until reviewed")
        })
        .collect();
    assert_eq!(legacy.len(), 138);
    for entry in &legacy {
        assert!(matches!(
            entry.undo,
            super::super::history_view::UndoStatus::Unavailable(_)
        ));
        assert!(entry.when.is_some(), "the original timestamp is kept");
    }
    // Original outcome and summary survive.
    let undone = entries
        .iter()
        .find(|entry| entry.transaction.transaction_id == "legacy-001")
        .unwrap();
    assert_eq!(undone.outcome, super::super::history_view::Outcome::Undone);
    assert_eq!(undone.when, Some(1_600_000_001));
    // Both records sharing an id are present under distinct keys.
    let shared: Vec<_> = entries
        .iter()
        .filter(|entry| entry.transaction.transaction_id == "shared-id")
        .collect();
    assert_eq!(shared.len(), 2);
    assert_ne!(shared[0].transaction_id, shared[1].transaction_id);
    // Unreadable evidence is visible.
    assert!(entries.iter().any(|entry| {
        entry.summary.starts_with("Unreadable record")
            && entry
                .target
                .as_deref()
                .is_some_and(|t| t.ends_with("broken.json"))
    }));

    // Search and filters include them.
    let mut view = app.history_view.clone();
    view.search = "legacy recovery record".into();
    let found = super::super::history_view::filter_entries(&entries, &view, None).unwrap();
    assert_eq!(found.len(), 138);
    view.search = "legacy-game-042".into();
    let found = super::super::history_view::filter_entries(&entries, &view, None).unwrap();
    assert_eq!(found.len(), 1);
    view.search.clear();
    view.status = super::super::history_view::StatusFilter::UndoAvailable;
    let undoable = super::super::history_view::filter_entries(&entries, &view, None).unwrap();
    assert!(
        undoable
            .iter()
            .all(|entry| !entry.transaction_id.starts_with("evidence:"))
    );
    assert_eq!(undoable.len(), 2);
}
