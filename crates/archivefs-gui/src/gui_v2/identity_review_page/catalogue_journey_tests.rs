use super::super::identity_review::{NoMatchReason, review_for};
use super::super::library::{IdentityContext, Library};
use super::*;

#[test]
fn verification_navigation_preserves_canonical_platform_and_never_mutates_identity() {
    let game = Game::from_archive(crate::gui_v2::tests::archive(1, "Fixture", Some("amiga")));
    for action in [
        super::super::game_verification_guidance::Action::SelectDat,
        super::super::game_verification_guidance::Action::Audit,
        super::super::game_verification_guidance::Action::Recheck,
    ] {
        assert_eq!(action.route(&game), Route::PlatformCheck("Amiga".into()));
    }
    assert_eq!(
        setup_verification_route(Some("unknown future platform")),
        Route::Section(Section::Check)
    );
}

#[test]
fn installed_data_guidance_requires_explicit_preview_and_confirmation() {
    let game = crate::gui_v2::tests::archive(1, "Fixture", Some("Amiga"));
    let library = Library::new(vec![game]);
    let context = IdentityContext::default();
    let mut review = review_for(&library.games[0], &context, None);
    review.state = ReviewState::NotCompared;
    let before = review.clone();
    let guidance = verification_guidance(&review);
    for text in [
        "no completed content check",
        "Preview",
        "confirm verification",
        "Not checked",
    ] {
        assert!(guidance.contains(text));
    }
    assert_eq!(review, before);
    review.state = ReviewState::NoData {
        reference_source_exists: true,
    };
    assert!(verification_guidance(&review).contains("installation alone does not check"));
    review.state = ReviewState::NoMatch(NoMatchReason::NoUsableEvidence);
    let guidance = verification_guidance(&review);
    assert!(guidance.contains("unsupported or unavailable"));
    assert!(!guidance.contains("could not read enough of this file"));
}

#[test]
fn dat_setup_keeps_reviewed_platform_when_opening_verification() {
    assert_eq!(
        setup_verification_route(Some("amiga")),
        Route::PlatformCheck("Amiga".into())
    );
    assert_eq!(
        setup_verification_route(None),
        Route::Section(Section::Check)
    );
    assert_eq!(
        setup_verification_route(Some("unknown future platform")),
        Route::Section(Section::Check)
    );
}
