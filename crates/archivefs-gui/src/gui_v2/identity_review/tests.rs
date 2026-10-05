use super::*;
use archivefs_core::PersistedArchive;
use archivefs_core::dat::library_identity_summary::DatVerificationState as State;
use archivefs_core::identity_attention::ReferenceInventory;

pub(crate) fn game(id: i64, file: &str, platform: &str) -> Game {
    let mut game = Game::from_archive(PersistedArchive {
        id,
        source_folder_id: 1,
        relative_path: file.into(),
        absolute_path: format!("/games/{file}").into(),
        archive_kind: "zip".into(),
        display_name: file.trim_end_matches(".zip").into(),
        normalized_name: file.to_lowercase(),
        size_bytes: Some(1),
        modified_time_unix_seconds: Some(1),
        platform: Some(platform.into()),
        platform_source: Some("test".into()),
        last_known_health: "pending".into(),
        last_seen_at: "now".into(),
        last_verified_missing_at: None,
        identity_report: None,
    });
    game.platform = platform.into();
    game
}

fn known(state: State, title: Option<&str>) -> DatKnowledge {
    DatKnowledge {
        state,
        trusted_source: true,
        stale: false,
        title: title.map(str::to_string),
        region: Some("USA".into()),
        revision: None,
        source_name: "No-Intro".into(),
        ecosystem: Some("No-Intro"),
        candidates: Vec::new(),
        technical: "technical".into(),
    }
}

fn exact(title: &str) -> DatKnowledge {
    known(
        State::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        },
        Some(title),
    )
}

fn ctx() -> IdentityContext {
    IdentityContext::default()
}

fn verified_facts(review: &Review) -> &VerifiedFacts {
    match &review.state {
        ReviewState::Verified(facts) => facts,
        other => panic!("expected Verified, got {other:?}"),
    }
}

#[test]
fn a_unique_exact_authoritative_match_is_verified_automatically_with_nothing_to_confirm() {
    let review = review_for(
        &game(1, "Amidar (USA).zip", "Atari2600"),
        &ctx(),
        Some(&exact("Amidar (USA)")),
    );
    let facts = verified_facts(&review);
    assert_eq!(facts.title, "Amidar (USA)");
    assert_eq!(facts.region.as_deref(), Some("USA"));
    assert!(!facts.by_file_evidence);
    assert_eq!(review.list_label(), "Verified");
    assert_eq!(facts.dump, DumpQuality::Clean);
}

#[test]
fn a_thousand_exact_matches_need_no_user_action() {
    // The saved-match join marks the game; the review state needs nothing more.
    let context = ctx();
    let verified = (1..=1000)
        .filter(|id| {
            let mut game = game(*id, &format!("g{id}.zip"), "SNES");
            game.mark_dat_exact(Some("No-Intro"));
            review_for(&game, &context, None).is_verified()
        })
        .count();
    assert_eq!(
        verified, 1000,
        "every exact match is verified with zero clicks"
    );
    assert!(!review_for(&game(5000, "other.zip", "SNES"), &context, None).is_verified());
}

#[test]
fn special_releases_with_an_exact_match_verify_automatically_and_keep_their_class() {
    for (title, class) in [
        ("Super Game (USA) (Beta 2)", "Beta"),
        ("Super Game (Europe) (Proto)", "Prototype"),
        ("Super Game (USA) (Demo)", "Demo"),
        ("Super Game (USA) (Sample)", "Sample"),
        ("Super Game (USA) (Preview)", "Preview"),
        ("Super Game (USA) (Kiosk)", "Kiosk / store demo"),
        ("Super Game (USA) (Promo)", "Promotional"),
    ] {
        let review = review_for(&game(1, "x.zip", "SNES"), &ctx(), Some(&exact(title)));
        let facts = verified_facts(&review);
        assert_eq!(facts.release, Some(class), "{title}");
        assert_eq!(facts.dump, DumpQuality::Clean);
    }
}

