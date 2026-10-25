use super::*;
use archivefs_core::game_identity::{
    IdentityEvidence, IdentityImageFormat, IdentityKind, IdentityPlatform, IdentityProvenance,
    IdentityStatus,
};
use std::path::PathBuf;

fn report() -> GameIdentityReport {
    GameIdentityReport {
        archive_path: PathBuf::from("fixture.iso"),
        platform: IdentityPlatform::GameCube,
        format: IdentityImageFormat::Iso,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::DolphinGameId,
            status: IdentityStatus::Verified,
            value: Some("GZLE01".into()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: PathBuf::from("fixture.iso"),
                member_path: Some(b"member.iso".to_vec()),
                member_index: Some(0),
                method: "bounded disc header".into(),
            },
            diagnostic: "ID read from the disc header; payload not checked".into(),
        }],
        warnings: vec![],
        bytes_read: 32,
        archive_members_inspected: 1,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    }
}

fn painted(output: &egui::FullOutput) -> String {
    fn visit(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(label) => {
                text.push_str(label.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for shape in &output.shapes {
        visit(&shape.shape, &mut text);
    }
    text
}

fn render(path: &Path, report: Option<&GameIdentityReport>) -> String {
    let context = egui::Context::default();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default().show(context, |ui| super::show(ui, path, report));
        },
    );
    painted(&output)
}

#[test]
fn saved_header_identity_explains_its_value_source_and_limits_without_changing_evidence() {
    let report = report();
    let before = serde_json::to_vec(&report).unwrap();
    let text = render(&report.archive_path, Some(&report));
    for expected in [
        "GZLE01",
        "Recorded status: Verified",
        "structured metadata",
        "bounded disc header",
        "member.iso",
        "payload not checked",
        "do not establish current DAT verification",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert_eq!(before, serde_json::to_vec(&report).unwrap());
}

#[test]
fn clues_conflicts_and_unavailable_facts_keep_their_recorded_status_and_reason() {
    for status in [
        IdentityStatus::Candidate,
        IdentityStatus::Missing,
        IdentityStatus::Unsupported,
        IdentityStatus::Deferred,
        IdentityStatus::Invalid,
        IdentityStatus::Ambiguous,
        IdentityStatus::ResourceLimitReached,
    ] {
        let mut report = report();
        report.evidence[0].status = status;
        report.evidence[0].confidence = IdentityConfidence::FilenameOnly;
        report.evidence[0].diagnostic = "Conflicting candidate: another title".into();
        let text = render(&report.archive_path, Some(&report));
        assert!(text.contains(&format!("Recorded status: {status}")));
        assert!(text.contains("filename clue only — cannot verify identity"));
        assert!(text.contains("Conflicting candidate: another title"));
        assert!(!text.contains("Recorded status: Verified"));
    }
    for confidence in [
        IdentityConfidence::ExactBytes,
        IdentityConfidence::CatalogueContext,
        IdentityConfidence::Unavailable,
    ] {
        let mut report = report();
        report.evidence[0].status = IdentityStatus::Candidate;
        report.evidence[0].confidence = confidence;
        let text = render(&report.archive_path, Some(&report));
        assert!(text.contains(super::confidence(confidence)));
        assert!(!text.contains("Recorded status: Verified"));
    }
}

#[test]
fn absent_or_wrong_selection_evidence_is_not_shown_for_the_selected_file() {
    let report = report();
    for saved in [None, Some(&report)] {
        let text = render(Path::new("different.iso"), saved);
        assert!(text.contains("No saved identity evidence is available"));
        assert!(!text.contains("GZLE01"));
    }
}

#[test]
fn incomplete_empty_reports_and_warnings_are_explicit_and_controls_cannot_spoof_rows() {
    let mut report = report();
    report.complete = false;
    report.evidence[0].value = Some("name\nVerified".into());
    report.evidence[0].provenance.method = "header\rsource".into();
    report.warnings.push("unsupported\nfield".into());
    let text = render(&report.archive_path, Some(&report));
    assert!(text.contains("inspection was incomplete"));
    assert!(text.contains("name\\nVerified"));
    assert!(text.contains("header\\rsource"));
    assert!(text.contains("Inspection warning: unsupported\\nfield"));
    report.evidence.clear();
    assert!(render(&report.archive_path, Some(&report)).contains("recorded no identity facts"));
}

#[test]
fn paging_is_bounded_and_keyboard_can_reach_the_remaining_facts() {
    let context = egui::Context::default();
    let mut saved = report();
    saved.evidence = (0..17)
        .map(|index| {
            let mut fact = saved.evidence[0].clone();
            fact.value = Some(format!("fact-{index:02}"));
            fact
        })
        .collect();
    let frame = |events| {
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
                    super::show(ui, &saved.archive_path, Some(&saved))
                });
            },
        )
    };
    frame(vec![]);
    let first = painted(&frame(vec![]));
    assert!(first.contains("fact-00"));
    assert!(!first.contains("fact-08"));
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    // Previous is disabled on the first page, making Next the first focusable control.
    frame(vec![key(egui::Key::Tab)]);
    frame(vec![key(egui::Key::Enter)]);
    let second = painted(&frame(vec![]));
    assert!(second.contains("fact-08"), "{second}");
    assert!(!second.contains("fact-00"));
    // The saved evidence itself is unchanged by paging.
    assert_eq!(saved.evidence.len(), 17);
}

