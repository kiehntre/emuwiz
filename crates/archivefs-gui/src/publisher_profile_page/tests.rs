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
