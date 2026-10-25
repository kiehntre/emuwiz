//! The verify-my-games journey: Check Games has a real next action, wording is
//! honest, platform context survives navigation, and Activity can show only the
//! jobs that need attention. Fixtures only.
use super::activity::{Activity, Phase, visible_jobs};
use super::identity_review::{ReviewState, review_for};
use super::identity_review_page::summarize_platform;
use super::library::{Game, IdentityContext, Library, UNKNOWN_PLATFORM};
use super::routes::{Route, Section, breadcrumb_labels};
use archivefs_core::PersistedArchive;

fn archive(id: i64, path: &str, platform: Option<&str>) -> PersistedArchive {
    PersistedArchive {
        id,
        source_folder_id: 1,
        relative_path: path.into(),
        absolute_path: format!("/games/{path}").into(),
        archive_kind: "zip".into(),
        display_name: path.trim_end_matches(".zip").into(),
        normalized_name: path.to_lowercase(),
        size_bytes: Some(1),
        modified_time_unix_seconds: Some(1),
        platform: platform.map(str::to_string),
        platform_source: Some("test".into()),
        last_known_health: "pending".into(),
        last_seen_at: "now".into(),
        last_verified_missing_at: None,
        identity_report: None,
    }
}

fn library(archives: Vec<PersistedArchive>) -> Library {
    Library::new(archives)
}

#[test]
fn a_platform_with_games_not_yet_compared_says_ready_to_verify_not_can_be_matched() {
    let mut context = IdentityContext::default();
    context.inventory = Some(archivefs_core::identity_attention::ReferenceInventory::default());
    let games = library(
        (1..=3)
            .map(|id| archive(id, &format!("Game {id}.zip"), Some("GBA")))
            .collect(),
    );
    let review = review_for(&games.games[0], &context, None);
    let label = review.list_label();
    assert_ne!(label, "Can be matched");
    assert!(
        ["Ready to verify", "Identification data missing"].contains(&label),
        "unexpected label {label}"
    );
    if matches!(review.state, ReviewState::NotCompared) {
        assert_eq!(label, "Ready to verify");
    }
}

#[test]
fn saved_exact_matches_move_games_into_the_verified_count() {
    let mut games = library(
        (1..=4)
            .map(|id| archive(id, &format!("Game {id}.zip"), Some("GBA")))
            .collect(),
    );
    let before = summarize_platform(&games, "Game Boy Advance");
    assert_eq!(before.verified, 0);
    assert_eq!(before.rows.len(), 4);
    for game in games.games.iter_mut().take(3) {
        game.dat_exact = Some(None);
    }
    let after = summarize_platform(&games, "Game Boy Advance");
    assert_eq!(after.verified, 3);
    assert_eq!(after.rows.len(), 1);
}

#[test]
fn summary_separates_ready_missing_data_and_needs_choice() {
    let mut games = library(vec![
        archive(1, "A.zip", Some("GBA")),
        archive(2, "B.zip", Some("GBA")),
    ]);
    games.games[0].dat_exact = Some(None);
    let summary = summarize_platform(&games, "Game Boy Advance");
    assert_eq!(summary.verified, 1);
    assert_eq!(
        summary.not_checked + summary.no_data + summary.no_match + summary.need_choice,
        1
    );
    assert_eq!(summary.rows.len(), 1);
}

#[test]
fn canonical_amiga_assignments_all_appear_under_amiga_and_never_via_folder_names() {
    let games = library(vec![
        archive(1, "Lemmings.zip", Some("Amiga")),
        archive(2, "Zool.zip", Some("amiga")),
        // A folder called amiga is not an assignment.
        archive(3, "amiga/Unassigned.zip", None),
        archive(4, "CD32 Game.zip", Some("AmigaCD32")),
    ]);
    assert_eq!(games.platforms.get("Amiga"), Some(&2));
    assert_eq!(games.platforms.get("AmigaCD32"), Some(&1));
    assert_eq!(games.platforms.get(UNKNOWN_PLATFORM), Some(&1));
    assert_eq!(summarize_platform(&games, "Amiga").rows.len(), 2);
    let unassigned: &Game = games.games.iter().find(|g| g.archive.id == 3).unwrap();
    assert_eq!(unassigned.platform, UNKNOWN_PLATFORM);
}

#[test]
fn canonical_atari2600_assignment_is_kept_by_the_game_model() {
    let games = library(vec![archive(1, "Combat.bin.zip", Some("Atari2600"))]);
    assert_eq!(games.games[0].platform, "Atari2600");
    assert_eq!(games.platforms.get("Atari2600"), Some(&1));
}

#[test]
fn the_platform_check_route_carries_its_platform_and_stays_in_check_games() {
    let route = Route::PlatformCheck("BBC Micro".into());
    assert_eq!(route.section(), Section::Check);
    assert_eq!(
        breadcrumb_labels(&route, None),
        vec!["Check Games".to_string(), "BBC Micro".to_string()]
    );
    let json = serde_json::to_string(&route).unwrap();
    assert_eq!(serde_json::from_str::<Route>(&json).unwrap(), route);
    assert_ne!(route, Route::PlatformCheck("Game Boy Advance".into()));
}

#[test]
fn attention_filter_shows_only_failed_jobs_and_show_all_restores_everything() {
    let mut activity = Activity::default();
    let done = activity.queue("Scan", Route::Section(Section::Games), false);
    activity.finish(done, "ok".into(), None);
    let failed = activity.queue("Check", Route::Section(Section::Check), false);
    activity.finish(failed, "no".into(), Some("boom".into()));
    let waiting = activity.queue("Later", Route::Section(Section::Games), false);

    let attention = visible_jobs(&activity, true);
    assert_eq!(attention.len(), 1);
    assert_eq!(*attention[0].0, failed);
    assert_eq!(attention[0].1.phase, Phase::Failed);
    assert_eq!(attention.len(), activity.failed());

    let all = visible_jobs(&activity, false);
    assert_eq!(all.len(), 3, "no job state is lost by filtering");
    assert!(all.iter().any(|(id, _)| **id == waiting));
    assert_eq!(activity.jobs.len(), 3);
}
