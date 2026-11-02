use super::super::game_verification_guidance::{Action, Guidance};
use super::super::identity_review::review_for;
use super::super::library::{Game, IdentityContext, Library};
use super::super::tests::archive;
use super::*;

fn game(id: i64, dat_exact: bool) -> Game {
    let mut game = Library::new(vec![archive(id, "Amidar", Some("Atari2600"))])
        .games
        .remove(0);
    if dat_exact {
        game.mark_dat_exact(Some("No-Intro"));
    }
    game
}

fn answer(reason: &'static str) -> Guidance {
    Guidance {
        reason,
        explanation: String::new(),
        action: Action::Review,
    }
}

fn identity(game: &Game, panel: Option<&Guidance>) -> DetailIdentity {
    let context = IdentityContext::default();
    decide(review_for(game, &context, None), game, &context, panel)
}

#[test]
fn a_saved_dat_match_is_checking_until_this_games_panel_answers() {
    let game = game(1, true);
    // The canonical review alone would say Verified from the Library flag.
    assert!(review_for(&game, &IdentityContext::default(), None).is_verified());
    let pending = identity(&game, None);
    assert_eq!(pending, DetailIdentity::Checking);
    assert!(!pending.is_verified());
    assert_eq!(pending.label(), "Checking verification");
    assert!(!disproves_library_match(&game, None));
}

#[test]
fn current_proof_from_the_panel_is_verified() {
    let game = game(1, true);
    let verified = identity(&game, Some(&answer("Verified")));
    assert!(verified.is_verified());
    assert_eq!(verified.label(), "Verified");
    assert!(!disproves_library_match(&game, Some(&answer("Verified"))));
}

#[test]
fn stale_legacy_or_missing_proof_is_never_verified_and_disproves_the_flag() {
    let game = game(1, true);
    for reason in [
        // stale, changed or missing file
        "File evidence stale or changed",
        // legacy saved match whose freshness was never established
        "Saved exact match; freshness needs checking",
        // the catalogue behind the match is missing or disabled
        "Configured DAT unavailable or disabled",
        "No trusted DAT configured",
        "Audit needs review",
    ] {
        let settled = identity(&game, Some(&answer(reason)));
        assert!(!settled.is_verified(), "{reason}");
        assert_ne!(settled.label(), "Verified", "{reason}");
        assert_ne!(settled.label(), "Checking verification", "{reason}");
        assert!(
            disproves_library_match(&game, Some(&answer(reason))),
            "{reason}"
        );
    }
}

#[test]
fn an_unreadable_answer_is_not_confirmed_and_never_disproves() {
    let game = game(1, true);
    for reason in [
        "Verification evidence unavailable",
        "Catalogue status unavailable",
    ] {
        let settled = identity(&game, Some(&answer(reason)));
        assert_eq!(settled, DetailIdentity::NotConfirmed, "{reason}");
        assert_eq!(settled.label(), "Verification not yet confirmed");
        assert!(!disproves_library_match(&game, Some(&answer(reason))));
    }
}

#[test]
fn a_game_without_a_dat_match_is_unaffected_by_any_panel_answer() {
    let game = game(1, false);
    let expected = review_for(&game, &IdentityContext::default(), None);
    for panel in [
        None,
        Some(answer("Verified")),
        Some(answer("No catalogue match")),
    ] {
        assert_eq!(
            identity(&game, panel.as_ref()),
            DetailIdentity::Known(expected.clone())
        );
        assert!(!disproves_library_match(&game, panel.as_ref()));
    }
}
