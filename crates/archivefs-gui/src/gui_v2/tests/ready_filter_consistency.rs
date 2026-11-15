//! The Library's Ready view agrees with launch checks finished on other
//! screens (Game Details, Setup & Doctor, Big Picture). They all record into
//! the one Browse & Play readiness cache through `note_checked_readiness`,
//! and an answer counts only while the inputs it was taken under are
//! unchanged: the file (size, modified time, health), the library snapshot
//! and the emulator state (scans, BIOS evidence, emulator choice, settings).
use super::*;
use crate::gui_v2::browse_play::filtered_game_indices;
use crate::gui_v2::launch_readiness_summary::{
    self as summary, CompactReadiness, ReadinessPresentationState as State,
};
use crate::gui_v2::library::ReadinessFilter;

fn blue_app(context: &egui::Context, rows: Vec<PersistedArchive>) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    app.library = Arc::new(Library::new(rows));
    app.loaded = true;
    app.filter.cache_aware = true;
    app
}

fn rows() -> Vec<PersistedArchive> {
    (1..=3)
        .map(|id| archive(id, &format!("Ready Fixture {id}"), Some("SNES")))
        .collect()
}

/// A compact card as another screen draws it.
fn card(tone: State, pending: bool, spinner: bool) -> CompactReadiness {
    CompactReadiness {
        badge: "fixture",
        tone,
        spinner,
        reason: "fixture".into(),
        headline: "fixture".into(),
        play_enabled: matches!(tone, State::Ready | State::ReadyWithWarnings),
        fix: None,
        pending,
        more_findings: 0,
    }
}

fn finished(tone: State) -> CompactReadiness {
    card(tone, false, false)
}

fn game(app: &App, id: i64) -> Game {
    app.library.game(id).unwrap().clone()
}

fn shown_ids(app: &mut App, view: ReadinessFilter) -> Vec<i64> {
    app.filter.readiness = view;
    filtered_game_indices(app)
        .into_iter()
        .map(|index| app.library.games[index].archive.id)
        .collect()
}

/// Emulator scans finished, so the emulator state has a signature.
fn finished_emulators(app: &mut App, context: &egui::Context) {
    let workflows = app
        .native_workflows
        .get_or_insert_with(|| native_workflows::NativeWorkflows::new(context.clone()));
    workflows.app.emulator_readiness = crate::EmulatorReadinessState {
        retroarch_profiles: crate::RetroArchProfilesState::Ready(crate::tests::cheat_discovery(
            vec![crate::tests::cheat_profile("native-user", true)],
        )),
        dolphin_local_profiles: crate::DolphinLocalProfilesState::Error("none".into()),
        pcsx2_launch_profiles: crate::Pcsx2LaunchProfilesState::Error("none".into()),
        pcsx2_firmware_evidence: crate::Pcsx2FirmwareEvidenceState::Ready(Vec::new()),
        ..Default::default()
    };
}

#[test]
fn a_fresh_ready_from_another_screen_shows_in_the_ready_view() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, rows());
    assert!(shown_ids(&mut app, ReadinessFilter::Ready).is_empty());

    // Game Details / Doctor / Big Picture finished a check for game 2.
    let two = game(&app, 2);
    app.note_checked_readiness(&two, &finished(State::Ready));
    let three = game(&app, 3);
    app.note_checked_readiness(&three, &finished(State::ReadyWithWarnings));

    assert_eq!(shown_ids(&mut app, ReadinessFilter::Ready), vec![2, 3]);
    assert_eq!(shown_ids(&mut app, ReadinessFilter::NotChecked), vec![1]);
    assert!(shown_ids(&mut app, ReadinessFilter::NeedsAttention).is_empty());
}

#[test]
fn the_library_page_shows_a_ready_recorded_elsewhere() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, rows());
    let two = game(&app, 2);
    app.note_checked_readiness(&two, &finished(State::Ready));
    app.filter.readiness = ReadinessFilter::Ready;
    app.router.current = Route::BrowsePlay;
    frame(&context, &mut app, [1672.0, 941.0]);
    let strings = text(&frame(&context, &mut app, [1672.0, 941.0]));
    assert!(
        strings.iter().any(|s| s.contains("Ready Fixture 2")),
        "{strings:?}"
    );
    assert!(
        !strings.iter().any(|s| s.contains("Ready Fixture 1")),
        "{strings:?}"
    );
}