#[test]
fn blue_review_exposes_the_saved_fact_through_its_keyboard_disclosure() {
    use crate::gui_v2::tests::review_app;
    use std::sync::Arc;
    let context = egui::Context::default();
    crate::gui_v2::readable_style(&context);
    context.style_mut(|style| style.animation_time = 0.0);
    let mut app = review_app(&context, vec![(1, "fixture.iso", "GameCube")]);
    let game = &mut Arc::make_mut(&mut app.library).games[0];
    let mut saved = report();
    saved.archive_path = game.archive.absolute_path.clone();
    game.archive.identity_report = Some(saved);
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
                egui::CentralPanel::default().show(context, |ui| app.review_identity(ui, 1));
            },
        )
    };
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let closed = painted(&frame(vec![]));
    assert!(closed.contains("Why this identity?"), "{closed}");
    assert!(!closed.contains("bounded disc header"));
    // Traverse the real page's controls; Enter activates the disclosure when focused.
    let mut opened = false;
    for _ in 0..12 {
        frame(vec![key(egui::Key::Tab)]);
        frame(vec![key(egui::Key::Enter)]);
        let text = painted(&frame(vec![]));
        if text.contains("bounded disc header") {
            assert!(text.contains("GZLE01"));
            assert!(text.contains("structured metadata"));
            opened = true;
            break;
        }
    }
    assert!(opened, "keyboard must reach the saved identity evidence");
}

#[test]
fn saved_member_index_and_inconsistent_outer_attribution_are_explicit() {
    let mut saved = report();
    saved.evidence[0].provenance.archive_path = "another.zip".into();
    let text = render(&saved.archive_path, Some(&saved));
    assert!(text.contains("Archive member index (zero-based): 0"));
    assert!(text.contains("another.zip"));
    assert!(text.contains("attribution is inconsistent"));
    saved.evidence[0].value = None;
    let text = render(&saved.archive_path, Some(&saved));
    assert!(text.contains("not recorded"));
}

#[test]
fn long_saved_fields_are_bounded_and_truncation_is_visible() {
    let mut saved = report();
    saved.evidence[0].value = Some("界".repeat(40_000));
    saved.evidence[0].provenance.method = "界".repeat(40_000);
    saved.evidence[0].diagnostic = "界".repeat(40_000);
    saved.warnings = vec!["界".repeat(40_000)];
    let before = serde_json::to_vec(&saved).unwrap();
    let text = render(&saved.archive_path, Some(&saved));
    assert!(text.contains("truncated for display"));
    assert!(
        text.len() < 30_000,
        "unbounded saved text reached rendering"
    );
    assert_eq!(before, serde_json::to_vec(&saved).unwrap());
}

#[test]
fn malformed_saved_json_from_the_native_loader_is_shown_as_unavailable_without_writes() {
    let fixture = crate::gui_v2::dat_exact_tests::fixture(&["Exact".into()]);
    for bytes in [b"{".as_slice(), b"{}", b"{\"evidence\":[null]}"] {
        {
            let connection = rusqlite::Connection::open(&fixture.db).unwrap();
            connection.execute(
                "UPDATE archives SET identity_report_json=?1, identity_report_size_bytes=size_bytes, identity_report_modified_time_unix_seconds=modified_time_unix_seconds WHERE id=1",
                [bytes],
            ).unwrap();
        }
        let before = std::fs::read(&fixture.db).unwrap();
        let database = archivefs_core::Database::open_read_only(&fixture.db).unwrap();
        let archives = database.load_archives().unwrap();
        let archive = archives.iter().find(|a| a.id == 1).unwrap();
        assert!(archive.identity_report.is_none());
        let text = render(&archive.absolute_path, archive.identity_report.as_ref());
        assert!(text.contains("No saved identity evidence is available"));
        assert!(!text.contains("Recorded status: Verified"));
        assert_eq!(before, std::fs::read(&fixture.db).unwrap());
    }
}

