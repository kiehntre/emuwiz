//! Focused tests for the Publisher / Frontend Library planning view's
//! state logic - task section 27's "GUI tests". These exercise the same
//! `preview()`/filter logic the page itself calls, not simulated egui
//! clicks (this codebase's other GUI pages test the same way - see
//! `playing_library_page/tests.rs`).

use std::path::PathBuf;

use archivefs_core::playing_library::{
    CandidateEvidenceSummary, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
    PlayingLibraryPlan, PlayingLibraryPolicy,
};
use archivefs_core::publisher_profile::PublisherActionSafety;

use super::*;

fn elected(name: &str, file: &str) -> ElectedGame {
    ElectedGame {
        dat_entry_name: name.to_string(),
        family_root_name: name.to_string(),
        explanation: ElectionExplanation {
            steps: vec!["the only election-eligible release in its family".to_string()],
            rejected: Vec::new(),
            winner_evidence: CandidateEvidenceSummary::unknown(),
        },
        launcher_operation: LinkedLibraryOperation {
            source_path: PathBuf::from(file),
            destination_path: PathBuf::from("/source-library").join(file),
        },
        companion_operations: Vec::new(),
    }
}

fn plan_with(games: Vec<ElectedGame>) -> PlayingLibraryPlan {
    PlayingLibraryPlan {
        destination_root: PathBuf::from("/source-library"),
        policy: PlayingLibraryPolicy::default(),
        archives_examined: games.len(),
        families_examined: games.len(),
        elected_games: games,
        unresolved_groups: Vec::new(),
        exclusions: Vec::new(),
        singleton_families: 0,
        conflicts: Vec::new(),
        operations: Vec::new(),
        rejected_launchers: Vec::new(),
    }
}

#[test]
fn preview_without_a_target_reports_a_plain_language_error() {
    let mut state = PublisherProfilePageState {
        destination_root: "/library".to_string(),
        source_plan: Some(plan_with(vec![elected("Game", "game.rom")])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    assert!(state.result.is_none());
    assert_eq!(
        state.error.as_deref(),
        Some("Choose a target profile first.")
    );
}

#[test]
fn preview_without_a_source_plan_reports_a_plain_language_error() {
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: "/library".to_string(),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    assert!(state.result.is_none());
    assert!(state.error.is_some());
}

#[test]
fn preview_with_a_relative_destination_is_rejected() {
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: "relative/path".to_string(),
        source_plan: Some(plan_with(vec![elected("Game", "game.rom")])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    assert!(state.result.is_none());
    assert!(state.error.as_deref().unwrap().contains("absolute"));
}

#[test]
fn preview_with_valid_input_produces_a_result_with_no_apply_path() {
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: "/library".to_string(),
        source_plan: Some(plan_with(vec![elected("Sensible Soccer", "soccer.adf")])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    assert!(state.error.is_none());
    let result = state.result.unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].safety, PublisherActionSafety::SafeToAct);
}

#[test]
fn es_de_target_resolves_via_its_own_reviewed_table() {
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::EsDe),
        destination_root: "/rompath".to_string(),
        source_plan: Some(plan_with(vec![elected("Bonk's Adventure", "bonk.pce")])),
        canonical_platform_id: "TurboGrafx-16".to_string(),
        ..Default::default()
    };
    state.preview();
    let result = state.result.unwrap();
    assert_eq!(
        result.items[0].planned_destination,
        Some(PathBuf::from("/rompath/tg16/bonk.pce"))
    );
}

#[test]
fn filters_partition_items_by_safety_and_destination_state() {
    let ready = elected("Ready Game", "ready.rom");
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: "/library".to_string(),
        source_plan: Some(plan_with(vec![ready])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    let result = state.result.as_ref().unwrap();
    let ready_count = result
        .items
        .iter()
        .filter(|item| PublisherResultFilter::Ready.matches(item))
        .count();
    assert_eq!(ready_count, 1);
    let blocked_count = result
        .items
        .iter()
        .filter(|item| PublisherResultFilter::Blocked.matches(item))
        .count();
    assert_eq!(blocked_count, 0);
}

fn executable_state(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> PublisherProfilePageState {
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
    state
}

#[test]
fn execution_review_requires_explicit_mode_and_exact_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    std::fs::write(&source, b"gui source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);

    state.execution_review();
    assert_eq!(state.execution_stage, PublisherExecutionStage::Review);
    assert!(state.execution_error.is_some());

    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Hardlink);
    state.execution_review();
    assert_eq!(state.execution_stage, PublisherExecutionStage::Confirming);
    assert!(!state.can_apply());
    state.confirmation_text = "PUBLISH 0 ITEMS".to_string();
    assert!(!state.can_apply());
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    assert!(state.can_apply());
}

#[test]
fn gui_hardlink_path_applies_and_rolls_back_without_touching_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"hardlink gui source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Hardlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.apply_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::Applied);
    let transaction = state.execution_transaction.as_ref().unwrap();
    let published = transaction.transaction.entries[0].destination_path.clone();
    assert!(published.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"hardlink gui source");
    assert_eq!(
        state.execution_result.as_ref().unwrap().mode,
        archivefs_core::publisher_profile::PublisherLinkMode::Hardlink
    );

    state.rollback_confirmation_text = state.rollback_confirmation_phrase().unwrap();
    assert!(state.can_rollback());
    state.rollback_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::RolledBack);
    assert!(!published.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"hardlink gui source");
}

