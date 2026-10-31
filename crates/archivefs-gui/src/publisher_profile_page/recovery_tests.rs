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
    export_from(mode, temp, destination, sources)
}

/// Each source lives in its own folder, so one folder can go missing alone.
fn exported_split(mode: PublisherLinkMode) -> Export {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("published");
    std::fs::create_dir(&destination).unwrap();
    let sources = vec![
        temp.path().join("disk_a/Game.rom"),
        temp.path().join("disk_b/Other.rom"),
    ];
    for source in &sources {
        std::fs::create_dir(source.parent().unwrap()).unwrap();
    }
    export_from(mode, temp, destination, sources)
}

fn export_from(
    mode: PublisherLinkMode,
    temp: tempfile::TempDir,
    destination: PathBuf,
    sources: Vec<PathBuf>,
) -> Export {
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
    let labels = page_labels(&mut state);
    assert!(labels.contains("could not be read"), "{labels}");
    assert!(labels.contains("Check again"), "{labels}");
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

#[test]
fn a_symlink_export_keeps_undo_while_its_destination_is_unmounted() {
    let export = exported(PublisherLinkMode::Symlink);
    let root = PathBuf::from(&export.state.destination_root);
    let hidden = root.with_extension("offline");
    std::fs::rename(&root, &hidden).unwrap();
    std::fs::create_dir(&root).unwrap();
    let blocker = restarted(&export.journal).recovery.exports[0]
        .blocker
        .clone()
        .expect("not offered");
    assert!(blocker.contains("no longer leads"), "{blocker}");
    std::fs::remove_dir(&root).unwrap();
    std::fs::rename(&hidden, &root).unwrap();
    let mut state = restarted(&export.journal);
    assert_eq!(state.recovery.exports[0].blocker, None);
    roll_back_listed(&mut state);
    assert!(!present(&export.links[0]));
}

#[test]
fn an_unreadable_journal_blocks_hardlink_undo_but_not_symlink_undo() {
    for (mode, blocked) in [
        (PublisherLinkMode::Hardlink, true),
        (PublisherLinkMode::Symlink, false),
    ] {
        let export = exported(mode);
        std::fs::write(export.journal.join("9999999999-0.json"), b"not json").unwrap();
        let mut state = restarted(&export.journal);
        assert_eq!(
            state.recovery.exports[0].blocker.is_some(),
            blocked,
            "{mode:?}"
        );
        let labels = page_labels(&mut state);
        assert!(
            labels.contains("1 journal(s) could not be read"),
            "{labels}"
        );
    }
}

#[test]
fn preview_during_an_outage_does_not_advise_a_rollback_that_would_lose_undo() {
    let mut export = exported(PublisherLinkMode::Symlink);
    let root = PathBuf::from(&export.state.destination_root);
    let hidden = root.with_extension("offline");
    std::fs::rename(&root, &hidden).unwrap();
    std::fs::create_dir(&root).unwrap();
    assert!(!export.state.keep_rollback_durable("Preview was not run"));
    let error = export.state.execution_error.clone().unwrap();
    assert!(!error.contains("Roll it back here first"), "{error}");
    assert!(error.contains("Reconnect them and try again"), "{error}");
    assert!(!error.contains("would not be offered again"), "{error}");
    std::fs::remove_dir(&root).unwrap();
    std::fs::rename(&hidden, &root).unwrap();
    assert!(export.state.keep_rollback_durable("Preview was not run"));
}

const BOTH_MODES: [PublisherLinkMode; 2] =
    [PublisherLinkMode::Symlink, PublisherLinkMode::Hardlink];

fn take_away(folder: &Path) -> PathBuf {
    let away = folder.with_extension("away");
    std::fs::rename(folder, &away).unwrap();
    away
}

fn journal_record(export: &Export) -> archivefs_core::dat::rename_apply::RenameTransaction {
    let id = export.state.recovery.exports.first().map_or_else(
        || {
            export
                .state
                .execution_transaction
                .as_ref()
                .unwrap()
                .transaction
                .transaction_id
                .clone()
        },
        |listed| listed.transaction.transaction_id.clone(),
    );
    journal::read_journal(&journal::journal_path(&export.journal, &id).unwrap()).unwrap()
}

#[test]
fn undo_survives_losing_one_source_folder_and_finishes_after_it_returns() {
    for mode in BOTH_MODES {
        let export = exported_split(mode);
        let folder = export.sources[0].parent().unwrap().to_path_buf();
        let away = take_away(&folder);
        let mut state = restarted(&export.journal);
        assert_eq!(state.recovery.exports[0].blocker, None, "{mode:?}");
        roll_back_listed(&mut state);

        // The reachable link was undone; the other was not touched or refused.
        assert!(!present(&export.links[1]), "{mode:?}");
        assert!(present(&export.links[0]), "{mode:?}");
        let said = message(&state);
        assert!(said.contains("not complete"), "{mode:?}: {said}");
        assert!(said.contains("not available"), "{mode:?}: {said}");
        assert!(said.contains("roll back again"), "{mode:?}: {said}");
        assert!(!said.contains("check them by hand"), "{mode:?}: {said}");

        // Still listed, the unreachable link counted, and Undo held back with the
        // reason until its folder returns - now and after a restart.
        for page in [&state, &restarted(&export.journal)] {
            assert_eq!(page.recovery.exports.len(), 1, "{mode:?}");
            assert_eq!(page.recovery.exports[0].item_count, 1, "{mode:?}");
            let blocker = page.recovery.exports[0].blocker.clone().expect("waits");
            assert!(blocker.contains("not available"), "{mode:?}: {blocker}");
            assert!(!blocker.contains("left in place"), "{mode:?}: {blocker}");
        }
        let record = journal_record(&export);
        assert_ne!(record.state, TransactionState::RolledBack);
        assert_eq!(record.entries[0].state, EntryState::Applied, "{mode:?}");
        assert_eq!(record.entries[1].state, EntryState::RolledBack, "{mode:?}");
        assert!(present(&export.links[0]), "{mode:?}");

        std::fs::rename(&away, &folder).unwrap();
        let mut back = restarted(&export.journal);
        roll_back_listed(&mut back);
        assert!(
            message(&back).contains("rolled back"),
            "{mode:?}: {}",
            message(&back)
        );
        assert!(!present(&export.links[0]), "{mode:?}");
        assert!(back.recovery.exports.is_empty(), "{mode:?}");
        assert_eq!(
            journal_record(&export).state,
            TransactionState::RolledBack,
            "{mode:?}"
        );
        for source in &export.sources {
            assert!(present(source), "{mode:?}");
        }
    }
}

#[test]
fn undo_survives_a_missing_folder_inside_the_destination() {
    for mode in BOTH_MODES {
        let export = exported(mode);
        let folder = export.links[0].parent().unwrap().to_path_buf();
        assert_ne!(folder, PathBuf::from(&export.state.destination_root));
        let away = take_away(&folder);
        let mut state = restarted(&export.journal);
        assert_eq!(state.recovery.exports[0].blocker, None, "{mode:?}");
        roll_back_listed(&mut state);
        assert!(message(&state).contains("not complete"), "{mode:?}");
        assert_eq!(state.recovery.exports[0].item_count, 2, "{mode:?}");

        std::fs::rename(&away, &folder).unwrap();
        for link in &export.links {
            assert!(present(link), "{mode:?}: nothing was removed");
        }
        let mut back = restarted(&export.journal);
        roll_back_listed(&mut back);
        assert!(
            message(&back).contains("rolled back"),
            "{mode:?}: {}",
            message(&back)
        );
        for link in &export.links {
            assert!(!present(link), "{mode:?}");
        }
    }
}

#[test]
fn in_session_undo_during_an_outage_can_be_retried() {
    for mode in BOTH_MODES {
        let mut export = exported_split(mode);
        let folder = export.sources[0].parent().unwrap().to_path_buf();
        let away = take_away(&folder);
        export.state.rollback_confirmation_text =
            export.state.rollback_confirmation_phrase().unwrap();
        export.state.rollback_with_journal_dir(&export.journal);
        let error = export.state.execution_error.clone().unwrap();
        assert!(error.contains("not complete"), "{mode:?}: {error}");
        assert!(error.contains("Reconnect"), "{mode:?}: {error}");
        assert_eq!(
            export.state.execution_stage,
            PublisherExecutionStage::NeedsReconciliation
        );
        // The page still offers Undo for exactly the link that was held back.
        assert_eq!(
            export.state.rollback_confirmation_phrase().as_deref(),
            Some("ROLL BACK 1 ITEMS"),
            "{mode:?}"
        );

        std::fs::rename(&away, &folder).unwrap();
        export.state.rollback_confirmation_text =
            export.state.rollback_confirmation_phrase().unwrap();
        export.state.rollback_with_journal_dir(&export.journal);
        assert_eq!(
            export.state.execution_stage,
            PublisherExecutionStage::RolledBack,
            "{mode:?}: {:?}",
            export.state.execution_error
        );
        for link in &export.links {
            assert!(!present(link), "{mode:?}");
        }
    }
}

#[test]
fn a_link_changed_during_an_outage_is_still_refused_and_never_deleted() {
    for mode in BOTH_MODES {
        let export = exported_split(mode);
        let folder = export.sources[0].parent().unwrap().to_path_buf();
        let away = take_away(&folder);
        let mut state = restarted(&export.journal);
        roll_back_listed(&mut state);
        assert!(present(&export.links[0]), "{mode:?}");

        // While it is away, the user replaces the still-recorded link.
        std::fs::remove_file(&export.links[0]).unwrap();
        std::fs::write(&export.links[0], b"user data").unwrap();
        std::fs::rename(&away, &folder).unwrap();
        let mut back = restarted(&export.journal);
        roll_back_listed(&mut back);

        assert_eq!(
            std::fs::read(&export.links[0]).unwrap(),
            b"user data",
            "{mode:?}"
        );
        assert!(message(&back).contains("not complete"), "{mode:?}");
        assert!(
            message(&back).contains("check them by hand"),
            "{mode:?}: {}",
            message(&back)
        );
        // Now it needs the user: no further Undo is offered for it.
        let blocker = back.recovery.exports[0].blocker.clone().expect("refused");
        assert!(blocker.contains("left in place"), "{mode:?}: {blocker}");
        let mut third = restarted(&export.journal);
        roll_back_listed(&mut third);
        assert_eq!(
            std::fs::read(&export.links[0]).unwrap(),
            b"user data",
            "{mode:?}"
        );
        assert_eq!(
            journal_record(&export).entries[0].state,
            EntryState::RollbackFailed,
            "{mode:?}"
        );
    }
}

#[test]
fn a_deleted_link_in_a_present_folder_is_a_change_not_an_outage() {
    for mode in BOTH_MODES {
        let export = exported_split(mode);
        std::fs::remove_file(&export.links[0]).unwrap();
        let mut state = restarted(&export.journal);
        roll_back_listed(&mut state);
        let record = journal_record(&export);
        assert_eq!(
            record.entries[0].state,
            EntryState::RollbackFailed,
            "{mode:?}"
        );
        assert!(!present(&export.links[1]), "{mode:?}");
        for source in &export.sources {
            assert!(present(source), "{mode:?}");
        }
    }
}

#[test]
fn a_retried_undo_removes_no_independent_file() {
    for mode in BOTH_MODES {
        let export = exported_split(mode);
        let independent = PathBuf::from(&export.state.destination_root).join("mine.txt");
        std::fs::write(&independent, b"mine").unwrap();
        let folder = export.sources[0].parent().unwrap().to_path_buf();
        let away = take_away(&folder);
        let mut state = restarted(&export.journal);
        roll_back_listed(&mut state);
        std::fs::rename(&away, &folder).unwrap();
        let mut back = restarted(&export.journal);
        roll_back_listed(&mut back);
        assert_eq!(std::fs::read(&independent).unwrap(), b"mine", "{mode:?}");
        for link in &export.links {
            assert!(!present(link), "{mode:?}");
        }
    }
}

#[test]
fn a_crash_mid_undo_during_an_outage_recovers_after_restart() {
    for mode in BOTH_MODES {
        let export = exported_split(mode);
        let mut state = restarted(&export.journal);
        let mut record = state.recovery.exports[0].transaction.clone();
        // The process died after checkpointing the reverse step of one link.
        record.entries[0].state = EntryState::RollingBack;
        record.state = TransactionState::RollingBack;
        journal::write_journal(&export.journal, &record).unwrap();
        let folder = export.sources[0].parent().unwrap().to_path_buf();
        let away = take_away(&folder);
        state.load_recoverable_exports();
        roll_back_listed(&mut state);
        assert!(present(&export.links[0]), "{mode:?}: nothing guessed");
        assert!(
            message(&state).contains("not complete")
                || message(&state).contains("reconcil")
                || message(&state).contains("not complete"),
            "{mode:?}: {}",
            message(&state)
        );

        std::fs::rename(&away, &folder).unwrap();
        let mut back = restarted(&export.journal);
        assert_eq!(back.recovery.exports.len(), 1, "{mode:?}: still listed");
        roll_back_listed(&mut back);
        for link in &export.links {
            assert!(!present(link), "{mode:?}: {}", message(&back));
        }
        assert_eq!(
            journal_record(&export).state,
            TransactionState::RolledBack,
            "{mode:?}"
        );
    }
}

#[test]
fn a_changed_journal_still_blocks_a_retry_after_an_outage() {
    let export = exported_split(PublisherLinkMode::Symlink);
    let folder = export.sources[0].parent().unwrap().to_path_buf();
    let away = take_away(&folder);
    let mut state = restarted(&export.journal);
    roll_back_listed(&mut state);
    std::fs::rename(&away, &folder).unwrap();
    let id = journal_record(&export).transaction_id;
    let path = journal::journal_path(&export.journal, &id).unwrap();
    let mut edited: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edited["created_at_unix"] = serde_json::json!(1);
    std::fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
    assert!(restarted(&export.journal).recovery.exports.is_empty());
    assert!(present(&export.links[0]));
}
