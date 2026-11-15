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
    assert_eq!(
        identity_wording(Some(&pending), &game, None).label,
        "Checking verification"
    );
    assert!(!disproves_library_match(&game, None));
}

#[test]
fn current_proof_from_the_panel_is_verified() {
    let game = game(1, true);
    let verified = identity(&game, Some(&answer("Verified")));
    assert!(verified.is_verified());
    assert_eq!(
        identity_wording(Some(&verified), &game, None).label,
        "Verified"
    );
    assert!(!disproves_library_match(&game, Some(&answer("Verified"))));
}

/// Every setup or coverage answer the panel classifies as "Needs setup",
/// plus the other answers that establish nothing about this game's proof.
const NOT_ESTABLISHED: [&str; 9] = [
    "Catalogue selection required",
    "Catalogue system coverage unresolved",
    "Catalogue does not cover this system",
    "Catalogue not validated or unavailable",
    "No trusted DAT configured",
    "Configured DAT unavailable or disabled",
    "Catalogue status unavailable",
    "Verification evidence unavailable",
    "File format unsupported for exact verification",
];

#[test]
fn setup_gaps_errors_and_unknown_answers_are_not_confirmed_and_keep_the_badge() {
    use super::super::game_verification_guidance::Outcome;
    let game = game(1, true);
    let unknown = [
        "Saved exact match; freshness needs checking",
        "Something new",
    ];
    for reason in NOT_ESTABLISHED.into_iter().chain(unknown) {
        let settled = identity(&game, Some(&answer(reason)));
        assert_eq!(
            classify(&game, &answer(reason)),
            ProofCheck::NotEstablished,
            "{reason}"
        );
        assert_eq!(settled, DetailIdentity::NotConfirmed, "{reason}");
        assert_eq!(
            identity_wording(Some(&settled), &game, None).label,
            "Verification not yet confirmed"
        );
        assert!(!settled.is_verified(), "{reason}");
        assert!(
            !disproves_library_match(&game, Some(&answer(reason))),
            "{reason}"
        );
    }
    // The two named setup gaps are what the panel itself calls "Needs setup".
    for reason in [
        "Catalogue selection required",
        "Catalogue system coverage unresolved",
    ] {
        assert_eq!(Outcome::of(reason), Outcome::NeedsSetup);
    }
}

fn explained(
    game: &Game,
    present: bool,
    unchanged: bool,
    freshness: archivefs_core::dat::library_identity_summary::DatProvenanceFreshness,
    catalogues: Result<
        &[archivefs_core::dat::catalogue_selection::InstalledCatalogueSummary],
        &str,
    >,
) -> Guidance {
    super::super::game_verification_guidance::explain(
        game, present, unchanged, 1, freshness, catalogues, None,
    )
}

/// Pins the classification to the panel's real answers, so a wording change
/// there cannot silently turn evidence into "not established" or back.
#[test]
fn the_panels_real_answers_classify_strictly() {
    use archivefs_core::dat::library_identity_summary::DatProvenanceFreshness as F;
    let unflagged = game(2, false);
    let game = game(1, true);
    // A saved row is marked stale, or no row re-reads as current (#296).
    let stale = explained(&game, true, true, F::Stale, Ok(&[]));
    assert_eq!(
        classify(&game, &stale),
        ProofCheck::Invalidated,
        "{stale:?}"
    );
    assert!(disproves_library_match(&game, Some(&stale)));
    // The file object changed and no current proof covers it.
    let changed = explained(&unflagged, true, false, F::Unknown, Ok(&[]));
    assert_eq!(
        classify(&unflagged, &changed),
        ProofCheck::Invalidated,
        "{changed:?}"
    );
    // A file that is unavailable now (an unmounted drive) is not disproof.
    let missing = explained(&game, false, true, F::Current, Ok(&[]));
    assert_eq!(missing.reason, STALE_OR_CHANGED);
    assert_eq!(missing.explanation, FILE_UNAVAILABLE);
    assert_eq!(classify(&game, &missing), ProofCheck::NotEstablished);
    assert_eq!(
        identity(&game, Some(&missing)),
        DetailIdentity::NotConfirmed
    );
    // Catalogue status could not be read; no catalogue is configured.
    for answer in [
        explained(&game, true, true, F::Current, Err("unreadable")),
        explained(&game, true, true, F::Current, Ok(&[])),
    ] {
        assert_eq!(
            classify(&game, &answer),
            ProofCheck::NotEstablished,
            "{answer:?}"
        );
    }
}

