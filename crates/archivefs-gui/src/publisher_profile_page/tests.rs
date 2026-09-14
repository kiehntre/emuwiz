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
