use super::*;
use crate::gui_v2::{identity_review::NoMatchReason, library::Library};
use std::sync::Arc;

fn game() -> Game {
    Game::from_archive(crate::gui_v2::tests::archive(7, "Fixture", Some("Amiga")))
}

fn review() -> Review {
    Review {
        state: ReviewState::NotCompared,
        apparent_release: None,
    }
}

fn guidance(action: VerificationAction) -> Guidance {
    Guidance {
        reason: "Saved fixture state",
        explanation: "Review the existing preview before confirming a check.".into(),
        action,
    }
}

#[test]
fn saved_missing_file_overrides_dat_flags_and_does_not_claim_damage() {
    for health in ["pending", "missing", "corrupt"] {
        let mut game = game();
        game.archive.last_known_health = health.into();
        game.archive.last_verified_missing_at = Some("saved fixture check".into());
        game.dat_exact = Some(Some("Fixture DAT"));
        let next = advice(
            &game,
            &review(),
            true,
            Some(&guidance(VerificationAction::Audit)),
        );
        assert_eq!(next.action, Some(Action::FindMissingFile));
        assert!(next.message.contains("does not establish damaged media"));
        assert!(next.message.contains("drive may be disconnected"));
        assert_eq!(next.action.unwrap().route(&game), Route::LibraryFiles);
    }
}

#[test]
fn only_explicit_saved_media_damage_offers_repair_review() {
    for health in [
        "pending",
        "ok",
        "unsupported",
        "error",
        "corrupt",
        "damaged",
    ] {
        let mut game = game();
        game.archive.last_known_health = health.into();
        let next = advice(
            &game,
            &review(),
            true,
            Some(&guidance(VerificationAction::SelectDat)),
        );
        assert_eq!(
            next.action == Some(Action::RepairReview),
            matches!(health, "corrupt" | "damaged")
        );
        if health == "error" {
            assert_eq!(
                next.action,
                Some(Action::Verification(VerificationAction::Recheck))
            );
            assert!(next.message.contains("read error alone does not prove"));
        }
        if matches!(health, "corrupt" | "damaged") {
            assert!(next.message.contains("may already contain another file"));
            assert!(
                next.message
                    .contains("check that its scope matches this file")
            );
        }
    }
}

#[test]
fn absent_identity_facts_no_match_and_special_releases_do_not_offer_repairs() {
    let mut game = game();
    assert!(game.archive.identity_report.is_none());
    for state in [
        ReviewState::NotCompared,
        ReviewState::NoMatch(NoMatchReason::NoUsableEvidence),
        ReviewState::NoMatch(NoMatchReason::NotInData),
        ReviewState::Ambiguous {
            candidates: vec!["one".into(), "two".into()],
        },
        ReviewState::Conflict {
            detail: "recorded disagreement".into(),
        },
    ] {
        let review = Review {
            state,
            apparent_release: Some("Prototype"),
        };
        let next = advice(
            &game,
            &review,
            true,
            Some(&guidance(VerificationAction::SelectDat)),
        );
        assert_ne!(next.action, Some(Action::RepairReview));
        assert_ne!(next.action, Some(Action::FindMissingFile));
    }
    game.dat_state = Some(archivefs_core::LibraryDatState::NoMatch);
    assert_ne!(
        advice(
            &game,
            &review(),
            true,
            Some(&guidance(VerificationAction::Recheck))
        )
        .action,
        Some(Action::RepairReview)
    );
}

#[test]
fn verification_actions_reuse_existing_routes_and_preserve_saved_state() {
    let game = game();
    let before = serde_json::to_vec(&game.archive).unwrap();
    for (existing, label, route) in [
        (
            VerificationAction::SelectDat,
            "Choose DAT",
            Route::PlatformCheck("Amiga".into()),
        ),
        (
            VerificationAction::Validate,
            "Choose DAT",
            Route::Section(Section::Dat),
        ),
        (
            VerificationAction::Audit,
            "Verify",
            Route::PlatformCheck("Amiga".into()),
        ),
        (
            VerificationAction::Recheck,
            "Re-check",
            Route::PlatformCheck("Amiga".into()),
        ),
        (
            VerificationAction::Review,
            "Review Identity",
            Route::ReviewIdentity(7),
        ),
    ] {
        let next = advice(&game, &review(), true, Some(&guidance(existing)));
        let action = next.action.unwrap();
        assert_eq!(action, Action::Verification(existing));
        assert_eq!(action.label(), label);
        assert_eq!(action.route(&game), route);
    }
    assert_eq!(serde_json::to_vec(&game.archive).unwrap(), before);
    assert_eq!(game.dat_exact, None);
    assert_eq!(game.dat_state, None);
}