fn knowledge(
    state: archivefs_core::dat::library_identity_summary::DatVerificationState,
    stale: bool,
) -> crate::gui_v2::identity_review::DatKnowledge {
    crate::gui_v2::identity_review::DatKnowledge {
        state,
        trusted_source: true,
        stale,
        title: Some("Trusted title".into()),
        region: None,
        revision: None,
        source_name: "Existing catalogue source".into(),
        ecosystem: Some("No-Intro"),
        candidates: vec!["Candidate A".into(), "Candidate B".into()],
        technical: "Existing catalogue provenance, hash, revision and freshness".into(),
    }
}

#[test]
fn native_review_keeps_verified_conflicting_stale_and_unavailable_decisions_unchanged() {
    use crate::gui_v2::{identity_review::review_for, tests::review_app};
    use archivefs_core::dat::library_identity_summary::DatVerificationState as S;
    use std::sync::Arc;
    for (state, stale) in [
        (
            S::VerifiedSingleMatch {
                algorithm: "SHA-1".into(),
            },
            false,
        ),
        (
            S::VerifiedSingleMatch {
                algorithm: "SHA-1".into(),
            },
            true,
        ),
        (S::FilenameOnlyNotVerified, false),
        (S::Probable, false),
        (
            S::Conflicting {
                detail: "Conflicting candidates".into(),
            },
            false,
        ),
        (
            S::AmbiguousMultipleCandidates {
                algorithm: "SHA-1".into(),
                candidate_count: 2,
            },
            false,
        ),
        (S::NoUsableEvidence, false),
    ] {
        let context = egui::Context::default();
        let mut app = review_app(&context, vec![(1, "fixture", "GameCube")]);
        let game = &mut Arc::make_mut(&mut app.library).games[0];
        let mut saved = report();
        saved.archive_path = game.archive.absolute_path.clone();
        saved.evidence[0].provenance.archive_path = saved.archive_path.clone();
        saved.evidence[0].status = IdentityStatus::Candidate;
        saved.evidence[0].confidence = IdentityConfidence::FilenameOnly;
        game.archive.identity_report = Some(saved);
        app.review.knowledge = Some(knowledge(state, stale));
        let library_before = app.library.clone();
        let known_before = app.review.knowledge.clone();
        let before = review_for(
            &app.library.games[0],
            &app.library.identity_context,
            app.review.knowledge.as_ref(),
        );
        let report_before =
            serde_json::to_vec(&app.library.games[0].archive.identity_report).unwrap();
        let jobs_before = app.activity.jobs.len();
        for _ in 0..2 {
            let _ = context.run(egui::RawInput::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    app.review_identity(ui, 1);
                    // Also project the expanded body for this exact saved record.
                    let game = &app.library.games[0];
                    super::show(
                        ui,
                        &game.archive.absolute_path,
                        game.archive.identity_report.as_ref(),
                    );
                });
            });
        }
        assert!(Arc::ptr_eq(&library_before, &app.library));
        assert_eq!(known_before, app.review.knowledge);
        assert_eq!(
            before,
            review_for(
                &app.library.games[0],
                &app.library.identity_context,
                app.review.knowledge.as_ref()
            )
        );
        assert_eq!(
            report_before,
            serde_json::to_vec(&app.library.games[0].archive.identity_report).unwrap()
        );
        assert_eq!(
            jobs_before,
            app.activity.jobs.len(),
            "rendering must not start an audit or inspection"
        );
        assert!(app.review.assign_job.is_none());
        assert!(app.review.knowledge_job.is_none());
    }
}

#[test]
fn saved_unicode_fields_wrap_at_narrow_width_and_large_scale() {
    let mut saved = report();
    saved.evidence[0].value =
        Some("任天堂 Édition française avec une longue identité enregistrée".repeat(3));
    saved.evidence[0].diagnostic =
        "Diagnostic précis: texte enregistré, pas une nouvelle vérification".repeat(3);
    for zoom in [1.0, 1.6] {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        context.set_zoom_factor(zoom);
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(420.0, 2200.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    super::show(ui, &saved.archive_path, Some(&saved))
                });
            },
        );
        let mut fields = 0;
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape
                && (text.galley.text().contains("任天堂")
                    || text.galley.text().contains("Diagnostic précis"))
            {
                fields += 1;
                assert!(text.pos.x >= clipped.clip_rect.min.x - 1.0);
                assert!(
                    text.pos.x + text.galley.size().x <= clipped.clip_rect.max.x + 1.0,
                    "saved evidence clipped at zoom {zoom}"
                );
            }
        }
        assert_eq!(fields, 2);
    }
}
