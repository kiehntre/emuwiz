use super::super::identity_review::{DumpQuality, NoMatchReason, VerifiedFacts};
use super::super::launch_readiness_summary::{Attempt, Finding, ReadinessFreshness, project};
use super::super::library::Library;
use super::super::tests::archive;
use super::*;
use crate::launch_readiness_page::LaunchReadinessInput;

fn game() -> Game {
    Library::new(vec![archive(1, "Amidar", Some("Atari2600"))])
        .games
        .remove(0)
}

fn known(state: ReviewState) -> DetailIdentity {
    DetailIdentity::Known(Review {
        state,
        apparent_release: None,
    })
}

fn verified(by_file_evidence: bool) -> ReviewState {
    ReviewState::Verified(VerifiedFacts {
        title: "Amidar".into(),
        region: None,
        release: None,
        dump: DumpQuality::Clean,
        source: None,
        by_file_evidence,
    })
}

fn tier(identity: Option<&DetailIdentity>, game: &Game) -> IdentityTier {
    identity_diagnosis(identity, game, None).tier
}

fn summary(status: ReadinessPresentationState) -> GameReadinessSummary {
    let mut summary = project(
        &LaunchReadinessInput::EvidenceNotLoaded,
        ReadinessFreshness::Current,
    );
    summary.status = status;
    summary.headline = "fixture headline".into();
    summary.firmware = FirmwareSummary::NotRequired;
    summary.findings.clear();
    summary.warnings.clear();
    summary
}

fn finding(cause: Cause, title: &'static str) -> Finding {
    Finding {
        cause,
        title,
        why: "fixture why",
        fix: None,
        technical: Vec::new(),
    }
}

#[test]
fn only_current_verified_review_is_verified() {
    let game = game();
    assert_eq!(
        tier(Some(&known(verified(true))), &game),
        IdentityTier::Verified
    );
    assert_eq!(
        tier(Some(&known(verified(false))), &game),
        IdentityTier::Verified
    );
    // A DAT match awaiting or lacking current proof is never verified.
    assert_eq!(
        tier(Some(&DetailIdentity::Checking), &game),
        IdentityTier::Checking
    );
    assert_eq!(
        tier(Some(&DetailIdentity::NotConfirmed), &game),
        IdentityTier::Probable
    );
    assert_ne!(
        tier(Some(&DetailIdentity::RecheckNeeded), &game),
        IdentityTier::Verified
    );
    assert_eq!(
        tier(Some(&known(ReviewState::ReferenceMatched)), &game),
        IdentityTier::Probable
    );
    assert_eq!(tier(None, &game), IdentityTier::Unknown);
}

#[test]
fn ambiguity_and_conflict_are_ambiguous_with_candidates() {
    let game = game();
    let ambiguous = known(ReviewState::Ambiguous {
        candidates: vec!["Amidar (USA)".into(), "Amidar (Europe)".into()],
    });
    let report = identity_diagnosis(Some(&ambiguous), &game, None);
    assert_eq!(report.tier, IdentityTier::Ambiguous);
    assert_eq!(report.candidates.len(), 2);
    let conflict = known(ReviewState::Conflict {
        detail: "header and serial disagree".into(),
    });
    let report = identity_diagnosis(Some(&conflict), &game, None);
    assert_eq!(report.tier, IdentityTier::Ambiguous);
    assert!(report.notes.iter().any(|n| n.contains("disagree")));
}

#[test]
fn unverified_identity_follows_the_existing_guess_basis() {
    let mut game = game();
    game.identified = false;
    game.dat_state = None;
    let not_compared = known(ReviewState::NotCompared);
    assert_eq!(tier(Some(&not_compared), &game), IdentityTier::Unknown);
    let no_match = known(ReviewState::NoMatch(NoMatchReason::WeakEvidenceOnly));
    assert_eq!(tier(Some(&no_match), &game), IdentityTier::Unknown);
    game.identified = true;
    assert_eq!(tier(Some(&not_compared), &game), IdentityTier::Probable);
    assert_eq!(
        tier(Some(&known(ReviewState::NoSystem)), &game),
        IdentityTier::Unknown
    );
}

#[test]
fn a_saved_ambiguous_dat_comparison_is_never_probable() {
    let mut game = game();
    game.identified = true;
    game.dat_state = Some(LibraryDatState::Ambiguous);
    let report = identity_diagnosis(Some(&known(ReviewState::NotCompared)), &game, None);
    assert_eq!(report.tier, IdentityTier::Ambiguous);
    // A verified answer is not downgraded by a stale saved comparison.
    assert_eq!(
        tier(Some(&known(verified(true))), &game),
        IdentityTier::Verified
    );
}

#[test]
fn unanswered_readiness_is_not_ready_or_missing() {
    assert_eq!(readiness_view(None), None);
    assert_eq!(
        readiness_view(Some(&summary(ReadinessPresentationState::Checking))),
        None
    );
}