#[test]
fn pending_unknown_and_unavailable_states_do_not_invent_a_verification_action() {
    let game = game();
    assert!(
        advice(
            &game,
            &review(),
            false,
            Some(&guidance(VerificationAction::Audit))
        )
        .action
        .is_none()
    );
    assert!(advice(&game, &review(), true, None).action.is_none());
    let unknown = Review {
        state: ReviewState::NoSystem,
        apparent_release: None,
    };
    let next = advice(
        &game,
        &unknown,
        true,
        Some(&guidance(VerificationAction::Audit)),
    );
    assert_eq!(
        next.action,
        Some(Action::Verification(VerificationAction::Review))
    );
    assert!(next.message.contains("Choose system above"));
    let mut game = game.clone();
    game.platform = "unknown future system".into();
    assert_eq!(
        VerificationAction::Recheck.route(&game),
        Route::Section(Section::Check)
    );
}

fn texts(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
    fn visit(shape: &egui::Shape, found: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                found.push((text.galley.text().to_owned(), text.visual_bounding_rect()))
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for shape in &output.shapes {
        visit(&shape.shape, &mut found);
    }
    found
}

#[test]
fn mr_wiz_buttons_are_clickable_at_small_size_and_same_page_review_is_not_duplicated() {
    for (health, existing, expected) in [
        (
            "pending",
            VerificationAction::SelectDat,
            Some(Action::Verification(VerificationAction::SelectDat)),
        ),
        (
            "pending",
            VerificationAction::Audit,
            Some(Action::Verification(VerificationAction::Audit)),
        ),
        (
            "pending",
            VerificationAction::Recheck,
            Some(Action::Verification(VerificationAction::Recheck)),
        ),
        ("pending", VerificationAction::Review, None),
        (
            "missing",
            VerificationAction::Audit,
            Some(Action::FindMissingFile),
        ),
        (
            "damaged",
            VerificationAction::Audit,
            Some(Action::RepairReview),
        ),
    ] {
        let context = egui::Context::default();
        let mut game = game();
        game.archive.last_known_health = health.into();
        let mut clicked = None;
        let mut frame = |events| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1024.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        clicked = show(
                            ui,
                            &game,
                            &review(),
                            true,
                            Some(&guidance(existing)),
                            &Route::ReviewIdentity(7),
                        );
                    });
                },
            )
        };
        frame(vec![]);
        let shown = frame(vec![]);
        let labels = texts(&shown);
        assert!(labels.iter().any(|(text, _)| text.contains("Mr Wiz")));
        if let Some(action) = expected {
            let targets: Vec<_> = labels
                .iter()
                .filter(|(text, _)| text == action.label())
                .collect();
            assert_eq!(targets.len(), 1, "exactly one action: {labels:?}");
            let point = targets[0].1.center();
            assert!(point.y < 600.0);
            frame(vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ]);
            frame(vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }]);
            assert_eq!(clicked, Some(action));
        } else {
            assert!(!labels.iter().any(|(label, _)| label == "Review Identity"));
        }
    }
}

#[test]
fn navigation_selects_existing_workflow_without_queuing_work_or_changing_evidence() {
    for action in [
        Action::FindMissingFile,
        Action::RepairReview,
        Action::Verification(VerificationAction::SelectDat),
        Action::Verification(VerificationAction::Validate),
        Action::Verification(VerificationAction::Audit),
        Action::Verification(VerificationAction::Recheck),
        Action::Verification(VerificationAction::Review),
    ] {
        let context = egui::Context::default();
        let mut app = crate::gui_v2::tests::fixture(&context);
        app.library = Arc::new(Library::new(vec![game().archive]));
        app.router.current = Route::Game(7);
        let before = serde_json::to_vec(&app.library.game(7).unwrap().archive).unwrap();
        let jobs = app.activity.jobs.len();
        let expected = action.route(app.library.game(7).unwrap());
        app.run_identity_context_action(7, action);
        assert_eq!(app.router.current, expected);
        assert_eq!(app.activity.jobs.len(), jobs);
        assert!(app.native_workflows.is_none());
        assert_eq!(app.reconcile.runs_started, 0);
        assert_eq!(app.reconcile.hash_runs_started, 0);
        assert!(app.review.assign_job.is_none());
        assert_eq!(
            serde_json::to_vec(&app.library.game(7).unwrap().archive).unwrap(),
            before
        );
        if action == Action::RepairReview {
            assert_eq!(
                app.organisation.view,
                crate::gui_v2::organisation::OrganisationView::RepairReview
            );
        }
        if action == Action::FindMissingFile {
            let tab = crate::gui_v2::library_reconciliation::Tab::Missing;
            assert_eq!(app.reconcile.tab, Some(tab));
            assert_eq!(app.reconcile.filter[&tab], "/fixture/Fixture.iso");
        }
    }
}
