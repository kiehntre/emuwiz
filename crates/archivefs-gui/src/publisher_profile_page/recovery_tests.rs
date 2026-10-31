//! Durable Publisher rollback through real Apply, sealed journals and rollback.
//! Each test uses disposable directories; a new page state stands in for a
//! restart because the rollback handle is held only in memory.

use std::path::{Path, PathBuf};

use archivefs_core::dat::rename_apply::{EntryState, TransactionState, journal};
use archivefs_core::publisher_profile::recovery::{PUBLISHER_EXPORT_MARKER, is_publisher_export};

use super::tests::{elected, page_labels, plan_with};
use super::*;

struct Export {
    state: PublisherProfilePageState,
    journal: PathBuf,
    sources: Vec<PathBuf>,
    links: Vec<PathBuf>,
    _temp: tempfile::TempDir,
}

fn exported(mode: PublisherLinkMode) -> Export {
    let temp = tempfile::tempdir().unwrap();
    let sources = vec![temp.path().join("Game.rom"), temp.path().join("Other.rom")];
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    for source in &sources {
        std::fs::write(source, source.display().to_string()).unwrap();
    }
    std::fs::create_dir(&destination).unwrap();
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: destination.display().to_string(),
        source_plan: Some(plan_with(vec![
            elected("Game", &sources[0].display().to_string()),
            elected("Other", &sources[1].display().to_string()),
        ])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    state.link_mode = Some(mode);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.apply_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::Applied);
    let links = state
        .execution_transaction
        .as_ref()
        .unwrap()
        .transaction
        .entries
        .iter()
        .map(|entry| entry.destination_path.clone())
        .collect();
    Export {
        state,
        journal,
        sources,
        links,
        _temp: temp,
    }
}

/// What a restarted EmuWiz sees: nothing in memory but the journal folder.
fn restarted(journal: &Path) -> PublisherProfilePageState {
    let mut state = PublisherProfilePageState::default();
    state.recovery.note_journal_dir(journal);
    state.load_recoverable_exports();
    state
}

fn roll_back_listed(state: &mut PublisherProfilePageState) {
    let export = state.recovery.exports[0].clone();
    state.recovery.selected = Some(export.transaction.transaction_id.clone());
    state.recovery.confirmation_text = format!("ROLL BACK {} ITEMS", export.item_count);
    state.rollback_recovered();
}

fn message(state: &PublisherProfilePageState) -> String {
    state.recovery.message.clone().unwrap_or_default()
}

fn present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

#[test]
fn restart_recovers_a_valid_undo_in_both_link_modes() {
    for mode in [PublisherLinkMode::Symlink, PublisherLinkMode::Hardlink] {
        let export = exported(mode);
        let mut state = restarted(&export.journal);
        assert_eq!(state.recovery.exports.len(), 1, "{mode:?}");
        assert_eq!(state.recovery.exports[0].item_count, 2, "{mode:?}");
        assert_eq!(state.recovery.exports[0].blocker, None, "{mode:?}");
        let labels = page_labels(&mut state);
        assert!(labels.contains("Exports you can still undo"), "{labels}");

        roll_back_listed(&mut state);
        assert!(
            message(&state).contains("rolled back"),
            "{}",
            message(&state)
        );
        for link in &export.links {
            assert!(!present(link), "{mode:?}");
        }
        for source in &export.sources {
            assert_eq!(
                std::fs::read_to_string(source).unwrap(),
                source.display().to_string()
            );
        }
        assert!(state.recovery.exports.is_empty(), "{mode:?}");
    }
}

#[test]
fn preview_keeps_the_export_recoverable_and_says_so() {
    let mut export = exported(PublisherLinkMode::Symlink);
    export.state.preview();
    assert!(export.state.execution_transaction.is_none());
    assert_eq!(export.state.recovery.exports.len(), 1);
    assert!(message(&export.state).contains("stays listed"));
    let labels = page_labels(&mut export.state);
    assert!(labels.contains("Exports you can still undo"), "{labels}");
    roll_back_listed(&mut export.state);
    assert!(!present(&export.links[0]));
}

