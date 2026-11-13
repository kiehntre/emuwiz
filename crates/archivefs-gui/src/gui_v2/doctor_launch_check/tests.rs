use super::*;
use crate::gui_v2::launch_readiness_summary::{
    Finding, FirmwareSummary, IdentitySummary, SourceSummary,
};
use crate::gui_v2::routes::{Route, Section};

fn summary(status: ReadinessPresentationState, findings: Vec<Cause>) -> GameReadinessSummary {
    GameReadinessSummary {
        status,
        headline: "Card headline".into(),
        explanation: "Card explanation".into(),
        emulator: None,
        firmware: FirmwareSummary::Unknown,
        identity: IdentitySummary::NeedsReview,
        source: SourceSummary::Available,
        warnings: Vec::new(),
        primary_action: None,
        details: Vec::new(),
        freshness: ReadinessFreshness::Current,
        findings: findings.into_iter().map(finding).collect(),
        attempt: None,
        other_emulator_choosable: false,
    }
}

fn finding(cause: Cause) -> Finding {
    let (title, why, fix) = match cause {
        Cause::FirmwareMissing => (
            "Console startup software (BIOS / firmware) is needed",
            "Add your own BIOS.",
            Some(ReadinessAction::CheckFirmware),
        ),
        Cause::EmulatorMissing => (
            "No emulator is set up for this system",
            "No usable emulator.",
            Some(ReadinessAction::SetUpEmulator),
        ),
        Cause::GameFileUnavailable => (
            "The game file can't be found",
            "Its drive may be disconnected.",
            Some(ReadinessAction::CheckGamesFolder),
        ),
        Cause::FormatUnsupported => (
            "This kind of game file isn't supported for launching yet",
            "Your file is fine.",
            Some(ReadinessAction::ReviewProblem),
        ),
        Cause::PlatformUnsupported => (
            "Launching isn't supported for this system yet",
            "Not launched through this emulator.",
            None,
        ),
        _ => ("Other", "Other.", Some(ReadinessAction::ReviewProblem)),
    };
    Finding {
        cause,
        title,
        why,
        fix,
        technical: Vec::new(),
    }
}

use ReadinessPresentationState as S;

#[test]
fn missing_bios_needs_attention_with_check_bios_routed_to_firmware() {
    let d = diagnose(Some(&summary(
        S::NeedsFirmware,
        vec![Cause::FirmwareMissing],
    )));
    assert_eq!(d.verdict, Verdict::NeedsAttention);
    assert_eq!(
        d.reason,
        "Console startup software (BIOS / firmware) is needed"
    );
    assert_eq!(d.explanation, "Add your own BIOS.");
    assert_eq!(d.action, Some(ReadinessAction::CheckFirmware));
    assert_eq!(
        doctor_action_label(ReadinessAction::CheckFirmware),
        "Check BIOS / Firmware"
    );
    assert_eq!(
        route(ReadinessAction::CheckFirmware, 7),
        Some(Route::Section(Section::Firmware))
    );
}

#[test]
fn missing_emulator_offers_set_up_emulator() {
    let d = diagnose(Some(&summary(
        S::NeedsEmulator,
        vec![Cause::EmulatorMissing],
    )));
    assert_eq!(d.verdict, Verdict::NeedsAttention);
    assert_eq!(d.action, Some(ReadinessAction::SetUpEmulator));
    assert_eq!(
        route(ReadinessAction::SetUpEmulator, 7),
        Some(Route::Section(Section::Emulators))
    );
}

#[test]
fn missing_game_file_points_at_the_games_folder() {
    let d = diagnose(Some(&summary(
        S::SourceUnavailable,
        vec![Cause::GameFileUnavailable],
    )));
    assert_eq!(d.verdict, Verdict::NeedsAttention);
    assert_eq!(d.reason, "The game file can't be found");
    assert_eq!(d.action, Some(ReadinessAction::CheckGamesFolder));
}