#[test]
fn gui_apply_refuses_a_replaced_destination_root_before_any_write() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"root replacement source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();

    std::fs::remove_dir(&destination).unwrap();
    std::fs::create_dir(&destination).unwrap();
    state.apply_with_journal_dir(&journal);

    assert_eq!(state.execution_stage, PublisherExecutionStage::Failed);
    assert!(
        state
            .execution_error
            .as_deref()
            .is_some_and(|error| error.contains("destination root changed"))
    );
    assert!(!destination.join("roms").exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"root replacement source");
}

#[test]
fn gui_symlink_path_applies_and_rolls_back_without_touching_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"symlink gui source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.apply_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::Applied);
    let published = state
        .execution_transaction
        .as_ref()
        .unwrap()
        .transaction
        .entries[0]
        .destination_path
        .clone();
    assert!(published.is_symlink());
    assert_eq!(std::fs::read_link(&published).unwrap(), source);
    assert_eq!(
        state.execution_result.as_ref().unwrap().mode,
        archivefs_core::publisher_profile::PublisherLinkMode::Symlink
    );

    state.rollback_confirmation_text = state.rollback_confirmation_phrase().unwrap();
    state.rollback_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::RolledBack);
    assert!(!published.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"symlink gui source");
}

#[test]
fn changed_destination_blocks_apply_and_requires_a_new_preview() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"stale gui source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.destination_root = temp.path().join("changed").display().to_string();
    state.apply_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::Failed);
    assert!(
        state
            .execution_error
            .as_deref()
            .unwrap()
            .contains("changed since preview")
    );
    assert!(!destination.join("roms").exists());
}

// ---- R3: `Ok(outcome)` from the executor is not "published" unless fully resolved ----

fn publish_summary(
    applied: usize,
    failed: usize,
    unresolved: usize,
    pending: usize,
) -> archivefs_core::dat::rename_apply::TransactionSummary {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionSummary, UnresolvedEntry};
    TransactionSummary {
        requested: applied + failed + unresolved + pending,
        applied,
        failed,
        unresolved,
        pending,
        unresolved_entries: (0..unresolved)
            .map(|i| UnresolvedEntry {
                source_path: std::path::PathBuf::from(format!("/src/a{i}.bin")),
                destination_path: std::path::PathBuf::from(format!("/out/A{i}.bin")),
                state: EntryState::Applying,
                stage: "rename in flight (completion not recorded)".into(),
                detail: None,
                required_action: "Run recovery reconciliation; do not replay.".into(),
            })
            .collect(),
        ..TransactionSummary::default()
    }
}

/// A journal recording no entry and no created directory.
fn no_change_record() -> archivefs_core::dat::rename_apply::RenameTransaction {
    use archivefs_core::dat::rename_apply::{RenameTransaction, TransactionState};
    RenameTransaction {
        transaction_id: "t".into(),
        plan_generation: 1,
        classifier_version: None,
        created_at_unix: 0,
        source_scan_root: String::new(),
        state: TransactionState::ApplyFailed,
        entries: Vec::new(),
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    }
}

#[test]
fn publisher_reports_applied_only_for_a_fully_resolved_batch() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let (state, _temp) = recorded_outcome(
        &[EntryState::Applied, EntryState::Applied],
        TransactionState::Applied,
        Vec::new(),
    );
    assert_eq!(state.execution_stage, PublisherExecutionStage::Applied);
    assert!(state.execution_error.is_none());

    let transaction = no_change_record();

    for incomplete in [
        publish_summary(0, 0, 1, 2),
        publish_summary(2, 1, 0, 0),
        publish_summary(1, 0, 0, 3),
    ] {
        assert_eq!(
            execution_stage_for(&incomplete, &transaction),
            PublisherExecutionStage::NeedsReconciliation
        );
        let message = incomplete_publish_message(&incomplete, &transaction).expect("explained");
        assert!(message.contains("NOT complete"), "{message}");
    }
    let message = incomplete_publish_message(&publish_summary(0, 0, 1, 0), &transaction).unwrap();
    assert!(message.contains("/out/A0.bin"), "{message}");
    assert!(message.contains("do not replay"), "{message}");
}