#[test]
fn findings_firmware_and_warnings_are_listed_without_duplicates() {
    let mut blocked = summary(ReadinessPresentationState::NeedsFirmware);
    blocked.firmware = FirmwareSummary::Missing;
    blocked.findings = vec![finding(Cause::FirmwareMissing, "BIOS needed")];
    blocked.warnings = vec!["fixture warning".into()];
    let view = readiness_view(Some(&blocked)).unwrap();
    assert!(!view.checks_passed);
    let missing: Vec<_> = view
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Missing)
        .collect();
    assert_eq!(missing.len(), 1, "firmware listed once: {:?}", view.lines);
    assert!(
        view.lines
            .iter()
            .any(|l| l.kind == LineKind::Warning && l.detail == "fixture warning")
    );

    // Firmware missing with no matching finding still appears.
    blocked.findings = vec![finding(Cause::EmulatorMissing, "No emulator")];
    let view = readiness_view(Some(&blocked)).unwrap();
    assert_eq!(
        view.lines
            .iter()
            .filter(|l| l.kind == LineKind::Missing)
            .count(),
        2
    );

    // Unknown or present-but-unverified firmware is never reported as missing;
    // the Doctor adds nothing the card's own warnings do not say.
    blocked.status = ReadinessPresentationState::ReadyWithWarnings;
    blocked.findings.clear();
    blocked.firmware = FirmwareSummary::Unknown;
    blocked.warnings = vec!["Firmware is present but not verified.".into()];
    let view = readiness_view(Some(&blocked)).unwrap();
    assert!(view.lines.iter().all(|l| l.kind != LineKind::Missing));
    assert_eq!(view.lines.len(), 1);
    assert_eq!(view.status, "Launch checks passed with warnings");
}

#[test]
fn a_blocked_answer_without_findings_still_names_its_reason() {
    let mut blocked = summary(ReadinessPresentationState::Blocked);
    blocked.explanation = "fixture explanation".into();
    let view = readiness_view(Some(&blocked)).unwrap();
    assert!(!view.checks_passed);
    assert_eq!(view.explanation, "fixture explanation");
    let line = view
        .lines
        .iter()
        .find(|l| l.kind == LineKind::Missing)
        .unwrap();
    assert_eq!(line.title, "fixture headline");
    assert_eq!(line.detail, "fixture explanation");
}

#[test]
fn ambiguity_survives_a_recheck() {
    let mut game = game();
    game.dat_state = Some(LibraryDatState::Verified);
    let review = Review {
        state: ReviewState::Ambiguous {
            candidates: vec!["A".into(), "B".into()],
        },
        apparent_release: None,
    };
    let report = identity_diagnosis(Some(&DetailIdentity::RecheckNeeded), &game, Some(&review));
    assert_eq!(report.tier, IdentityTier::Ambiguous);
    assert_eq!(report.candidates.len(), 2);
}

#[test]
fn ready_means_checks_passed_never_played() {
    let mut ready = summary(ReadinessPresentationState::Ready);
    let view = readiness_view(Some(&ready)).unwrap();
    assert!(view.checks_passed);
    assert!(view.lines.is_empty());
    assert_eq!(view.status, "Launch checks passed");
    // A quick exit last time is surfaced as a warning, not a verdict.
    ready.attempt = Some(Attempt::EndedQuickly { seconds: 1 });
    let view = readiness_view(Some(&ready)).unwrap();
    let line = view
        .lines
        .iter()
        .find(|l| l.kind == LineKind::Warning)
        .unwrap();
    assert!(line.detail.contains("not a record of gameplay"));
    // The fixed disclaimer never claims play.
    for text in [NOT_PROOF_OF_PLAY, READINESS_PENDING] {
        let lower = text.to_lowercase();
        assert!(!lower.contains("works") && !lower.contains("will play."));
    }
    assert!(NOT_PROOF_OF_PLAY.contains("do not prove"));
}

#[test]
fn stale_answers_say_so() {
    let mut stale = summary(ReadinessPresentationState::Ready);
    stale.freshness = ReadinessFreshness::Stale;
    let view = readiness_view(Some(&stale)).unwrap();
    assert!(view.lines.iter().any(|l| l.title.contains("out of date")));
}

#[test]
fn the_doctor_renders_without_actions() {
    let report = report(
        identity_diagnosis(Some(&known(verified(true))), &game(), None),
        Some(&summary(ReadinessPresentationState::Ready)),
    );
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show(ui, 1, &report));
    });
}

#[test]
fn game_details_shows_the_doctor_closed_and_text_only() {
    use super::super::routes::Route;
    use super::super::tests::{fixture, frame, text};
    use std::sync::Arc;
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![archive(
        9,
        "Doctor Fixture",
        Some("PSX"),
    )]));
    app.router.current = Route::Game(9);
    let mut output = frame(&context, &mut app, [1280.0, 4000.0]);
    for _ in 0..3 {
        output = frame(&context, &mut app, [1280.0, 4000.0]);
    }
    let strings = text(&output);
    assert!(strings.iter().any(|s| s == "Game Doctor"), "{strings:?}");
    // Closed by default: its body is not drawn until opened.
    assert!(!strings.iter().any(|s| s == NOT_PROOF_OF_PLAY));
}
