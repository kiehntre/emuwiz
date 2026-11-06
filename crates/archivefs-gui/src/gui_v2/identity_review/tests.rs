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
        header: Default::default(),
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
    assert_eq!(nothing.list_label(), "Not checked");
    assert_eq!(nothing.headline(), "Not checked");
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

fn summary(state: State) -> LibraryDatIdentitySummary {
    use archivefs_core::dat::library_identity_summary::*;
    use archivefs_core::dat::model::DatEcosystem;
    LibraryDatIdentitySummary {
        verification_state: state,
        source: DatSourceProvenance {
            source_id: "s".into(),
            source_name: "No-Intro Atari 2600".into(),
            ecosystem: Some(DatEcosystem::NoIntro),
            variant: None,
            source_revision: Some("2026".into()),
            catalogue_sha256: None,
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
    }
}

#[test]
fn a_summary_from_the_core_model_carries_trust_and_technical_detail() {
    let summary = summary(State::VerifiedSingleMatch {
        algorithm: "SHA-1".into(),
    });
    let knowledge = DatKnowledge::from_summary(&summary);
    assert!(knowledge.trusted_source && !knowledge.stale);
    assert!(knowledge.technical.contains("abc") && knowledge.technical.contains("/dats/a.dat"));
    assert!(review_for(&game(1, "x.zip", "Atari2600"), &ctx(), Some(&knowledge)).is_verified());
}

// The native identity-review page displays this presentation in one wrapped
// label inside its existing Technical details disclosure. Exercise that same
// text/layout without starting the app, a worker, or a database read.
fn technical_frame(
    context: &eframe::egui::Context,
    knowledge: &DatKnowledge,
    size: eframe::egui::Vec2,
    events: Vec<eframe::egui::Event>,
) -> eframe::egui::FullOutput {
    use eframe::egui;
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.collapsing("Technical details", |ui| {
                    ui.label(&knowledge.technical);
                });
            });
        },
    )
}

fn rendered_text(output: &eframe::egui::FullOutput) -> String {
    use eframe::egui::Shape;
    fn walk(shape: &Shape, text: &mut String) {
        match shape {
            Shape::Text(value) => {
                text.push_str(value.galley.text());
                text.push('\n');
            }
            Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut text);
    }
    text
}