/// A real reviewed transaction, reshaped into what the executor recorded,
/// with its summary built from that record by the core.
fn recorded_outcome(
    entry_states: &[archivefs_core::dat::rename_apply::EntryState],
    transaction_state: archivefs_core::dat::rename_apply::TransactionState,
    created_directories: Vec<PathBuf>,
) -> (PublisherProfilePageState, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    std::fs::write(&source, b"outcome source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    let mut transaction = state.execution_transaction.take().expect("reviewed");
    let entry = transaction.transaction.entries[0].clone();
    transaction.transaction.entries = entry_states
        .iter()
        .map(|entry_state| {
            let mut entry = entry.clone();
            entry.state = *entry_state;
            entry
        })
        .collect();
    transaction.transaction.state = transaction_state;
    transaction.transaction.created_directories = created_directories;
    let outcome = archivefs_core::dat::rename_apply::ApplyOutcome {
        transaction: transaction.transaction.clone(),
        summary: archivefs_core::dat::rename_apply::TransactionSummary::from_transaction(
            &transaction.transaction,
        ),
    };
    state.record_success(transaction, outcome);
    (state, temp)
}

#[test]
fn all_skipped_run_is_never_published_or_offered_rollback() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let (mut state, _temp) = recorded_outcome(
        &[EntryState::Skipped, EntryState::Skipped],
        TransactionState::ApplyFailed,
        Vec::new(),
    );
    assert_eq!(
        state.execution_stage,
        PublisherExecutionStage::NothingPublished
    );
    assert!(state.execution_error.is_none());
    state.rollback_confirmation_text = "ROLL BACK 0 ITEMS".into();
    assert!(!state.can_rollback(), "nothing was recorded to reverse");
    let labels = page_labels(&mut state);
    assert!(labels.contains("Nothing published"), "{labels}");
    assert!(labels.contains("2 item(s) were skipped"), "{labels}");
    for false_claim in ["Published 0", "Ready to publish", "Roll back publisher"] {
        assert!(!labels.contains(false_claim), "{false_claim}: {labels}");
    }
}

#[test]
fn run_recorded_as_rolled_back_reads_undone_not_unchanged() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let (mut state, _temp) = recorded_outcome(
        &[EntryState::RolledBack, EntryState::RolledBack],
        TransactionState::RolledBack,
        Vec::new(),
    );
    assert_eq!(state.execution_stage, PublisherExecutionStage::RolledBack);
    assert!(state.execution_error.is_none());
    let labels = page_labels(&mut state);
    assert!(labels.contains("Export rolled back"), "{labels}");
    assert!(!labels.contains("Nothing was published"), "{labels}");

    // The journal must prove it: a contradicting record is never called undone.
    let (state, _temp) = recorded_outcome(
        &[EntryState::RolledBack, EntryState::RollbackFailed],
        TransactionState::RolledBack,
        Vec::new(),
    );
    assert_eq!(
        state.execution_stage,
        PublisherExecutionStage::NeedsReconciliation
    );
    assert!(state.execution_error.is_some());
}

#[test]
fn failed_unresolved_or_pending_only_batches_need_reconciliation() {
    let transaction = no_change_record();
    for summary in [
        publish_summary(0, 1, 0, 0),
        publish_summary(0, 0, 1, 0),
        publish_summary(0, 0, 0, 1),
    ] {
        assert_eq!(
            execution_stage_for(&summary, &transaction),
            PublisherExecutionStage::NeedsReconciliation
        );
    }
}