#[test]
fn changing_the_source_plan_keeps_the_export_recoverable() {
    let mut export = exported(PublisherLinkMode::Symlink);
    export.state.set_source_plan(None);
    assert!(export.state.execution_transaction.is_none());
    assert_eq!(export.state.recovery.exports.len(), 1);
}

#[test]
fn preview_refuses_to_drop_a_rollback_it_cannot_recover_later() {
    let mut export = exported(PublisherLinkMode::Symlink);
    let id = export
        .state
        .execution_transaction
        .as_ref()
        .unwrap()
        .transaction
        .transaction_id
        .clone();
    std::fs::remove_file(journal::journal_path(&export.journal, &id).unwrap()).unwrap();
    export.state.preview();
    assert!(export.state.execution_transaction.is_some(), "handle kept");
    assert_eq!(
        export.state.execution_stage,
        PublisherExecutionStage::Applied
    );
    let error = export.state.execution_error.clone().expect("explained");
    assert!(error.contains("Preview was not run"), "{error}");

    export.state.set_source_plan(None);
    assert!(export.state.source_plan.is_some(), "plan kept");
    assert!(export.state.execution_transaction.is_some(), "handle kept");
}

#[test]
fn modified_or_replaced_links_are_refused_and_preserved() {
    let plant: [(&str, fn(&Path)); 2] = [
        ("replaced by a user file", |link| {
            std::fs::remove_file(link).unwrap();
            std::fs::write(link, b"user data").unwrap();
        }),
        ("repointed elsewhere", |link| {
            std::fs::remove_file(link).unwrap();
            std::os::unix::fs::symlink("/somewhere/else", link).unwrap();
        }),
    ];
    for (name, change) in plant {
        let export = exported(PublisherLinkMode::Symlink);
        change(&export.links[0]);
        let before = std::fs::symlink_metadata(&export.links[0]).unwrap().len();
        let mut state = restarted(&export.journal);
        roll_back_listed(&mut state);
        assert!(present(&export.links[0]), "{name}");
        assert_eq!(
            std::fs::symlink_metadata(&export.links[0]).unwrap().len(),
            before,
            "{name}"
        );
        assert!(
            !message(&state).contains("Export "),
            "{name}: {}",
            message(&state)
        );
        assert!(message(&state).contains("not complete") || message(&state).contains("Nothing"));
    }
}

#[test]
fn a_hardlink_replaced_by_a_user_file_is_refused_and_preserved() {
    let export = exported(PublisherLinkMode::Hardlink);
    std::fs::remove_file(&export.links[0]).unwrap();
    std::fs::write(&export.links[0], b"user data").unwrap();
    let mut state = restarted(&export.journal);
    roll_back_listed(&mut state);
    assert_eq!(std::fs::read(&export.links[0]).unwrap(), b"user data");
    assert!(
        !message(&state).starts_with("Export"),
        "{}",
        message(&state)
    );
    assert_eq!(
        std::fs::read_to_string(&export.sources[0]).unwrap(),
        export.sources[0].display().to_string()
    );
}

#[test]
fn partial_undo_reverses_the_rest_and_keeps_the_refused_link_visible() {
    let export = exported(PublisherLinkMode::Symlink);
    std::fs::remove_file(&export.links[1]).unwrap();
    std::fs::write(&export.links[1], b"user data").unwrap();
    let mut state = restarted(&export.journal);
    roll_back_listed(&mut state);
    assert!(!present(&export.links[0]), "the proven link is reversed");
    assert_eq!(std::fs::read(&export.links[1]).unwrap(), b"user data");
    assert!(
        message(&state).contains("not complete"),
        "{}",
        message(&state)
    );

    // Neither this session nor a restart loses sight of it, or offers a retry.
    for mut page in [state, restarted(&export.journal)] {
        assert_eq!(page.recovery.exports.len(), 1);
        let listed = &page.recovery.exports[0];
        assert_eq!(listed.item_count, 0);
        let blocker = listed.blocker.clone().expect("explained");
        assert!(blocker.contains("left in place"), "{blocker}");
        page.recovery.selected = Some(listed.transaction.transaction_id.clone());
        page.recovery.confirmation_text = "ROLL BACK 0 ITEMS".into();
        assert!(!page.can_rollback_recovered());
        assert!(page_labels(&mut page).contains("left in place"));
    }
}

