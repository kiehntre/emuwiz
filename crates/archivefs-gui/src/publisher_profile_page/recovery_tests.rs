//! Durable Publisher rollback through real Apply, sealed journals and rollback.
//! Each test uses disposable directories; a new page state stands in for a
//! restart because the rollback handle is held only in memory.

use std::path::{Path, PathBuf};

use archivefs_core::dat::rename_apply::{EntryState, TransactionState, journal};
use archivefs_core::publisher_profile::recovery::{
    PUBLISHER_EXPORT_MARKER, is_publisher_export, open_publisher_export_for_undo,
};

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
    let destination = temp.path().join("published");
    std::fs::create_dir(&destination).unwrap();
    export_to(mode, temp, destination)
}

fn export_to(mode: PublisherLinkMode, temp: tempfile::TempDir, destination: PathBuf) -> Export {
    let sources = vec![temp.path().join("Game.rom"), temp.path().join("Other.rom")];
    let journal = temp.path().join("journal");
    for source in &sources {
        std::fs::write(source, source.display().to_string()).unwrap();
    }
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
    assert_eq!(
        state.execution_stage,
        PublisherExecutionStage::Applied,
        "{:?}",
        state.execution_error
    );
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
    type Plant = (&'static str, fn(&Path));
    let plant: [Plant; 2] = [
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
    assert!(
        open_publisher_export_for_undo(&export.journal, transaction)
            .unwrap_err()
            .contains("not a Publisher export")
    );
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
    let listed = state.recovery.exports[0].clone();
    assert!(listed.blocker.is_some());
    assert!(
        open_publisher_export_for_undo(&export.journal, &listed.transaction)
            .unwrap_err()
            .contains("no proof")
    );
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

/// Replays Apply over the same paths so a second, newer journal records them.
fn reexport(export: &mut Export) {
    for link in &export.links {
        std::fs::remove_file(link).unwrap();
    }
    export.state.result = None;
    export.state.execution_stage = PublisherExecutionStage::Preview;
    export.state.execution_transaction = None;
    export.state.preview();
    export.state.link_mode = Some(PublisherLinkMode::Hardlink);
    export.state.execution_review();
    export.state.confirmation_text = export.state.apply_confirmation_phrase().unwrap();
    export.state.apply_with_journal_dir(&export.journal);
    assert_eq!(
        export.state.execution_stage,
        PublisherExecutionStage::Applied
    );
}

#[test]
fn an_old_hardlink_export_cannot_undo_a_newer_export_of_the_same_paths() {
    let mut export = exported(PublisherLinkMode::Hardlink);
    let older = restarted(&export.journal).recovery.exports[0].clone();
    reexport(&mut export);
    let mut state = restarted(&export.journal);
    assert_eq!(state.recovery.exports.len(), 2);
    let listed = state
        .recovery
        .exports
        .iter()
        .find(|item| item.transaction.transaction_id == older.transaction.transaction_id)
        .unwrap()
        .clone();
    assert!(listed.blocker.as_deref().unwrap().contains("newer"));
    assert!(
        open_publisher_export_for_undo(&export.journal, &listed.transaction)
            .unwrap_err()
            .contains("newer")
    );
    state.recovery.selected = Some(listed.transaction.transaction_id.clone());
    state.recovery.confirmation_text = "ROLL BACK 2 ITEMS".into();
    assert!(!state.can_rollback_recovered());
    state.rollback_recovered();
    assert!(present(&export.links[0]));
    assert!(present(&export.links[1]));
}

#[test]
fn a_stale_row_is_refused_after_the_journal_legitimately_changes() {
    let export = exported(PublisherLinkMode::Symlink);
    std::fs::remove_file(&export.links[1]).unwrap();
    std::fs::write(&export.links[1], b"user data").unwrap();
    let mut stale = restarted(&export.journal);
    let mut other = restarted(&export.journal);
    roll_back_listed(&mut other);
    roll_back_listed(&mut stale);
    assert!(
        message(&stale).contains("no longer matches"),
        "{}",
        message(&stale)
    );
    assert_eq!(std::fs::read(&export.links[1]).unwrap(), b"user data");
}

#[test]
fn a_temporary_outage_is_not_blamed_on_the_links() {
    for mode in [PublisherLinkMode::Symlink, PublisherLinkMode::Hardlink] {
        let export = exported(mode);
        let moved: Vec<PathBuf> = export
            .sources
            .iter()
            .map(|source| source.with_extension("away"))
            .collect();
        for (source, away) in export.sources.iter().zip(&moved) {
            std::fs::rename(source, away).unwrap();
        }
        let destination = PathBuf::from(&export.state.destination_root);
        let hidden = destination.with_extension("offline");
        std::fs::rename(&destination, &hidden).unwrap();
        let state = restarted(&export.journal);
        let blocker = state.recovery.exports[0]
            .blocker
            .clone()
            .expect("not offered");
        assert!(blocker.contains("not available"), "{mode:?}: {blocker}");
        std::fs::rename(&hidden, &destination).unwrap();
        for (source, away) in export.sources.iter().zip(&moved) {
            std::fs::rename(away, source).unwrap();
        }
        let mut back = restarted(&export.journal);
        assert_eq!(back.recovery.exports[0].blocker, None, "{mode:?}");
        roll_back_listed(&mut back);
        assert!(!present(&export.links[0]), "{mode:?}");
    }
}

#[test]
fn a_refused_link_reports_the_recorded_reason() {
    let export = exported(PublisherLinkMode::Symlink);
    std::fs::remove_file(&export.links[1]).unwrap();
    std::fs::write(&export.links[1], b"user data").unwrap();
    let mut state = restarted(&export.journal);
    roll_back_listed(&mut state);
    let blocker = state.recovery.exports[0].blocker.clone().unwrap();
    assert!(blocker.contains("not the journal-proved link"), "{blocker}");
}

#[cfg(unix)]
#[test]
fn an_unreadable_journal_folder_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    let export = exported(PublisherLinkMode::Symlink);
    let mode = |bits| std::fs::Permissions::from_mode(bits);
    std::fs::set_permissions(&export.journal, mode(0o000)).unwrap();
    let mut state = restarted(&export.journal);
    std::fs::set_permissions(&export.journal, mode(0o700)).unwrap();
    assert!(page_labels(&mut state).contains("could not be read"));
}

#[test]
fn an_unverifiable_export_is_counted_not_silently_dropped() {
    let export = exported(PublisherLinkMode::Symlink);
    let id = export
        .state
        .execution_transaction
        .as_ref()
        .unwrap()
        .transaction
        .transaction_id
        .clone();
    let path = journal::journal_path(&export.journal, &id).unwrap();
    let mut edited: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edited["created_at_unix"] = serde_json::json!(1);
    std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
    let mut state = restarted(&export.journal);
    assert!(state.recovery.exports.is_empty());
    assert!(page_labels(&mut state).contains("1 export journal(s) could not be verified"));
}

#[cfg(unix)]
#[test]
fn an_interrupted_undo_never_claims_nothing_happened() {
    use std::os::unix::fs::PermissionsExt;
    let export = exported(PublisherLinkMode::Symlink);
    let mut state = restarted(&export.journal);
    let mode = |bits| std::fs::Permissions::from_mode(bits);
    std::fs::set_permissions(&export.journal, mode(0o500)).unwrap();
    roll_back_listed(&mut state);
    std::fs::set_permissions(&export.journal, mode(0o700)).unwrap();
    assert!(
        message(&state).contains("needs reconciliation"),
        "{}",
        message(&state)
    );
    assert!(!message(&state).contains("Nothing was rolled back"));
}

/// `temp/via/published`, where `via` is a symlink to `temp/real`.
#[cfg(unix)]
fn through_a_symlinked_parent() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("real/published")).unwrap();
    std::os::unix::fs::symlink(temp.path().join("real"), temp.path().join("via")).unwrap();
    let destination = temp.path().join("via/published");
    (temp, destination)
}