#[test]
fn every_journal_state_maps_to_an_outcome_the_record_supports() {
    use archivefs_core::dat::rename_apply::{
        EntryState as E, TransactionState as T, TransactionSummary,
    };
    let entry_states = [
        E::Planned,
        E::PreflightPassed,
        E::Applying,
        E::Applied,
        E::ApplyFailed,
        E::RollingBack,
        E::RolledBack,
        E::RollbackFailed,
        E::Skipped,
    ];
    let transaction_states = [
        T::Planned,
        T::Applying,
        T::Applied,
        T::ApplyFailed,
        T::RollingBack,
        T::RolledBack,
        T::RollbackFailed,
        T::Finalising,
        T::Finalised,
    ];
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    std::fs::write(&source, b"outcome source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut reviewed = executable_state(&source, &destination);
    reviewed.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    reviewed.execution_review();
    let mut base = reviewed.execution_transaction.take().expect("reviewed");
    let extra = base.transaction.entries[0].clone();
    base.transaction.entries.push(extra);

    for first in entry_states {
        for second in entry_states {
            for transaction_state in transaction_states {
                for created_flag in [false, true] {
                    let created = if created_flag {
                        vec![PathBuf::from("/out/created")]
                    } else {
                        Vec::new()
                    };
                    let mut transaction = base.clone();
                    transaction.transaction.entries[0].state = first;
                    transaction.transaction.entries[1].state = second;
                    transaction.transaction.state = transaction_state;
                    transaction.transaction.created_directories = created;
                    let record = transaction.transaction.clone();
                    let summary = TransactionSummary::from_transaction(&record);
                    let outcome = archivefs_core::dat::rename_apply::ApplyOutcome {
                        transaction: record.clone(),
                        summary: summary.clone(),
                    };
                    let mut state = PublisherProfilePageState {
                        link_mode: reviewed.link_mode,
                        ..Default::default()
                    };
                    state.record_success(transaction, outcome);
                    let case = format!("{first:?}/{second:?}/{transaction_state:?}");
                    // Expected values come from the raw states, not the
                    // production predicates under test.
                    let entries = [first, second];
                    let count = |wanted: E| entries.iter().filter(|e| **e == wanted).count();
                    let applied = count(E::Applied);
                    let surrendered = matches!(transaction_state, T::Finalising | T::Finalised);
                    let reversible = !surrendered
                        && match transaction_state {
                            T::Applied => applied > 0,
                            T::RolledBack => false,
                            _ => true,
                        };
                    let changed = created_flag
                        || entries.iter().any(|e| {
                            matches!(
                                e,
                                E::Applying | E::Applied | E::RollingBack | E::RollbackFailed
                            )
                        });
                    let unsettled = entries
                        .iter()
                        .any(|e| !matches!(e, E::Applied | E::Skipped | E::RolledBack));
                    let rolled_back = count(E::RolledBack);
                    let expected_stage = if unsettled {
                        PublisherExecutionStage::NeedsReconciliation
                    } else if applied > 0 {
                        if reversible {
                            PublisherExecutionStage::Applied
                        } else {
                            PublisherExecutionStage::NeedsReconciliation
                        }
                    } else if rolled_back > 0 {
                        if transaction_state == T::RolledBack {
                            PublisherExecutionStage::RolledBack
                        } else {
                            PublisherExecutionStage::NeedsReconciliation
                        }
                    } else if changed {
                        PublisherExecutionStage::NeedsReconciliation
                    } else {
                        PublisherExecutionStage::NothingPublished
                    };
                    assert_eq!(state.execution_stage, expected_stage, "{case}");
                    if expected_stage == PublisherExecutionStage::NeedsReconciliation {
                        assert!(state.execution_error.is_some(), "{case}");
                    }
                    let in_flight = count(E::Applying) + count(E::RollingBack);
                    let reversible_items = applied + in_flight;
                    let expected_phrase = (reversible_items > 0
                        && reversible
                        && expected_stage != PublisherExecutionStage::RolledBack)
                        .then(|| format!("ROLL BACK {reversible_items} ITEMS"));
                    assert_eq!(
                        state.rollback_confirmation_phrase(),
                        expected_phrase,
                        "{case}"
                    );
                    state.rollback_confirmation_text = expected_phrase.clone().unwrap_or_default();
                    assert_eq!(state.can_rollback(), expected_phrase.is_some(), "{case}");
                }
            }
        }
    }
}

/// Two really published items whose journal is then rewritten to `entries`,
/// as a run that stopped part-way would have left it.
struct ReshapedRun {
    state: PublisherProfilePageState,
    journal: PathBuf,
    published: Vec<PathBuf>,
    temp: tempfile::TempDir,
}

fn reshaped_run(
    entries: &[archivefs_core::dat::rename_apply::EntryState],
    transaction_state: archivefs_core::dat::rename_apply::TransactionState,
) -> ReshapedRun {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("Game.rom");
    let second = temp.path().join("Other.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&first, b"first source").unwrap();
    std::fs::write(&second, b"second source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = PublisherProfilePageState {
        target: Some(PublisherFrontend::RomM),
        destination_root: destination.display().to_string(),
        source_plan: Some(plan_with(vec![
            elected("Game", &first.display().to_string()),
            elected("Other", &second.display().to_string()),
        ])),
        canonical_platform_id: "Amiga".to_string(),
        ..Default::default()
    };
    state.preview();
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.apply_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::Applied);
    let transaction = &mut state.execution_transaction.as_mut().unwrap().transaction;
    for (entry, entry_state) in transaction.entries.iter_mut().zip(entries) {
        entry.state = *entry_state;
    }
    transaction.state = transaction_state;
    archivefs_core::dat::rename_apply::journal::write_journal(&journal, transaction).unwrap();
    let published = transaction
        .entries
        .iter()
        .map(|entry| entry.destination_path.clone())
        .collect();
    state.execution_stage = PublisherExecutionStage::NeedsReconciliation;
    ReshapedRun {
        state,
        journal,
        published,
        temp,
    }
}

#[test]
fn rollback_is_not_called_complete_while_a_failed_entrys_link_remains() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let mut run = reshaped_run(
        &[EntryState::Applied, EntryState::ApplyFailed],
        TransactionState::ApplyFailed,
    );
    assert_eq!(
        run.state.rollback_confirmation_phrase().as_deref(),
        Some("ROLL BACK 1 ITEMS")
    );
    run.state.rollback_confirmation_text = "ROLL BACK 1 ITEMS".into();
    run.state.rollback_with_journal_dir(&run.journal);
    assert!(std::fs::symlink_metadata(&run.published[1]).is_ok());
    assert_eq!(
        run.state.execution_stage,
        PublisherExecutionStage::NeedsReconciliation
    );
    let message = run.state.execution_error.clone().expect("explained");
    assert!(
        message.contains(&run.published[1].display().to_string()),
        "{message}"
    );
    let labels = page_labels(&mut run.state);
    assert!(!labels.contains("rolled back."), "{labels}");
    assert!(!labels.contains("Export rolled back"), "{labels}");
}