#[test]
fn unsupported_only_when_every_finding_is_unsupported() {
    let d = diagnose(Some(&summary(S::Blocked, vec![Cause::FormatUnsupported])));
    assert_eq!(d.verdict, Verdict::Unsupported);
    assert_eq!(
        d.reason,
        "This kind of game file isn't supported for launching yet"
    );
    let d = diagnose(Some(&summary(S::Blocked, vec![Cause::PlatformUnsupported])));
    assert_eq!(d.verdict, Verdict::Unsupported);
    assert_eq!(
        d.action, None,
        "no workflow exists, so no button is invented"
    );
    // Something fixable alongside it leads instead.
    let d = diagnose(Some(&summary(
        S::Blocked,
        vec![Cause::EmulatorMissing, Cause::FormatUnsupported],
    )));
    assert_eq!(d.verdict, Verdict::NeedsAttention);
    assert_eq!(d.action, Some(ReadinessAction::SetUpEmulator));
}

#[test]
fn ready_only_for_a_current_passing_answer_with_no_findings() {
    let d = diagnose(Some(&summary(S::Ready, vec![])));
    assert_eq!(d.verdict, Verdict::Ready);
    assert_eq!(d.action, None, "Doctor never offers Play");
    let mut with_warning = summary(S::ReadyWithWarnings, vec![]);
    with_warning.warnings.push("Identity not verified".into());
    let d = diagnose(Some(&with_warning));
    assert_eq!(d.verdict, Verdict::Ready);
    assert!(d.explanation.contains("Identity not verified"));
}

#[test]
fn unknown_checking_and_stale_states_are_never_ready() {
    assert_eq!(diagnose(None).verdict, Verdict::NotChecked);
    assert_eq!(diagnose(None).action, None);
    let d = diagnose(Some(&summary(S::Checking, vec![])));
    assert_eq!(d.verdict, Verdict::Checking);
    assert_eq!(verdict_label(d.verdict), "Checking");
    assert_eq!(d.action, None);
    assert_eq!(checking().verdict, Verdict::Checking);
    let mut stale = summary(S::Ready, vec![]);
    stale.freshness = ReadinessFreshness::Stale;
    stale.primary_action = Some(ReadinessAction::RecheckReadiness);
    let d = diagnose(Some(&stale));
    assert_eq!(d.verdict, Verdict::NeedsAttention);
    assert_eq!(doctor_action_label(d.action.unwrap()), "Open Game Details");
    assert_eq!(route(d.action.unwrap(), 7), Some(Route::Game(7)));
    // A blocked answer with no finding keeps the card's own reason.
    let mut blocked = summary(S::Blocked, vec![]);
    blocked.primary_action = Some(ReadinessAction::ReviewProblem);
    let d = diagnose(Some(&blocked));
    assert_eq!(d.verdict, Verdict::NeedsAttention);
    assert_eq!(d.reason, "Card headline");
    assert_eq!(d.action, Some(ReadinessAction::ReviewProblem));
}

#[test]
fn play_routes_to_the_launch_page_never_starts_a_game() {
    assert_eq!(
        doctor_action_label(ReadinessAction::Play),
        "Open Launch page"
    );
    assert_eq!(
        route(ReadinessAction::Play, 7),
        Some(Route::Task {
            section: Section::Launch,
            game: 7
        })
    );
}

#[test]
fn status_and_action_fit_a_1024_by_600_window() {
    let context = egui::Context::default();
    let diagnosis = diagnose(Some(&summary(
        S::NeedsFirmware,
        vec![Cause::FirmwareMissing],
    )));
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| {
                show(ui, Some("Some Game"), &diagnosis, &mut String::new());
            });
        },
    );
    for needle in ["! Needs attention", "Check BIOS / Firmware"] {
        let rects = crate::gui_v2::tests::text_bounds(&output, needle);
        assert!(!rects.is_empty(), "{needle} not drawn");
        assert!(
            rects.iter().all(|r| r.max.y <= 600.0),
            "{needle} below the fold"
        );
    }
}