#[test]
fn old_exports_without_the_marker_are_not_listed_or_opened() {
    let mut export = exported(PublisherLinkMode::Symlink);
    let transaction = &mut export
        .state
        .execution_transaction
        .as_mut()
        .unwrap()
        .transaction;
    assert!(
        transaction
            .unknown
            .remove(PUBLISHER_EXPORT_MARKER)
            .is_some()
    );
    journal::write_journal(&export.journal, transaction).unwrap();
    assert!(!is_publisher_export(transaction));
    let state = restarted(&export.journal);
    assert!(state.recovery.exports.is_empty());
    assert!(present(&export.links[0]));
}

#[test]
fn a_changed_journal_is_refused_even_when_listed_earlier() {
    let export = exported(PublisherLinkMode::Symlink);
    let mut state = restarted(&export.journal);
    let id = state.recovery.exports[0].transaction.transaction_id.clone();
    let path = journal::journal_path(&export.journal, &id).unwrap();
    let mut edited: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edited["created_at_unix"] = serde_json::json!(1);
    std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
    roll_back_listed(&mut state);
    assert!(present(&export.links[0]));
    assert!(
        message(&state).contains("Nothing was rolled back"),
        "{}",
        message(&state)
    );
    assert!(restarted(&export.journal).recovery.exports.is_empty());
}

#[test]
fn missing_ownership_evidence_is_listed_with_a_reason_and_never_offered() {
    let mut export = exported(PublisherLinkMode::Symlink);
    let transaction = &mut export
        .state
        .execution_transaction
        .as_mut()
        .unwrap()
        .transaction;
    transaction.entries[0].state = EntryState::Applying;
    transaction.entries[0].unknown.clear();
    transaction.state = TransactionState::ApplyFailed;
    journal::write_journal(&export.journal, transaction).unwrap();
    let mut state = restarted(&export.journal);
    assert_eq!(state.recovery.exports.len(), 1);
    assert!(state.recovery.exports[0].blocker.is_some());
    roll_back_listed(&mut state);
    assert!(present(&export.links[0]));
    assert!(present(&export.links[1]));
}

#[test]
fn repeated_undo_is_refused_and_changes_nothing() {
    let export = exported(PublisherLinkMode::Symlink);
    let mut state = restarted(&export.journal);
    let stale = state.recovery.exports.clone();
    roll_back_listed(&mut state);
    assert!(
        message(&state).contains("rolled back"),
        "{}",
        message(&state)
    );
    // A page that still shows the old row, as a second window would.
    state.recovery.exports = stale;
    roll_back_listed(&mut state);
    assert!(
        message(&state).contains("Nothing was rolled back"),
        "{}",
        message(&state)
    );
    assert!(restarted(&export.journal).recovery.exports.is_empty());
}

#[test]
fn wrong_phrase_rolls_back_nothing() {
    let export = exported(PublisherLinkMode::Symlink);
    let mut state = restarted(&export.journal);
    let id = state.recovery.exports[0].transaction.transaction_id.clone();
    state.recovery.selected = Some(id);
    state.recovery.confirmation_text = "ROLL BACK 1 ITEMS".into();
    assert!(!state.can_rollback_recovered());
    state.rollback_recovered();
    assert!(present(&export.links[0]));
}