#[test]
fn rollback_reports_anything_in_a_created_directory_it_did_not_record() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    type Plant = fn(&std::path::Path);
    let shapes: [(&str, Plant); 6] = [
        ("empty folder", |d| {
            std::fs::create_dir(d.join("saves")).unwrap()
        }),
        ("nested tombstone name", |d| {
            std::fs::create_dir(d.join("saves")).unwrap();
            std::fs::write(d.join("saves/.emuwiz-undone-x"), b"user").unwrap();
        }),
        ("file with tombstone prefix", |d| {
            std::fs::write(d.join(".emuwiz-undone-user-save"), b"user").unwrap()
        }),
        ("symlink with tombstone prefix", |d| {
            std::os::unix::fs::symlink("/nowhere", d.join(".emuwiz-undone-link")).unwrap()
        }),
        ("folder with tombstone prefix", |d| {
            std::fs::create_dir(d.join(".emuwiz-undone-d")).unwrap()
        }),
        ("plain symlink", |d| {
            std::os::unix::fs::symlink("/nowhere", d.join("plain")).unwrap()
        }),
    ];
    for (name, plant) in shapes {
        let mut run = reshaped_run(
            &[EntryState::Applied, EntryState::Applied],
            TransactionState::Applied,
        );
        plant(run.published[0].parent().unwrap());
        run.state.rollback_confirmation_text = "ROLL BACK 2 ITEMS".into();
        run.state.rollback_with_journal_dir(&run.journal);
        assert_eq!(
            run.state.execution_stage,
            PublisherExecutionStage::NeedsReconciliation,
            "{name}"
        );
        assert!(run.state.execution_error.is_some(), "{name}");
    }
}

#[test]
fn rollback_refuses_an_in_flight_entry_with_no_creation_proof() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let mut run = reshaped_run(
        &[EntryState::Applied, EntryState::Applied],
        TransactionState::Applied,
    );
    let journal = run.journal.clone();
    let transaction = &mut run
        .state
        .execution_transaction
        .as_mut()
        .unwrap()
        .transaction;
    transaction.entries[1].state = EntryState::Applying;
    transaction.entries[1].unknown.clear();
    transaction.state = TransactionState::ApplyFailed;
    archivefs_core::dat::rename_apply::journal::write_journal(&journal, transaction).unwrap();
    run.state.rollback_confirmation_text = "ROLL BACK 2 ITEMS".into();
    run.state.rollback_with_journal_dir(&journal);
    assert_eq!(
        run.state.execution_stage,
        PublisherExecutionStage::NeedsReconciliation
    );
    assert!(run.state.execution_error.is_some());
    for link in &run.published {
        assert!(std::fs::symlink_metadata(link).is_ok(), "nothing reversed");
    }
}

#[test]
fn clean_rollback_of_real_links_still_reads_rolled_back() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let mut run = reshaped_run(
        &[EntryState::Applied, EntryState::Applied],
        TransactionState::Applied,
    );
    run.state.rollback_confirmation_text = "ROLL BACK 2 ITEMS".into();
    run.state.rollback_with_journal_dir(&run.journal);
    assert_eq!(
        run.state.execution_stage,
        PublisherExecutionStage::RolledBack
    );
    assert!(run.state.execution_error.is_none());
}

#[test]
fn rollback_control_does_not_promise_it_outlasts_this_session() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let (mut state, _temp) = recorded_outcome(
        &[EntryState::Applied],
        TransactionState::Applied,
        Vec::new(),
    );
    let labels = page_labels(&mut state);
    assert!(
        labels.contains("available only until you preview again or close EmuWiz"),
        "{labels}"
    );
}

#[test]
fn rollback_counts_and_reverses_an_in_flight_entry() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let mut run = reshaped_run(
        &[EntryState::Applied, EntryState::Applying],
        TransactionState::ApplyFailed,
    );
    assert_eq!(
        run.state.rollback_confirmation_phrase().as_deref(),
        Some("ROLL BACK 2 ITEMS")
    );
    let labels = page_labels(&mut run.state);
    assert!(labels.contains("completion was not recorded"), "{labels}");
    run.state.rollback_confirmation_text = "ROLL BACK 2 ITEMS".into();
    run.state.rollback_with_journal_dir(&run.journal);
    assert_eq!(
        run.state.execution_stage,
        PublisherExecutionStage::RolledBack
    );
    for link in &run.published {
        assert!(std::fs::symlink_metadata(link).is_err());
    }
}