#[test]
fn a_changed_game_file_makes_the_answer_stale() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, rows());
    let two = game(&app, 2);
    app.note_checked_readiness(&two, &finished(State::Ready));
    assert_eq!(shown_ids(&mut app, ReadinessFilter::Ready), vec![2]);

    // The same game row, but the file on record is a different size now.
    let mut changed = rows();
    changed[1].size_bytes = Some(99);
    app.library = Arc::new(Library::new(changed.clone()));
    assert!(shown_ids(&mut app, ReadinessFilter::Ready).is_empty());
    assert!(shown_ids(&mut app, ReadinessFilter::NotChecked).contains(&2));

    // Likewise a new modified time or a new health result.
    for edit in [
        |row: &mut PersistedArchive| row.modified_time_unix_seconds = Some(7),
        |row: &mut PersistedArchive| row.last_known_health = "healthy".into(),
    ] {
        let mut app = blue_app(&context, rows());
        let two = game(&app, 2);
        app.note_checked_readiness(&two, &finished(State::Ready));
        let mut changed = rows();
        edit(&mut changed[1]);
        app.library = Arc::new(Library::new(changed));
        assert!(shown_ids(&mut app, ReadinessFilter::Ready).is_empty());
    }
}

#[test]
fn a_changed_emulator_setup_or_library_snapshot_makes_the_answer_stale() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, rows());
    finished_emulators(&mut app, &context);
    let (one, two, three) = (game(&app, 1), game(&app, 2), game(&app, 3));
    app.note_checked_readiness(&two, &finished(State::Ready));
    // Unchanged inputs: a later answer elsewhere keeps the earlier one.
    app.note_checked_readiness(&three, &finished(State::Ready));
    assert_eq!(shown_ids(&mut app, ReadinessFilter::Ready), vec![2, 3]);

    // The emulator choice changed (launch choices are part of the emulator
    // state every check reads): earlier answers were taken under the old one.
    readiness_state(&mut app).launch_choices.preferences = Err("changed".into());
    app.note_checked_readiness(&one, &finished(State::Ready));
    assert_eq!(
        shown_ids(&mut app, ReadinessFilter::Ready),
        vec![1],
        "only the answer taken under the new emulator state is Ready"
    );

    // A rescan moved the library snapshot.
    app.native_workflows
        .as_mut()
        .unwrap()
        .app
        .database_generation
        .0 += 1;
    app.note_checked_readiness(&two, &finished(State::Ready));
    assert_eq!(shown_ids(&mut app, ReadinessFilter::Ready), vec![2]);

    // An emulator rescan that has not finished leaves nothing to check
    // against, so nothing earlier stays Ready.
    readiness_state(&mut app).retroarch_profiles = crate::RetroArchProfilesState::NotScanned;
    app.note_checked_readiness(&three, &card(State::Checking, true, true));
    assert!(shown_ids(&mut app, ReadinessFilter::Ready).is_empty());
}

fn readiness_state(app: &mut App) -> &mut crate::EmulatorReadinessState {
    &mut app
        .native_workflows
        .as_mut()
        .unwrap()
        .app
        .emulator_readiness
}

#[test]
fn failed_unknown_stale_and_partial_answers_are_never_ready() {
    let context = egui::Context::default();
    let unknown = summary::compact(&summary::not_started_summary(), false);
    let answers = [
        ("not started", unknown),
        ("checking", card(State::Checking, true, true)),
        ("check errored / stale", card(State::Stale, true, false)),
        ("waiting on checksum", card(State::Checking, true, false)),
        ("ready but still pending", card(State::Ready, true, false)),
        ("missing emulator", finished(State::NeedsEmulator)),
        ("missing BIOS", finished(State::NeedsFirmware)),
        ("file unavailable", finished(State::SourceUnavailable)),
        ("blocked", finished(State::Blocked)),
    ];
    for (label, answer) in answers {
        let mut app = blue_app(&context, rows());
        let two = game(&app, 2);
        app.note_checked_readiness(&two, &answer);
        assert!(
            shown_ids(&mut app, ReadinessFilter::Ready).is_empty(),
            "{label} must not be Ready"
        );
        // A Ready followed by an answer that is not a current Ready (a
        // recheck running, a failure) is no longer Ready either.
        app.note_checked_readiness(&two, &finished(State::Ready));
        assert_eq!(shown_ids(&mut app, ReadinessFilter::Ready), vec![2]);
        app.note_checked_readiness(&two, &answer);
        assert!(
            shown_ids(&mut app, ReadinessFilter::Ready).is_empty(),
            "{label} after Ready must not stay Ready"
        );
    }
}