#[test]
fn setup_and_doctor_leads_with_the_launch_check_above_the_fold_at_1024_by_600() {
    use crate::gui_v2::tests::{fixture, frame, text, text_bounds};
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.welcome_dismissed = true;
    app.router.current = Route::Section(Section::Setup);
    frame(&context, &mut app, [1024.0, 600.0]);
    let output = frame(&context, &mut app, [1024.0, 600.0]);
    let strings = text(&output);
    assert!(strings.iter().any(|s| s == NO_SELECTION_LINE));
    // No game is selected: reported as not checked, never as ready.
    assert!(strings.iter().any(|s| s == "· Not checked"));
    assert!(!strings.iter().any(|s| s == "Ready"));
    for needle in ["· Not checked", NO_SELECTION_LINE] {
        let rects = text_bounds(&output, needle);
        assert!(!rects.is_empty(), "{needle} not drawn");
        assert!(
            rects.iter().all(|r| r.max.y <= 600.0),
            "{needle} below the fold"
        );
    }
}

fn picker_library() -> crate::gui_v2::library::Library {
    use crate::gui_v2::tests::archive;
    crate::gui_v2::library::Library::new(vec![
        archive(1, "Super Mario World", Some("SNES")),
        archive(2, "Sonic the Hedgehog", Some("Mega Drive")),
        archive(3, "Mario Kart 64", Some("N64")),
    ])
}

#[test]
fn picker_search_matches_every_word_in_title_or_platform() {
    let library = picker_library();
    let titles = |query: &str| {
        matches(&library.games, query)
            .shown
            .into_iter()
            .map(|game| game.title.clone())
            .collect::<Vec<_>>()
    };
    assert!(titles("").is_empty(), "an empty query lists nothing");
    assert!(titles("   ").is_empty());
    let mario = titles("mario");
    assert_eq!(mario.len(), 2);
    assert!(mario.iter().all(|title| title.contains("Mario")));
    assert_eq!(titles("mario 64").len(), 1);
    assert_eq!(titles("zelda").len(), 0);
}

#[test]
fn picker_limits_the_match_list() {
    use crate::gui_v2::tests::archive;
    let many = crate::gui_v2::library::Library::new(
        (1..=20)
            .map(|id| archive(id, &format!("Game {id}"), Some("SNES")))
            .collect(),
    );
    let found = matches(&many.games, "game");
    assert_eq!(found.shown.len(), PICKER_LIMIT);
    assert_eq!(found.total, 20);
    assert_eq!(found.hidden(), 12);
}

fn ranked(titles: &[(&str, &str)], query: &str) -> Vec<String> {
    use crate::gui_v2::tests::archive;
    let library = crate::gui_v2::library::Library::new(
        titles
            .iter()
            .zip(1..)
            .map(|((title, platform), id)| archive(id, title, Some(platform)))
            .collect(),
    );
    matches(&library.games, query)
        .shown
        .into_iter()
        .map(|game| game.title.clone())
        .collect()
}

#[test]
fn picker_ranks_exact_then_prefix_then_words_then_other() {
    let got = ranked(
        &[
            ("Super Mario World", "SNES"),
            ("Dr. Mario", "NES"),
            ("Mariokart Clone", "PC"),
            ("Mario Kart 64", "N64"),
            ("Mario", "NES"),
            ("Supermario Remix", "PC"),
        ],
        "mario",
    );
    assert_eq!(
        got,
        [
            "Mario",             // exact
            "Mariokart Clone",   // prefix (library order among prefixes)
            "Mario Kart 64",     // prefix
            "Super Mario World", // word
            "Dr. Mario",         // word
            "Supermario Remix",  // inside a word
        ]
    );
}