#[test]
fn rollback_is_not_called_complete_while_a_created_directory_remains() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let mut run = reshaped_run(
        &[EntryState::Applied, EntryState::Applied],
        TransactionState::Applied,
    );
    let directory = run.published[0].parent().unwrap().to_path_buf();
    assert!(
        run.state
            .execution_transaction
            .as_ref()
            .unwrap()
            .transaction
            .created_directories
            .contains(&directory)
    );
    let stranger = directory.join("user-note.txt");
    std::fs::write(&stranger, b"not ours").unwrap();
    run.state.rollback_confirmation_text = "ROLL BACK 2 ITEMS".into();
    run.state.rollback_with_journal_dir(&run.journal);
    assert_eq!(std::fs::read(&stranger).unwrap(), b"not ours");
    assert_eq!(
        run.state.execution_stage,
        PublisherExecutionStage::NeedsReconciliation
    );
    let message = run.state.execution_error.clone().expect("explained");
    assert!(
        message.contains(&directory.display().to_string()),
        "{message}"
    );
    assert!(run.temp.path().exists());
}

#[test]
fn partial_failure_keeps_a_rollback_route_for_what_the_journal_applied() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let (mut state, _temp) = recorded_outcome(
        &[EntryState::Applied, EntryState::ApplyFailed],
        TransactionState::ApplyFailed,
        vec![PathBuf::from("/out/created")],
    );
    assert_eq!(
        state.execution_stage,
        PublisherExecutionStage::NeedsReconciliation
    );
    assert_eq!(
        state.rollback_confirmation_phrase().as_deref(),
        Some("ROLL BACK 1 ITEMS")
    );
    let labels = page_labels(&mut state);
    assert!(
        labels.contains("1 item(s) applied and 1 destination"),
        "{labels}"
    );
    assert!(
        labels.contains("Roll back publisher transaction"),
        "{labels}"
    );
    assert!(!labels.contains("cannot reverse"), "{labels}");
}

#[test]
fn reconciliation_rollback_removes_real_links_without_touching_the_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"reconciliation source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.apply_with_journal_dir(&journal);
    let published = state
        .execution_transaction
        .as_ref()
        .unwrap()
        .transaction
        .entries[0]
        .destination_path
        .clone();
    assert!(std::fs::symlink_metadata(&published).is_ok());
    state.execution_stage = PublisherExecutionStage::NeedsReconciliation;
    state.rollback_confirmation_text = "ROLL BACK 1 ITEMS".into();
    state.rollback_with_journal_dir(&journal);
    assert_eq!(state.execution_stage, PublisherExecutionStage::RolledBack);
    assert!(std::fs::symlink_metadata(&published).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"reconciliation source");
    assert!(state.rollback_confirmation_phrase().is_none());
}

#[test]
fn real_refusal_after_creating_directories_reports_them_without_a_rollback_claim() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"refused source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    std::fs::remove_file(&source).unwrap();
    state.apply_with_journal_dir(&journal);
    let record = &state.execution_transaction.as_ref().unwrap().transaction;
    assert_eq!(record.applied_count(), 0);
    let directories = record.created_directories.len();
    assert!(directories > 0, "fixture must create directories first");
    assert!(state.execution_result.is_none());
    assert_eq!(
        state.execution_stage,
        PublisherExecutionStage::NeedsReconciliation
    );
    assert!(state.rollback_confirmation_phrase().is_none());
    let labels = page_labels(&mut state);
    assert!(
        labels.contains(&format!("0 item(s) applied and {directories} destination")),
        "{labels}"
    );
    assert!(labels.contains("cannot reverse"), "{labels}");
}

#[test]
fn partly_skipped_run_says_so_and_keeps_rollback_for_what_was_applied() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let (mut state, _temp) = recorded_outcome(
        &[
            EntryState::Applied,
            EntryState::Applied,
            EntryState::Applied,
            EntryState::Skipped,
            EntryState::Skipped,
        ],
        TransactionState::Applied,
        Vec::new(),
    );
    assert_eq!(state.execution_stage, PublisherExecutionStage::Applied);
    assert_eq!(
        state.rollback_confirmation_phrase().as_deref(),
        Some("ROLL BACK 3 ITEMS")
    );
    state.rollback_confirmation_text = "ROLL BACK 3 ITEMS".into();
    assert!(state.can_rollback());
    let labels = page_labels(&mut state);
    assert!(labels.contains("Export partly published"), "{labels}");
    assert!(
        labels.contains("3 published, 2 skipped and left untouched"),
        "{labels}"
    );
    assert!(!labels.contains("Published 3 item(s)"), "{labels}");
}