#[cfg(unix)]
#[test]
fn a_hardlink_export_through_an_unchanged_symlink_survives_preview_and_restart() {
    let (temp, destination) = through_a_symlinked_parent();
    let mut export = export_to(PublisherLinkMode::Hardlink, temp, destination);
    export.state.preview();
    assert!(export.state.execution_error.is_none());
    let mut state = restarted(&export.journal);
    assert_eq!(state.recovery.exports[0].blocker, None);
    roll_back_listed(&mut state);
    assert!(
        message(&state).contains("rolled back"),
        "{}",
        message(&state)
    );
    assert!(!present(&export.links[0]));
    assert!(export.sources[0].exists());
}

#[cfg(unix)]
#[test]
fn a_retargeted_symlink_or_replaced_folder_refuses_hardlink_undo() {
    let (temp, destination) = through_a_symlinked_parent();
    let root = temp.path().to_path_buf();
    let export = export_to(PublisherLinkMode::Hardlink, temp, destination);
    let refused = |journal: &Path| {
        let mut state = restarted(journal);
        let blocker = state.recovery.exports[0].blocker.clone().expect("refused");
        assert!(blocker.contains("no longer leads"), "{blocker}");
        roll_back_listed(&mut state);
    };

    std::fs::create_dir_all(root.join("other/published")).unwrap();
    std::fs::remove_file(root.join("via")).unwrap();
    std::os::unix::fs::symlink(root.join("other"), root.join("via")).unwrap();
    refused(&export.journal);
    std::fs::remove_file(root.join("via")).unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("via")).unwrap();

    let moved = root.join("real/moved");
    std::fs::rename(root.join("real/published"), &moved).unwrap();
    std::fs::create_dir(root.join("real/published")).unwrap();
    refused(&export.journal);
    assert!(moved.join("roms/amiga/Game.rom").exists());
}

#[cfg(unix)]
#[test]
fn a_symlink_export_through_a_symlinked_parent_stops_before_apply() {
    let (temp, destination) = through_a_symlinked_parent();
    let source = temp.path().join("Game.rom");
    std::fs::write(&source, b"rom").unwrap();
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: destination.display().to_string(),
        source_plan: Some(plan_with(vec![elected(
            "Game",
            &source.display().to_string(),
        )])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    state.link_mode = Some(PublisherLinkMode::Symlink);
    state.execution_review();
    assert_ne!(state.execution_stage, PublisherExecutionStage::Confirming);
    let error = state.execution_error.clone().unwrap_or_default();
    assert!(error.contains("goes through a symlink"), "{error}");
    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
}