#[test]
fn a_special_release_label_alone_neither_blocks_nor_creates_verification() {
    // label + no evidence: not verified, and the label is only an apparent class
    let review = review_for(
        &game(1, "Super Game (USA) (Beta).zip", "SNES"),
        &ctx(),
        None,
    );
    assert!(!review.is_verified());
    assert_eq!(review.apparent_release, Some("Beta"));
    assert!(review.explanation().contains("not a fault with the file"));
    // the same name with an exact authoritative match: verified, label retained
    let review = review_for(
        &game(1, "Super Game (USA) (Beta).zip", "SNES"),
        &ctx(),
        Some(&exact("Super Game (USA) (Beta)")),
    );
    assert!(review.is_verified());
}

#[test]
fn a_known_bad_dump_is_identified_but_never_called_clean() {
    for (title, quality, label) in [
        ("Game X (Beta) [b]", DumpQuality::KnownBad, "Known bad dump"),
        (
            "Game X (USA) (Bad Dump)",
            DumpQuality::KnownBad,
            "Known bad dump",
        ),
        ("Game X [o]", DumpQuality::Overdump, "Overdump"),
        ("Game X [f1]", DumpQuality::Fixed, "Fixed dump"),
        ("Game X [m]", DumpQuality::Modified, "Modified dump"),
        ("Game X [cr]", DumpQuality::Cracked, "Cracked"),
        ("Game X [t2]", DumpQuality::Trained, "Trained"),
        ("Game X [h]", DumpQuality::Hacked, "Hacked"),
    ] {
        let review = review_for(&game(1, "x.zip", "SNES"), &ctx(), Some(&exact(title)));
        let facts = verified_facts(&review);
        assert_eq!(facts.dump, quality, "{title}");
        assert_eq!(facts.dump.label(), Some(label));
    }
    assert_eq!(
        DumpQuality::from_dat_name("Game X (USA) [!]"),
        DumpQuality::Clean
    );
    assert_eq!(
        DumpQuality::from_dat_name("Game X (USA)"),
        DumpQuality::Clean
    );
    // the quality comes from the trusted DAT entry, never from the file's own name
    let review = review_for(
        &game(1, "Game X [b].zip", "SNES"),
        &ctx(),
        Some(&exact("Game X (USA)")),
    );
    assert_eq!(verified_facts(&review).dump, DumpQuality::Clean);
}

#[test]
fn a_research_source_without_an_adapter_cannot_become_verification_authority() {
    let mut research = exact("Unreleased Game (Proto)");
    research.trusted_source = false;
    research.source_name = "BetaArchive".into();
    research.ecosystem = None;
    let review = review_for(&game(1, "x.zip", "SNES"), &ctx(), Some(&research));
    assert!(!review.is_verified());
}

#[test]
fn weak_evidence_never_verifies() {
    for state in [State::Probable, State::FilenameOnlyNotVerified] {
        let review = review_for(
            &game(1, "x.zip", "SNES"),
            &ctx(),
            Some(&known(state, Some("Game"))),
        );
        assert_eq!(
            review.state,
            ReviewState::NoMatch(NoMatchReason::WeakEvidenceOnly)
        );
    }
    let none = review_for(
        &game(1, "x.zip", "SNES"),
        &ctx(),
        Some(&known(State::NoMatch, None)),
    );
    assert_eq!(none.state, ReviewState::NoMatch(NoMatchReason::NotInData));
    let nothing = review_for(
        &game(1, "x.zip", "SNES"),
        &ctx(),
        Some(&known(State::NoUsableEvidence, None)),
    );
    assert_eq!(
        nothing.state,
        ReviewState::NoMatch(NoMatchReason::NoUsableEvidence)
    );
}