fn expand_technical_details(
    context: &eframe::egui::Context,
    knowledge: &DatKnowledge,
    size: eframe::egui::Vec2,
) -> eframe::egui::FullOutput {
    use eframe::egui;
    let closed = technical_frame(context, knowledge, size, vec![]);
    let header = closed
        .shapes
        .iter()
        .find_map(|clipped| {
            if let egui::Shape::Text(text) = &clipped.shape
                && text.galley.text() == "Technical details"
            {
                Some(text.pos + text.galley.size() * 0.5)
            } else {
                None
            }
        })
        .expect("existing Technical details disclosure is rendered");
    for pressed in [true, false] {
        let _ = technical_frame(
            context,
            knowledge,
            size,
            vec![
                egui::Event::PointerMoved(header),
                egui::Event::PointerButton {
                    pos: header,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    technical_frame(context, knowledge, size, vec![])
}

#[test]
fn catalogue_metadata_renders_in_the_existing_native_technical_details_disclosure() {
    use eframe::egui;
    let mut summary = summary(State::VerifiedSingleMatch {
        algorithm: "SHA-1".into(),
    });
    summary.source.catalogue_names = vec!["Atari - 2600".into()];
    summary.source.author = Some("DAT Team".into());
    let knowledge = DatKnowledge::from_summary(&summary);
    let context = egui::Context::default();
    context.style_mut(|style| style.animation_time = 0.0);
    let size = egui::vec2(800.0, 1600.0);
    let closed = rendered_text(&technical_frame(&context, &knowledge, size, vec![]));
    assert!(!closed.contains("Atari - 2600") && !closed.contains("DAT Team"));
    let open = rendered_text(&expand_technical_details(&context, &knowledge, size));
    assert!(
        open.contains("Matched with: SHA-1 abc"),
        "disclosure did not open: {open}"
    );
    for expected in [
        "DAT catalogue: Atari - 2600",
        "Catalogue author: DAT Team",
        "Source: No-Intro Atari 2600",
        "Matched with: SHA-1 abc",
    ] {
        assert!(open.contains(expected), "missing {expected:?}: {open}");
    }
    assert_eq!(knowledge.title.as_deref(), Some("Amidar (USA)"));
}

#[test]
fn catalogue_metadata_reaches_native_review_from_a_saved_synthetic_audit() {
    use crate::gui_v2::{dat_exact_tests, identity_review_page::knowledge_with_catalogues};
    let fixture = dat_exact_tests::fixture(&["Synthetic Game (USA)".into()]);
    dat_exact_tests::audit_and_save(&fixture);
    let before = std::fs::read(&fixture.db).unwrap();
    let knowledge = knowledge_with_catalogues(
        &fixture.db,
        1,
        &dat_exact_tests::catalogues(&fixture.db).unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(
        knowledge
            .technical
            .contains("DAT catalogue: Nintendo - Game Boy Color\n")
    );
    assert!(knowledge.technical.contains("Catalogue author: No-Intro\n"));
    assert_eq!(knowledge.title.as_deref(), Some("Synthetic Game (USA)"));
    assert!(matches!(knowledge.state, State::VerifiedSingleMatch { .. }));
    assert_eq!(
        std::fs::read(&fixture.db).unwrap(),
        before,
        "native review remains read-only"
    );
}

#[test]
fn catalogue_metadata_omits_missing_or_blank_values_and_never_infers_an_author() {
    let mut summary = summary(State::Probable);
    for author in [None, Some(String::new()), Some(" \t\n ".into())] {
        summary.source.catalogue_names = vec![String::new(), " \t ".into()];
        summary.source.author = author;
        let knowledge = DatKnowledge::from_summary(&summary);
        assert!(!knowledge.technical.contains("DAT catalogue:"));
        assert!(!knowledge.technical.contains("Catalogue author:"));
    }
    summary.source.catalogue_names = vec![" Atari - 2600 ".into()];
    let knowledge = DatKnowledge::from_summary(&summary);
    assert!(
        knowledge
            .technical
            .contains("DAT catalogue: Atari - 2600\n")
    );
    assert!(!knowledge.technical.contains("Catalogue author:"));
    summary.source.catalogue_names.clear();
    summary.source.author = Some(" DAT Team ".into());
    let knowledge = DatKnowledge::from_summary(&summary);
    assert!(knowledge.technical.contains("Catalogue author: DAT Team\n"));
    assert!(!knowledge.technical.contains("DAT catalogue:"));
}

#[test]
fn catalogue_metadata_lists_all_names_without_resolving_an_ambiguous_game() {
    let mut summary = summary(State::AmbiguousMultipleCandidates {
        algorithm: "SHA-1".into(),
        candidate_count: 2,
    });
    summary.canonical.canonical_dat_name = None;
    summary.ambiguous_candidates = vec!["Amidar (USA)".into(), "Amidar (Europe)".into()];
    summary.source.catalogue_names =
        vec!["Atari - 2600 (USA)".into(), "Atari - 2600 (Europe)".into()];
    summary.source.author = Some("DAT Team".into());
    let knowledge = DatKnowledge::from_summary(&summary);
    for name in &summary.source.catalogue_names {
        assert!(
            knowledge
                .technical
                .contains(&format!("DAT catalogue: {name}\n"))
        );
    }
    let review = review_for(&game(1, "x.zip", "Atari2600"), &ctx(), Some(&knowledge));
    assert!(!review.is_verified());
    assert_eq!(
        review.state,
        ReviewState::Ambiguous {
            candidates: summary.ambiguous_candidates
        }
    );
    assert_eq!(knowledge.title, None);
}

#[test]
fn catalogue_metadata_preserves_verification_freshness_confidence_and_other_facts() {
    use archivefs_core::dat::model::DatEcosystem;
    for state in [
        State::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        },
        State::Probable,
        State::AmbiguousMultipleCandidates {
            algorithm: "SHA-1".into(),
            candidate_count: 2,
        },
        State::Conflicting {
            detail: "Sources disagree".into(),
        },
        State::FilenameOnlyNotVerified,
        State::NoUsableEvidence,
        State::NoMatch,
    ] {
        for freshness in [
            DatProvenanceFreshness::Current,
            DatProvenanceFreshness::Stale,
            DatProvenanceFreshness::Unknown,
        ] {
            for ecosystem in [Some(DatEcosystem::NoIntro), None] {
                let mut summary = summary(state.clone());
                summary.provenance_freshness = freshness;
                summary.source.ecosystem = ecosystem;
                let before = DatKnowledge::from_summary(&summary);
                let review_before =
                    review_for(&game(1, "x.zip", "Atari2600"), &ctx(), Some(&before));
                summary.source.catalogue_names = vec!["Catalogue A".into(), "Catalogue B".into()];
                summary.source.author = Some("DAT Team".into());
                let mut after = DatKnowledge::from_summary(&summary);
                assert_eq!(
                    review_before,
                    review_for(&game(1, "x.zip", "Atari2600"), &ctx(), Some(&after))
                );
                for line in before.technical.lines() {
                    assert!(
                        after.technical.lines().any(|new_line| new_line == line),
                        "lost existing detail {line:?}"
                    );
                }
                after.technical = before.technical.clone();
                after.header = before.header.clone();
                assert_eq!(
                    before, after,
                    "metadata must only change technical presentation"
                );
            }
        }
    }
}

#[test]
fn catalogue_metadata_remains_with_its_source_and_does_not_change_source_selection() {
    let mut first = summary(State::Probable);
    first.source.source_id = "a".into();
    first.source.source_name = "Source A".into();
    first.source.catalogue_names = vec!["Catalogue A".into()];
    first.source.author = Some("Author A".into());
    let mut second = summary(State::VerifiedSingleMatch {
        algorithm: "SHA-1".into(),
    });
    second.source.source_id = "b".into();
    second.source.source_name = "Source B".into();
    second.source.catalogue_names = vec!["Catalogue B1".into(), "Catalogue B2".into()];
    second.source.author = None;
    for second_freshness in [
        DatProvenanceFreshness::Current,
        DatProvenanceFreshness::Stale,
    ] {
        second.provenance_freshness = second_freshness;
        let best = DatKnowledge::best(vec![
            DatKnowledge::from_summary(&first),
            DatKnowledge::from_summary(&second),
        ])
        .unwrap();
        if second_freshness == DatProvenanceFreshness::Current {
            assert_eq!(best.source_name, "Source B");
            assert!(
                best.technical.contains("Catalogue B1") && best.technical.contains("Catalogue B2")
            );
            assert!(
                !best.technical.contains("Catalogue A") && !best.technical.contains("Author A")
            );
            assert!(!best.technical.contains("Catalogue author:"));
        } else {
            assert_eq!(best.source_name, "Source A");
            assert!(best.technical.contains("Catalogue A") && best.technical.contains("Author A"));
            assert!(
                !best.technical.contains("Catalogue B1")
                    && !best.technical.contains("Catalogue B2")
            );
            assert_eq!(best.state, State::Probable);
        }
    }
}

#[test]
fn catalogue_metadata_wraps_long_unicode_values_at_narrow_width_and_large_scale() {
    use eframe::egui;
    let mut summary = summary(State::Probable);
    summary.source.catalogue_names = vec![
        "Atari - 2600 (Europe, accented édition française)".into(),
        "任天堂 カタログ 第二版".into(),
    ];
    summary.source.author = Some("Équipe de préservation des jeux et manuels".into());
    let knowledge = DatKnowledge::from_summary(&summary);
    for zoom in [1.0, 1.6] {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        context.set_zoom_factor(zoom);
        context.style_mut(|style| style.animation_time = 0.0);
        let output = expand_technical_details(&context, &knowledge, egui::vec2(420.0, 2200.0));
        let text = rendered_text(&output);
        for value in summary
            .source
            .catalogue_names
            .iter()
            .chain(summary.source.author.iter())
        {
            assert!(
                text.contains(value),
                "missing {value:?} at zoom {zoom}: {text}"
            );
        }
        let metadata = output
            .shapes
            .iter()
            .filter_map(|clipped| {
                if let egui::Shape::Text(text) = &clipped.shape
                    && text.galley.text().contains("édition")
                {
                    Some((clipped, text))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert!(!metadata.is_empty(), "metadata must actually render");
        for (clipped, text) in metadata {
            assert!(text.pos.x >= clipped.clip_rect.min.x - 1.0);
            assert!(
                text.pos.x + text.galley.size().x <= clipped.clip_rect.max.x + 1.0,
                "metadata clipped at zoom {zoom}"
            );
        }
        assert!(
            !review_for(&game(1, "x.zip", "Atari2600"), &ctx(), Some(&knowledge)).is_verified()
        );
    }
}

/// Click the disclosure on the actual selected-game page, then inspect its
/// painted provenance label. The existing app fixture avoids real DB workers.
fn native_provenance(knowledge: DatKnowledge) -> (String, String, Review) {
    native_provenance_with_file(knowledge, "ambiguous.gbc")
}

fn native_provenance_with_file(knowledge: DatKnowledge, file: &str) -> (String, String, Review) {
    use crate::gui_v2::{routes::Route, tests::review_app};
    use eframe::egui;
    let context = egui::Context::default();
    context.style_mut(|style| style.animation_time = 0.0);
    let mut app = review_app(&context, vec![(1, file, "GameBoyColor")]);
    app.review.knowledge = Some(knowledge);
    app.router.current = Route::ReviewIdentity(1);
    let mut frame = |events| {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 2000.0),
                )),
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| app.review_identity(ui, 1));
            },
        )
    };
    let closed = frame(vec![]);
    assert!(!rendered_text(&closed).contains("DAT catalogue:"));
    let header = closed
        .shapes
        .iter()
        .find_map(|clipped| {
            if let egui::Shape::Text(text) = &clipped.shape
                && text.galley.text() == "Technical details"
            {
                Some(text.pos + text.galley.size() * 0.5)
            } else {
                None
            }
        })
        .expect("native Technical details header");
    for pressed in [true, false] {
        frame(vec![
            egui::Event::PointerMoved(header),
            egui::Event::PointerButton {
                pos: header,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }
    let output = frame(vec![]);
    let provenance = output
        .shapes
        .iter()
        .find_map(|clipped| {
            if let egui::Shape::Text(text) = &clipped.shape
                && text.galley.text().starts_with("Source: ")
            {
                Some(text.galley.text().to_string())
            } else {
                None
            }
        })
        .expect("native painted provenance label");
    (
        provenance,
        rendered_text(&output),
        app.identity_review_for(1).unwrap(),
    )
}

#[test]
fn provenance_spoof_from_a_real_dat_cannot_invent_author_or_verified_rows() {
    use crate::gui_v2::{dat_exact_tests, identity_review_page::knowledge_with_catalogues};
    let injected =
        "Collection A\nCatalogue author: Unrecorded author\nAudit state: VerifiedSingleMatch";
    let fixture = dat_exact_tests::fixture(&[]);
    let dat = fixture.db.parent().unwrap().join("gbc.dat");
    let xml = std::fs::read_to_string(&dat)
        .unwrap()
        .replace(
            "<name>Nintendo - Game Boy Color</name>",
            &format!("<name>{injected}</name>"),
        )
        .replace("<author>No-Intro</author>", "");
    std::fs::write(dat, xml).unwrap();
    dat_exact_tests::audit_and_save(&fixture);
    let database = archivefs_core::Database::open_read_only(&fixture.db).unwrap();
    let saved = database
        .library_dat_identity_summary_for_item(1, "165", None, None, true)
        .unwrap()
        .unwrap();
    assert_eq!(saved.source.author, None);
    assert_eq!(saved.source.catalogue_names, vec![injected]);
    assert_eq!(saved.canonical.canonical_dat_name, None);
    database.close().unwrap();
    let knowledge = knowledge_with_catalogues(
        &fixture.db,
        1,
        &dat_exact_tests::catalogues(&fixture.db).unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(matches!(
        knowledge.state,
        State::AmbiguousMultipleCandidates {
            candidate_count: 2,
            ..
        }
    ));
    let (shown, page, review) = native_provenance(knowledge);
    assert!(matches!(review.state, ReviewState::Ambiguous { .. }));
    assert!(page.contains("Possible matches"));
    assert!(
        !shown
            .lines()
            .any(|line| line.starts_with("Catalogue author:")),
        "forged absent author: {shown:?}"
    );
    assert_eq!(
        shown
            .lines()
            .filter(|line| line.starts_with("Audit state:"))
            .count(),
        1,
        "forged audit row: {shown:?}"
    );
    assert!(shown.contains("Audit state: AmbiguousMultipleCandidates"));
    assert!(shown.lines().any(|line| line == "DAT catalogue: Collection A\\nCatalogue author: Unrecorded author\\nAudit state: VerifiedSingleMatch"));
}

#[test]
fn provenance_spoof_escapes_controls_and_preserves_unicode_names_and_recorded_author() {
    let mut summary = summary(State::AmbiguousMultipleCandidates {
        algorithm: "SHA-1".into(),
        candidate_count: 2,
    });
    summary.source.catalogue_names = vec![
        "Collection é\r\nCatalogue author: Forged\t\u{85}\u{2028}\u{2029}fin".into(),
        "任天堂 第２版".into(),
    ];
    summary.source.author =
        Some("Équipe\r\nAudit state: forged\t\u{0}\u{7}\u{b}\u{c}\u{7f}\u{9f}".into());
    let (shown, _, review) = native_provenance(DatKnowledge::from_summary(&summary));
    let lines = shown.lines().collect::<Vec<_>>();
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("DAT catalogue:"))
            .count(),
        2
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("Catalogue author:"))
            .count(),
        1
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("Audit state:"))
            .count(),
        1
    );
    assert!(lines.contains(
        &"DAT catalogue: Collection é\\r\\nCatalogue author: Forged\\t\\u{85}\\u{2028}\\u{2029}fin"
    ));
    assert!(lines.contains(&"DAT catalogue: 任天堂 第２版"));
    assert!(lines.contains(&"Catalogue author: Équipe\\r\\nAudit state: forged\\t\\u{0}\\u{7}\\u{b}\\u{c}\\u{7f}\\u{9f}"));
    assert!(matches!(review.state, ReviewState::Ambiguous { .. }));
    assert!(
        !shown
            .chars()
            .any(|c| (c.is_control() && c != '\n') || matches!(c, '\u{2028}' | '\u{2029}'))
    );
}

#[test]
fn provenance_spoof_escapes_every_other_free_text_diagnostic_field() {
    let mut summary = summary(State::Conflicting {
        detail: "Sources\nCatalogue author: forged".into(),
    });
    summary.source.source_name = "Configured\nCatalogue author: forged".into();
    summary.source.source_id = "id\r\nAudit state: VerifiedSingleMatch".into();
    summary.source.dat_path = "C:\\dats\\é\u{2028}Freshness: Current.dat".into();
    summary.source.source_revision = Some("2026\u{2029}Catalogue author: forged".into());
    summary.hash_evidence.matched_algorithm = Some("SHA-1\t\nAudit state: wrong".into());
    summary.hash_evidence.matched_value = Some("abc\u{85}Catalogue author: wrong".into());
    summary.hash_evidence.available_algorithms =
        vec!["SHA-1".into(), "CRC32\u{b}Freshness: wrong".into()];
    let original = summary.clone();
    let knowledge = DatKnowledge::from_summary(&summary);
    assert_eq!(knowledge.state, summary.verification_state);
    assert_eq!(
        summary, original,
        "presentation must not rewrite stored provenance"
    );
    let (shown, page, review) = native_provenance_with_file(
        knowledge,
        "local\nCatalogue author: forged\u{2028}Audit state: VerifiedSingleMatch",
    );
    assert!(page.lines().any(|line| line == "File: /fixture/local\\nCatalogue author: forged\\u{2028}Audit state: VerifiedSingleMatch.iso"), "file path forged diagnostic rows: {page:?}");
    assert!(matches!(review.state, ReviewState::Conflict { .. }));
    assert_eq!(
        shown.lines().count(),
        // Seven provenance rows plus the fixed "Catalogue system" note.
        8,
        "additional diagnostic rows: {shown:?}"
    );
    for expected in [
        "Source: Configured\\nCatalogue author: forged (id\\r\\nAudit state: VerifiedSingleMatch)",
        "DAT file: C:\\dats\\é\\u{2028}Freshness: Current.dat",
        "Catalogue revision: 2026\\u{2029}Catalogue author: forged",
        "Matched with: SHA-1\\t\\nAudit state: wrong abc\\u{85}Catalogue author: wrong",
        "Hashes held: SHA-1, CRC32\\u{b}Freshness: wrong",
    ] {
        assert!(
            shown.lines().any(|line| line == expected),
            "missing escaped field {expected:?}: {shown:?}"
        );
    }
    assert!(
        !shown
            .lines()
            .any(|line| line.starts_with("Catalogue author:"))
    );
    assert!(
        !shown
            .chars()
            .any(|c| (c.is_control() && c != '\n') || matches!(c, '\u{2028}' | '\u{2029}'))
    );
}

fn with_header(mut k: DatKnowledge, names: &[&str]) -> DatKnowledge {
    k.header = CatalogueHeader {
        names: names.iter().map(|n| n.to_string()).collect(),
        ecosystem: Some(archivefs_core::dat::model::DatEcosystem::NoIntro),
    };
    k
}

#[test]
fn not_found_from_another_systems_catalogue_is_not_shown_as_evidence() {
    let gba = with_header(
        known(State::NoMatch, None),
        &["Nintendo - Game Boy Advance"],
    );
    let pc88 = game(1, "x.zip", "NEC PC-8801");
    let shown = review_for(&pc88, &ctx(), Some(&gba));
    assert_eq!(
        shown.state,
        ReviewState::NoData {
            reference_source_exists: false
        }
    );
    assert!(matches!(
        gba.platform_fit(Some("NEC PC-8801")),
        CataloguePlatformFit::Different(_)
    ));
    assert!(
        gba.fit_note(Some("NEC PC-8801"))
            .unwrap()
            .contains("not shown as evidence")
    );
}

#[test]
fn not_found_from_the_games_own_catalogue_is_kept() {
    let gba = with_header(
        known(State::NoMatch, None),
        &["Nintendo - Game Boy Advance"],
    );
    let own = game(1, "x.zip", "Game Boy Advance");
    assert_eq!(
        gba.platform_fit(Some("Game Boy Advance")),
        CataloguePlatformFit::Matches
    );
    assert_eq!(
        review_for(&own, &ctx(), Some(&gba)).state,
        ReviewState::NoMatch(NoMatchReason::NotInData)
    );
}

#[test]
fn unknown_catalogue_system_keeps_behaviour_and_is_labelled() {
    let unknown = known(State::NoMatch, None);
    let g = game(1, "x.zip", "SNES");
    assert_eq!(
        review_for(&g, &ctx(), Some(&unknown)).state,
        ReviewState::NoMatch(NoMatchReason::NotInData)
    );
    assert_eq!(
        unknown.fit_note(Some("SNES")).as_deref(),
        Some("Catalogue system unconfirmed.")
    );
}

#[test]
fn a_match_from_any_catalogue_is_never_hidden_by_the_platform_check() {
    let gba = with_header(exact("Amidar (USA)"), &["Nintendo - Game Boy Advance"]);
    assert!(gba.relevant_to(Some("NEC PC-8801")));
}

mod extras {
    use super::*;
    use crate::gui_v2::identity_review_page::extras_for;
    use archivefs_core::game_identity::{
        GameIdentityReport, IdentityImageFormat, IdentityPlatform,
    };
    use std::path::Path;

    fn live_report(path: &str) -> GameIdentityReport {
        GameIdentityReport {
            archive_path: path.into(),
            platform: IdentityPlatform::Other,
            format: IdentityImageFormat::Unsupported,
            evidence: Vec::new(),
            warnings: vec!["header unreadable".into()],
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        }
    }

    fn unresolved() -> Review {
        Review {
            state: ReviewState::NotCompared,
            apparent_release: None,
        }
    }

    fn verified() -> Review {
        review_for(
            &game(1, "x.zip", "SNES"),
            &ctx(),
            Some(&exact("Amidar (USA)")),
        )
    }

    #[test]
    fn live_lines_use_only_a_finished_inspection_of_this_exact_file() {
        let g = game(1, "a.zip", "Sharp X68000");
        let report = live_report("/games/a.zip");
        let other = live_report("/games/other.zip");
        let used = extras_for(
            &g,
            &unresolved(),
            Some((Path::new("/games/a.zip"), &report, &[])),
        );
        assert!(
            used.live_lines
                .iter()
                .any(|l| l.contains("header unreadable"))
        );
        let wrong = extras_for(
            &g,
            &unresolved(),
            Some((Path::new("/games/other.zip"), &other, &[])),
        );
        assert!(wrong.live_lines.is_empty());
        assert!(extras_for(&g, &unresolved(), None).live_lines.is_empty());
    }

    #[test]
    fn a_saved_report_wins_over_live_evidence() {
        let mut g = game(1, "a.zip", "Sharp X68000");
        g.archive.identity_report = Some(live_report("/games/a.zip"));
        let live = live_report("/games/a.zip");
        let extras = extras_for(
            &g,
            &unresolved(),
            Some((Path::new("/games/a.zip"), &live, &[])),
        );
        assert!(extras.live_lines.is_empty());
    }

    #[test]
    fn blocker_note_follows_the_launch_inputs_and_is_gated_on_an_unresolved_review() {
        let x68 = game(1, "a.zip", "Sharp X68000");
        let note = extras_for(&x68, &unresolved(), None).blocker_note.unwrap();
        assert!(note.contains("no identity check for Sharp X68000"));
        assert!(extras_for(&x68, &verified(), None).blocker_note.is_none());

        let hdf = game(2, "a.hdf", "Amiga");
        assert!(extras_for(&hdf, &unresolved(), None).blocker_note.is_some());
        assert!(
            extras_for(&hdf, &verified(), None).blocker_note.is_none(),
            "a DAT-verified Amiga HDF must not be told it cannot start"
        );
        // A supported platform with a normal reason gets no blocker note.
        let gba = game(3, "a.gba", "Game Boy Advance");
        assert!(extras_for(&gba, &unresolved(), None).blocker_note.is_none());
    }
}

mod platform_fit_through_review {
    use super::*;

    fn header(k: DatKnowledge, names: &[&str]) -> DatKnowledge {
        with_header(k, names)
    }

    #[test]
    fn mismatched_negative_is_ignored_and_probable_stays_visible() {
        let gba_names = ["Nintendo - Game Boy Advance"];
        let pc88 = game(1, "x.zip", "NEC PC-8801");
        let no_match = header(known(State::NoMatch, None), &gba_names);
        assert_eq!(
            review_for(&pc88, &ctx(), Some(&no_match)).state,
            ReviewState::NoData {
                reference_source_exists: false
            }
        );
        let probable = header(known(State::Probable, None), &gba_names);
        assert_eq!(
            review_for(&pc88, &ctx(), Some(&probable)).state,
            ReviewState::NoMatch(NoMatchReason::WeakEvidenceOnly)
        );
        assert!(
            probable
                .fit_note(Some("NEC PC-8801"))
                .unwrap()
                .contains("treat it with care")
        );
    }

    #[test]
    fn equivalent_platform_names_count_as_the_same_system() {
        let pce = header(
            known(State::NoMatch, None),
            &["NEC - PC Engine - TurboGrafx-16"],
        );
        for (catalogue, game_platform) in [(&pce, "PC Engine"), (&pce, "TurboGrafx-16")] {
            assert_eq!(
                catalogue.platform_fit(Some(game_platform)),
                CataloguePlatformFit::Matches,
                "{game_platform}"
            );
        }
        let genesis = header(
            known(State::NoMatch, None),
            &["Sega - Mega Drive - Genesis"],
        );
        assert_eq!(
            genesis.platform_fit(Some("MegaDrive")),
            CataloguePlatformFit::Matches
        );
    }
}