#[test]
fn failed_apply_is_a_plain_failure_only_when_the_record_shows_no_change() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionEntry};
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    std::fs::write(&source, b"outcome source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Symlink);
    state.execution_review();
    let mut record = state.execution_transaction.expect("reviewed").transaction;
    assert_eq!(
        stage_after_apply_error(&record),
        PublisherExecutionStage::Failed
    );
    let set_entries = |record: &mut archivefs_core::dat::rename_apply::RenameTransaction,
                       value: EntryState| {
        record
            .entries
            .iter_mut()
            .for_each(|entry: &mut TransactionEntry| entry.state = value);
    };
    // In flight with nothing yet Applied: the old check called this a failure.
    set_entries(&mut record, EntryState::Applying);
    assert_eq!(
        stage_after_apply_error(&record),
        PublisherExecutionStage::NeedsReconciliation
    );
    set_entries(&mut record, EntryState::Skipped);
    record.created_directories = vec![PathBuf::from("/out/created")];
    assert_eq!(
        stage_after_apply_error(&record),
        PublisherExecutionStage::NeedsReconciliation
    );
}

#[test]
fn all_skipped_run_that_created_directories_is_not_called_unchanged() {
    use archivefs_core::dat::rename_apply::{EntryState, TransactionState};
    let (state, _temp) = recorded_outcome(
        &[EntryState::Skipped, EntryState::Skipped],
        TransactionState::ApplyFailed,
        vec![PathBuf::from("/out/created")],
    );
    assert_eq!(
        state.execution_stage,
        PublisherExecutionStage::NeedsReconciliation
    );
    let message = state.execution_error.as_deref().expect("explained");
    assert!(message.contains("1 destination"), "{message}");
}

#[test]
fn export_profiles_preserve_user_frontend_files_through_apply_and_undo() {
    for target in [PublisherFrontend::RomM, PublisherFrontend::EsDe] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Game.adf");
        let destination = temp.path().join("frontend");
        let journal = temp.path().join("journal");
        std::fs::write(&source, b"synthetic game").unwrap();
        std::fs::create_dir(&destination).unwrap();
        let files = [
            (
                "gamelist.xml",
                b"<gameList><!-- user edit --><game><name>My name</name></game></gameList>"
                    .as_slice(),
            ),
            ("es_systems.xml", b"user configuration".as_slice()),
            ("cover.png", b"synthetic user artwork".as_slice()),
        ];
        for (name, content) in &files {
            std::fs::write(destination.join(name), content).unwrap();
        }
        let gamelist = destination.join("amiga/gamelist.xml");
        let artwork = destination.join("amiga/media/cover.png");
        std::fs::create_dir_all(artwork.parent().unwrap()).unwrap();
        std::fs::write(&gamelist, files[0].1).unwrap();
        std::fs::write(&artwork, files[2].1).unwrap();
        let mut state = executable_state(&source, &destination);
        state.target = Some(target);
        state.preview();
        state.link_mode = Some(PublisherLinkMode::Symlink);
        state.execution_review();
        state.confirmation_text = state.apply_confirmation_phrase().unwrap();
        state.apply_with_journal_dir(&journal);
        assert_eq!(
            state.execution_stage,
            PublisherExecutionStage::Applied,
            "{:?}",
            state.execution_error
        );
        for (name, content) in &files {
            assert_eq!(std::fs::read(destination.join(name)).unwrap(), *content);
        }
        state.rollback_confirmation_text = state.rollback_confirmation_phrase().unwrap();
        state.rollback_with_journal_dir(&journal);
        assert_eq!(state.execution_stage, PublisherExecutionStage::RolledBack);
        for (name, content) in &files {
            assert_eq!(std::fs::read(destination.join(name)).unwrap(), *content);
        }
        assert_eq!(std::fs::read(&gamelist).unwrap(), files[0].1);
        assert_eq!(std::fs::read(&artwork).unwrap(), files[2].1);
        assert_eq!(std::fs::read(&source).unwrap(), b"synthetic game");
    }
}

#[test]
fn export_profile_refuses_to_replace_user_edited_gamelist_in_game_slot() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("gamelist.xml");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic source").unwrap();
    let occupied = destination.join("amiga/gamelist.xml");
    std::fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    let user_edit = b"<gameList><!-- user names, play counts and artwork references --></gameList>";
    std::fs::write(&occupied, user_edit).unwrap();
    let mut state = executable_state(&source, &destination);
    state.target = Some(PublisherFrontend::EsDe);
    state.preview();
    let result = state.result.as_ref().unwrap();
    assert_eq!(result.summary().will_publish, 0);
    state.link_mode = Some(PublisherLinkMode::Symlink);
    state.execution_review();
    assert!(!state.can_apply());
    state.confirmation_text = "PUBLISH 1 ITEMS".into();
    state.apply_with_journal_dir(&temp.path().join("journal"));
    assert_eq!(std::fs::read(occupied).unwrap(), user_edit);
    assert!(!temp.path().join("journal").exists());
}

#[test]
fn reviewing_again_consumes_previous_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(PublisherLinkMode::Symlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    assert!(state.can_apply());
    state.execution_review();
    assert!(!state.can_apply());
}