#[test]
fn ambiguous_and_conflicting_matches_do_not_verify_and_are_not_auto_selected() {
    let mut ambiguous = known(
        State::AmbiguousMultipleCandidates {
            algorithm: "SHA-1".into(),
            candidate_count: 2,
        },
        None,
    );
    ambiguous.candidates = vec!["Game (USA)".into(), "Game (Europe)".into()];
    let review = review_for(&game(1, "x.zip", "SNES"), &ctx(), Some(&ambiguous));
    assert_eq!(
        review.state,
        ReviewState::Ambiguous {
            candidates: vec!["Game (USA)".into(), "Game (Europe)".into()]
        }
    );
    assert_eq!(review.list_label(), "Needs your choice");
    let conflict = known(
        State::Conflicting {
            detail: "filename suggests USA, hash matches Europe".into(),
        },
        None,
    );
    let review = review_for(&game(1, "x.zip", "SNES"), &ctx(), Some(&conflict));
    assert!(
        matches!(review.state, ReviewState::Conflict { ref detail } if detail.contains("Europe"))
    );
    assert!(!review.is_verified());
}

#[test]
fn a_game_with_no_system_does_not_verify_and_asks_for_one() {
    let review = review_for(&game(1, "x.zip", UNKNOWN_PLATFORM), &ctx(), None);
    assert_eq!(review.state, ReviewState::NoSystem);
    assert_eq!(review.list_label(), "Needs your choice");
}

#[test]
fn missing_identification_data_does_not_verify_and_is_named() {
    let mut context = ctx();
    context.inventory = Some(ReferenceInventory {
        platforms: Default::default(),
        ecosystems: Vec::new(),
        has_unattributed: false,
    });
    let review = review_for(&game(1, "Game.sfc", "SNES"), &context, None);
    assert_eq!(
        review.state,
        ReviewState::NoData {
            reference_source_exists: true
        }
    );
    assert_eq!(review.list_label(), "Identification data missing");
    let none = review_for(&game(2, "Game.d64", "Commodore 64"), &ctx(), None);
    assert!(!none.is_verified());
}

#[test]
fn stale_recorded_answers_are_ignored() {
    let mut stale = exact("Amidar (USA)");
    stale.stale = true;
    assert!(!review_for(&game(1, "x.zip", "SNES"), &ctx(), Some(&stale)).is_verified());
    assert!(DatKnowledge::best(vec![stale]).is_none());
}

#[test]
fn the_most_decisive_source_wins_among_several() {
    let best = DatKnowledge::best(vec![
        known(State::NoMatch, None),
        exact("Amidar (USA)"),
        known(State::Probable, Some("x")),
    ])
    .unwrap();
    assert!(matches!(best.state, State::VerifiedSingleMatch { .. }));
}

#[test]
fn a_summary_from_the_core_model_carries_trust_and_technical_detail() {
    use archivefs_core::dat::library_identity_summary::*;
    use archivefs_core::dat::model::DatEcosystem;
    let summary = LibraryDatIdentitySummary {
        verification_state: State::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        },
        source: DatSourceProvenance {
            source_id: "s".into(),
            source_name: "No-Intro Atari 2600".into(),
            ecosystem: Some(DatEcosystem::NoIntro),
            variant: None,
            source_revision: Some("2026".into()),
            author: None,
            catalogue_names: vec![],
            dat_path: "/dats/a.dat".into(),
        },
        canonical: DatCanonicalIdentity {
            canonical_dat_name: Some("Amidar (USA)".into()),
            canonical_rom_name: None,
            region: Some("USA".into()),
            revision: None,
        },
        hash_evidence: DatHashEvidenceSummary {
            matched_algorithm: Some("SHA-1".into()),
            matched_value: Some("abc".into()),
            available_algorithms: vec!["SHA-1".into()],
        },
        provenance_freshness: DatProvenanceFreshness::Unknown,
        ambiguous_candidates: vec![],
        candidate_provenance: vec![],
        set_dependency: DatSetDependencySummary::Pending {
            reason: "test".into(),
        },
    };
    let knowledge = DatKnowledge::from_summary(&summary);
    assert!(knowledge.trusted_source && !knowledge.stale);
    assert!(knowledge.technical.contains("abc") && knowledge.technical.contains("/dats/a.dat"));
    assert!(review_for(&game(1, "x.zip", "Atari2600"), &ctx(), Some(&knowledge)).is_verified());
}