#[test]
fn picker_exact_match_ignores_case_spacing_and_punctuation() {
    let got = ranked(
        &[
            ("Zelda: A Link to the Past Randomizer", "SNES"),
            ("The Legend of Zelda", "NES"),
            ("Zelda: A Link to the Past", "SNES"),
        ],
        "  ZELDA a link to   the past ",
    );
    assert_eq!(got[0], "Zelda: A Link to the Past");
    assert_eq!(got[1], "Zelda: A Link to the Past Randomizer");
    assert_eq!(got.len(), 2);
}

#[test]
fn picker_multi_word_prefix_and_platform_only_matches_rank_last() {
    let got = ranked(
        &[
            ("Kart Racer", "Mario"), // matches "mario kart" only via the platform
            ("Super Mario Kart", "SNES"),
            ("Mario Kart: Double Dash", "GameCube"),
        ],
        "mario kart",
    );
    assert_eq!(
        got,
        ["Mario Kart: Double Dash", "Super Mario Kart", "Kart Racer"]
    );
}

#[test]
fn picker_shows_the_best_matches_even_beyond_the_first_eight_in_library_order() {
    let mut titles: Vec<(String, &str)> = (1..=30)
        .map(|n| (format!("Tetris Clone {n}"), "PC"))
        .collect();
    titles.push(("Tetris".into(), "Game Boy"));
    let refs: Vec<(&str, &str)> = titles.iter().map(|(t, p)| (t.as_str(), *p)).collect();
    let got = ranked(&refs, "tetris");
    assert_eq!(
        got[0], "Tetris",
        "the exact title is listed first though it is last in the library"
    );
    assert_eq!(got.len(), PICKER_LIMIT);
    // Equal relevance keeps library order (stable).
    assert_eq!(&got[1..3], ["Tetris Clone 1", "Tetris Clone 2"]);
}

#[test]
fn picker_overflow_note_says_how_many_are_hidden() {
    assert_eq!(overflow_note(0), None);
    assert!(
        overflow_note(1)
            .unwrap()
            .starts_with("1 more match not shown")
    );
    assert!(
        overflow_note(12)
            .unwrap()
            .starts_with("12 more matches not shown")
    );
}

#[test]
fn a_picked_game_is_kept_until_another_game_is_opened_elsewhere() {
    let mut picker = Picker::default();
    assert_eq!(
        picker.current(Some(1)),
        None,
        "default follows the last opened game"
    );
    picker.pick(2);
    assert!(picker.query.is_empty());
    // The pick frame: the check for game 2 has not been started yet.
    assert_eq!(picker.current(Some(1)), Some(2));
    // The check now belongs to game 2.
    assert_eq!(picker.current(Some(2)), Some(2));
    // Another game opened in Browse & Play: follow it again.
    assert_eq!(picker.current(Some(3)), None);
    assert_eq!(picker.game, None);
}

#[test]
fn picking_a_game_checks_that_game_and_never_shows_another_games_answer() {
    use crate::gui_v2::tests::{fixture, frame, text};
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = std::sync::Arc::new(picker_library());
    app.welcome_dismissed = true;
    app.router.current = Route::Section(Section::Setup);
    app.doctor_picker.query = "sonic".into();
    let strings = text(&frame(&context, &mut app, [1024.0, 600.0]));
    assert!(
        strings
            .iter()
            .any(|s| s.starts_with("Sonic the Hedgehog ("))
    );
    assert!(!strings.iter().any(|s| s.starts_with("Super Mario World (")));
    app.doctor_picker.pick(2);
    for _ in 0..3 {
        let strings = text(&frame(&context, &mut app, [1024.0, 600.0]));
        assert!(strings.iter().any(|s| s == "Sonic the Hedgehog"));
        assert!(
            !strings.iter().any(|s| s == "Ready"),
            "never Ready without an answer"
        );
        // The existing readiness check now belongs to the picked game.
        let on_screen = app
            .native_workflows
            .as_ref()
            .and_then(|workflows| workflows.selected_game_on_screen())
            .map(|(id, _)| id);
        assert_eq!(on_screen, Some(2));
    }
    assert_eq!(app.doctor_picker.game, Some(2));
}