#[test]
fn background_preview_is_read_only_and_retains_its_result() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.adf");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    let mut state = executable_state(&source, &destination);
    let context = egui::Context::default();
    state.start(PublisherJob::Preview, context.clone());
    assert!(state.is_busy());
    // A second request cannot replace the accepted job.
    state.start(PublisherJob::Apply, context.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while state.is_busy() {
        state.poll(&context);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(state.result.is_some());
    assert!(!destination.exists());
    assert_eq!(std::fs::read(source).unwrap(), b"synthetic");
}

fn rendered_labels(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
    fn gather(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.text().into(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    gather(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for shape in &output.shapes {
        gather(&shape.shape, &mut out);
    }
    out
}

fn page_labels(state: &mut PublisherProfilePageState) -> String {
    let context = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 2400.0));
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                show_publisher_profile_page(ui, state);
            });
        },
    );
    rendered_labels(&output)
        .into_iter()
        .map(|x| x.0)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn worker_failure_never_claims_no_destination_changes() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.execution_stage = PublisherExecutionStage::NeedsReconciliation;
    state.execution_error = Some("Synthetic worker failure".into());
    let context = egui::Context::default();
    let output = context.run(Default::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            show_publisher_profile_page(ui, &mut state);
        });
    });
    let labels = rendered_labels(&output)
        .into_iter()
        .map(|x| x.0)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(labels.contains("Synthetic worker failure"));
    assert!(!labels.contains("No destination changes were made"));
}

#[test]
fn confirmation_is_keyboard_editable_and_apply_reachable_at_1024_by_600() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(PublisherLinkMode::Symlink);
    state.execution_review();
    let context = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 600.0));
    let draw = |events, state: &mut PublisherProfilePageState| {
        context.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        show_publisher_profile_page(ui, state);
                    });
                });
            },
        )
    };
    draw(vec![], &mut state);
    let mut found = false;
    // Standard tab navigation must eventually focus the confirmation editor.
    for _ in 0..35 {
        draw(
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            &mut state,
        );
        draw(
            vec![egui::Event::Text("PUBLISH 1 ITEMS".into())],
            &mut state,
        );
        if state.confirmation_text == "PUBLISH 1 ITEMS" {
            found = true;
            break;
        }
        // Typing into the destination/system fields must not invalidate the fixture for subsequent focus steps.
        state.destination_root = destination.display().to_string();
        state.canonical_platform_id = "Amiga".into();
    }
    assert!(found, "confirmation editor was unreachable by Tab");
    assert!(state.can_apply());
    context.style_mut(|style| style.scroll_animation = egui::style::ScrollAnimation::none());
    draw(
        vec![egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        &mut state,
    );
    draw(vec![], &mut state);
    let output = draw(vec![], &mut state);
    assert!(
        rendered_labels(&output)
            .iter()
            .any(|(label, rect)| label == "Apply publisher plan" && screen.contains_rect(*rect)),
        "Apply was not scrolled into view by keyboard focus: focused={:?}, labels={:?}",
        context.memory(|memory| memory.focused()),
        rendered_labels(&output)
    );
    assert!(!destination.join("roms").exists());
}

#[test]
fn unsupported_frontend_platform_has_no_apply_path() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("frontend");
    std::fs::write(&source, b"synthetic").unwrap();
    let mut state = executable_state(&source, &destination);
    state.canonical_platform_id = "Synthetic unsupported platform".into();
    state.preview();
    assert_eq!(state.result.as_ref().unwrap().summary().will_publish, 0);
    state.link_mode = Some(PublisherLinkMode::Symlink);
    state.execution_review();
    assert!(!state.can_apply());
    state.confirmation_text = "PUBLISH 1 ITEMS".into();
    state.apply_with_journal_dir(&temp.path().join("journal"));
    assert!(!destination.exists());
    assert!(!temp.path().join("journal").exists());
}

#[test]
fn result_view_shows_the_journal_based_outcome_badge() {
    use crate::transaction_outcome::test_support::rendered_text;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Game.rom");
    let destination = temp.path().join("published");
    let journal = temp.path().join("journal");
    std::fs::write(&source, b"badge source").unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut state = executable_state(&source, &destination);
    state.link_mode = Some(archivefs_core::publisher_profile::PublisherLinkMode::Hardlink);
    state.execution_review();
    state.confirmation_text = state.apply_confirmation_phrase().unwrap();
    state.apply_with_journal_dir(&journal);
    let render = |state: &mut PublisherProfilePageState| {
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_publisher_profile_page(ui, state);
            });
        });
        rendered_text(&output)
    };
    let applied = render(&mut state);
    assert!(applied.contains("Completed"), "{applied}");
    assert!(applied.contains("1 applied"), "{applied}");
    assert!(applied.contains("Undo available now"), "{applied}");
    state.rollback_confirmation_text = state.rollback_confirmation_phrase().unwrap();
    state.rollback_with_journal_dir(&journal);
    let undone = render(&mut state);
    assert!(undone.contains("Undone"), "{undone}");
    assert!(!undone.contains("Undo available"), "{undone}");
}