#[test]
fn only_strict_invalidation_reads_recheck_needed() {
    let game = game(1, true);
    let stale = answer(STALE_OR_CHANGED);
    let settled = identity(&game, Some(&stale));
    assert_eq!(settled, DetailIdentity::RecheckNeeded);
    assert_eq!(
        identity_wording(Some(&settled), &game, None).label,
        "Re-check needed"
    );
    assert!(!settled.is_verified());
    // Once applied in memory, the game keeps saying so, whatever the panel
    // says on a later visit, until the Library reloads from the database.
    let mut invalidated = game.clone();
    invalidated.dat_exact = None;
    invalidated.dat_proof_invalidated = true;
    assert_eq!(invalidated.dat_identity_summary(), "Re-check needed");
    assert_eq!(invalidated.identity_summary(), "Re-check needed");
    for panel in [None, Some(answer("Audit needs review"))] {
        assert_eq!(
            identity(&invalidated, panel.as_ref()),
            DetailIdentity::RecheckNeeded
        );
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

/// Blue's guess is labelled by how it was reached, offers working routes,
/// and never claims verification.
#[test]
fn the_likely_match_card_is_honest_and_routes_to_review_and_check() {
    use super::super::routes::Route;
    use super::{
        CHECK_THIS_GAME, CHOOSE_ANOTHER_MATCH, GuessBasis, LikelyMatch, basis_text, likely_match,
        show_likely_match,
    };
    let game = super::super::library::Game::from_archive(archivefs_core::PersistedArchive {
        id: 9,
        source_folder_id: 1,
        relative_path: "Super Game (USA).sfc".into(),
        absolute_path: "/fixture/Super Game (USA).sfc".into(),
        archive_kind: "file".into(),
        display_name: "Super Game (USA)".into(),
        normalized_name: "super game".into(),
        size_bytes: Some(1),
        modified_time_unix_seconds: Some(1),
        platform: Some("SNES".into()),
        platform_source: Some("fixture".into()),
        last_known_health: "pending".into(),
        last_seen_at: "fixture".into(),
        last_verified_missing_at: None,
        identity_report: None,
    });
    let guess = likely_match(&game, None);
    assert_eq!(guess.basis, GuessBasis::FileName);
    let alternatives = LikelyMatch {
        basis: GuessBasis::FileName,
        alternatives: vec!["Super Game (Europe)".into()],
    };
    let painted = crate::shared_apply_warnings::test_support::render(|ui| {
        show_likely_match(ui, &game, &alternatives);
    });
    for wanted in [
        "Does this look like your game? Super Game (USA)",
        CHOOSE_ANOTHER_MATCH,
        CHECK_THIS_GAME,
        basis_text(GuessBasis::FileName),
        "Or: Super Game (Europe)",
    ] {
        assert!(painted.contains(wanted), "{wanted}: {painted}");
    }
    assert!(!painted.contains("Verified"), "{painted}");
    let _ = Route::GameCheck(9);
}

/// Game Details' one identity wording over every identity state and guess
/// basis. Only a verified review says Verified, a hint always says "not
/// verified", and no answer names a requirement (BIOS, emulator) or the
/// state of an action (Play, launch, blocked).
#[test]
fn identity_wording_table_never_overstates_or_mixes_in_requirements() {
    use super::super::identity_review::{DumpQuality, NoMatchReason, ReviewState, VerifiedFacts};
    use archivefs_core::LibraryDatState;
    let known = |state| {
        DetailIdentity::Known(Review {
            state,
            apparent_release: None,
        })
    };
    let verified = |by_file_evidence| {
        ReviewState::Verified(VerifiedFacts {
            title: "Amidar".into(),
            region: None,
            release: None,
            dump: DumpQuality::Clean,
            source: None,
            by_file_evidence,
        })
    };
    // (identified by the file's own evidence, saved catalogue state) and
    // what an unverified, uncompared answer then reads.
    let bases = [
        (
            false,
            None,
            "Name hint only · not verified",
            Confidence::Unknown,
        ),
        (true, None, "Likely · not verified", Confidence::Probable),
        (
            false,
            Some(LibraryDatState::Probable),
            "Possible match · not verified",
            Confidence::Probable,
        ),
        (
            true,
            Some(LibraryDatState::Ambiguous),
            "Several possible matches · not verified",
            Confidence::Ambiguous,
        ),
    ];
    let hints = [
        ReviewState::NotCompared,
        ReviewState::NoMatch(NoMatchReason::NotInData),
        ReviewState::NoMatch(NoMatchReason::WeakEvidenceOnly),
        ReviewState::NoMatch(NoMatchReason::NoUsableEvidence),
        ReviewState::NoData {
            reference_source_exists: true,
        },
        ReviewState::NoData {
            reference_source_exists: false,
        },
    ];
    let settled = [
        (known(verified(true)), "Verified", Confidence::Verified),
        (known(verified(false)), "Verified", Confidence::Verified),
        (
            DetailIdentity::Checking,
            "Checking verification",
            Confidence::Checking,
        ),
        (
            DetailIdentity::NotConfirmed,
            "Verification not yet confirmed",
            Confidence::Probable,
        ),
        (
            known(ReviewState::ReferenceMatched),
            "Matched to reference data",
            Confidence::Probable,
        ),
        (
            known(ReviewState::Ambiguous {
                candidates: Vec::new(),
            }),
            "Several possible matches",
            Confidence::Ambiguous,
        ),
        (
            known(ReviewState::Conflict {
                detail: "header and serial disagree".into(),
            }),
            "Evidence disagrees",
            Confidence::Ambiguous,
        ),
        (
            known(ReviewState::NoSystem),
            "Unknown · system not known",
            Confidence::Unknown,
        ),
    ];
    let requirement_or_action = [
        "BIOS",
        "firmware",
        "emulator",
        "Play",
        "launch",
        "Launch",
        "blocked",
        "Strong local",
        "Needs review",
    ];
    for (identified, dat_state, hint_label, hint_confidence) in bases {
        let mut game = game(1, false);
        game.identified = identified;
        game.dat_state = dat_state;
        let several = dat_state == Some(LibraryDatState::Ambiguous);
        let mut rows: Vec<(DetailIdentity, &str, Confidence)> = hints
            .iter()
            .cloned()
            .map(|state| (known(state), hint_label, hint_confidence))
            .collect();
        for (identity, label, confidence) in settled.iter().cloned() {
            // Several saved candidates make any unverified answer ambiguous.
            let confidence = match confidence {
                Confidence::Probable | Confidence::Unknown if several => Confidence::Ambiguous,
                other => other,
            };
            rows.push((identity, label, confidence));
        }
        for (identity, label, confidence) in rows {
            let wording = identity_wording(Some(&identity), &game, None);
            let case = format!("{identity:?} identified={identified} {dat_state:?}");
            assert_eq!(wording.label, label, "{case}");
            assert_eq!(wording.confidence, confidence, "{case}");
            assert_eq!(wording.is_verified(), identity.is_verified(), "{case}");
            assert_eq!(
                wording.label.starts_with("Verified"),
                wording.is_verified(),
                "{case}"
            );
            for text in std::iter::once(wording.label).chain(wording.note) {
                for word in requirement_or_action {
                    assert!(!text.contains(word), "{case}: {text}");
                }
            }
        }
    }
    let game = game(1, false);
    let recheck = identity_wording(Some(&DetailIdentity::RecheckNeeded), &game, None);
    assert_eq!(recheck.label, "Re-check needed");
    assert!(!recheck.is_verified());
    assert_eq!(
        identity_wording(None, &game, None).confidence,
        Confidence::Unknown
    );
}
