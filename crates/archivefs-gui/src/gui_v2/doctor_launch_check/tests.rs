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
    assert_eq!(d.verdict, Verdict::NotChecked);
    assert_eq!(d.action, None);
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
                show(ui, Some("Some Game"), &diagnosis);
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
