//! Disposable game files and shared inventory projections; no live config or database.
use super::*;
use archivefs_core::PersistedArchive;
use archivefs_core::dat::catalogue_selection::{CatalogueInventoryInputs, EvidenceValue};
use archivefs_core::dat::managed_sources::ManagedDatSources;
use archivefs_core::dat::sources::{
    DatHealthState, DatSourceEntry, DatSourceKind, DatSourceRegistry,
};
use eframe::egui;

struct Fixture {
    _dir: tempfile::TempDir,
    game: Game,
    catalogues: Vec<InstalledCatalogueSummary>,
}

fn fixture(platform: &str, extension: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("game.{extension}"));
    std::fs::write(&path, b"disposable game content").unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let game = Game::from_archive(PersistedArchive {
        id: 1,
        source_folder_id: 1,
        relative_path: path.file_name().unwrap().into(),
        absolute_path: path,
        archive_kind: "direct_game_image".into(),
        display_name: "Game".into(),
        normalized_name: "game".into(),
        size_bytes: Some(metadata.len()),
        modified_time_unix_seconds: Some(
            metadata
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        ),
        platform: Some(platform.into()),
        platform_source: Some("fixture".into()),
        last_known_health: "pending".into(),
        last_seen_at: "fixture".into(),
        last_verified_missing_at: None,
        identity_report: None,
    });
    let dat = dir.path().join("catalogue.dat");
    std::fs::write(
        &dat,
        "<datafile><header><name>Fixture</name></header></datafile>",
    )
    .unwrap();
    // A recorded validation is fixture evidence, never derived from the count
    // of installed catalogues. The guidance layer itself must not parse DATs.
    let mut entry = DatSourceEntry::new(
        "fixture".into(),
        "Fixture DAT".into(),
        dat,
        DatSourceKind::File,
    );
    entry.platform = Some(platform.into());
    entry.health.state = Some(DatHealthState::Valid);
    let mut registry = DatSourceRegistry::new();
    registry.add(entry).unwrap();
    let catalogues = list_installed_catalogues(CatalogueInventoryInputs {
        local_registry: &registry,
        managed_sources: &ManagedDatSources::new(),
        managed_root: dir.path(),
        limits: Default::default(),
    });
    assert_eq!(catalogues[0].availability, CatalogueAvailability::Ready);
    Fixture {
        _dir: dir,
        game,
        catalogues,
    }
}

const SYSTEMS: [(&str, &str); 4] = [
    ("BBC Micro", "ssd"),
    ("Amiga", "adf"),
    ("ZX Spectrum", "tzx"),
    ("PS1", "cue"),
];

#[test]
fn ready_dat_installation_never_verifies_any_of_the_four_systems() {
    for (platform, extension) in SYSTEMS {
        let f = fixture(platform, extension);
        let result = explain(
            &f.game,
            true,
            true,
            0,
            DatProvenanceFreshness::Unknown,
            Ok(&f.catalogues),
            None,
        );
        assert_eq!(result.reason, "Audit not yet performed", "{platform}");
        assert_eq!(result.action, Action::Audit);
        assert!(result.explanation.contains("do not check game content"));
        assert!(result.explanation.contains("preview eligible files"));
        assert_eq!(
            result.action.route(&f.game),
            super::super::routes::Route::PlatformCheck(f.game.platform.clone())
        );
    }
}

#[test]
fn every_system_has_truthful_guidance_for_assignment_validation_and_audit_results() {
    for (platform, extension) in SYSTEMS {
        let mut f = fixture(platform, extension);
        let missing = explain(
            &f.game,
            true,
            true,
            0,
            DatProvenanceFreshness::Unknown,
            Ok(&[]),
            None,
        );
        assert_eq!(
            (missing.reason, missing.action),
            ("No compatible DAT assigned", Action::SelectDat)
        );
        let saved = f.catalogues[0].availability.clone();
        for availability in [
            CatalogueAvailability::NeedsValidation {
                reason: "Not validated yet".into(),
            },
            CatalogueAvailability::Missing {
                reason: "Catalogue file is missing".into(),
            },
            CatalogueAvailability::Corrupt {
                reason: "Invalid DAT".into(),
            },
            CatalogueAvailability::StaleManagedState {
                reason: "Snapshot unavailable".into(),
            },
        ] {
            f.catalogues[0].availability = availability;
            let result = explain(
                &f.game,
                true,
                true,
                0,
                DatProvenanceFreshness::Unknown,
                Ok(&f.catalogues),
                None,
            );
            assert_eq!(result.reason, "Catalogue not validated or unavailable");
            assert_eq!(result.action, Action::Validate);
        }
        f.catalogues[0].availability = saved;
        f.game.dat_state = Some(LibraryDatState::NoMatch);
        let result = explain(
            &f.game,
            true,
            true,
            1,
            DatProvenanceFreshness::Unknown,
            Ok(&f.catalogues),
            None,
        );
        assert_eq!(
            (result.reason, result.action),
            ("No catalogue match", Action::Recheck)
        );
        assert!(
            result
                .explanation
                .contains("does not prove the file is corrupt")
        );
        for weak in [
            LibraryDatState::Probable,
            LibraryDatState::Ambiguous,
            LibraryDatState::Verified,
        ] {
            f.game.dat_state = Some(weak);
            assert_eq!(
                explain(
                    &f.game,
                    true,
                    true,
                    1,
                    DatProvenanceFreshness::Unknown,
                    Ok(&f.catalogues),
                    None
                )
                .reason,
                "Audit needs review"
            );
        }
        f.game.dat_exact = Some(Some("TOSEC"));
        let result = explain(
            &f.game,
            true,
            true,
            1,
            DatProvenanceFreshness::Current,
            Ok(&f.catalogues),
            None,
        );
        assert_eq!((result.reason, result.action), ("Verified", Action::Review));
        assert_eq!(
            result.action.route(&f.game),
            super::super::routes::Route::ReviewIdentity(1)
        );
    }
}

#[test]
fn changed_missing_or_stale_evidence_overrides_a_saved_exact_flag() {
    for (platform, extension) in SYSTEMS {
        let mut f = fixture(platform, extension);
        f.game.dat_exact = Some(None);
        for (present, unchanged, stale) in [
            (false, false, false),
            (true, false, false),
            (true, true, true),
        ] {
            let result = explain(
                &f.game,
                present,
                unchanged,
                1,
                if stale {
                    DatProvenanceFreshness::Stale
                } else {
                    DatProvenanceFreshness::Unknown
                },
                Ok(&f.catalogues),
                None,
            );
            assert_eq!(
                (result.reason, result.action),
                ("File evidence stale or changed", Action::Recheck)
            );
        }
    }
}

#[test]
fn explicit_preview_exclusion_is_preserved_without_guessing_from_media_extension() {
    for (platform, extension) in SYSTEMS {
        let f = fixture(platform, extension);
        let result = explain(
            &f.game,
            true,
            true,
            0,
            DatProvenanceFreshness::Unknown,
            Ok(&f.catalogues),
            Some("not represented by the selected catalogue"),
        );
        assert_eq!(
            result.reason,
            "File format unsupported for exact verification"
        );
        assert!(
            result
                .explanation
                .contains("not represented by the selected catalogue")
        );
        assert_eq!(result.action, Action::SelectDat);
        // No exclusion from the owning preview means eligibility is unknown,
        // not an invented format refusal based on SSD/ADF/TZX/CUE suffixes.
        assert_eq!(
            explain(
                &f.game,
                true,
                true,
                0,
                DatProvenanceFreshness::Unknown,
                Ok(&f.catalogues),
                None
            )
            .reason,
            "Audit not yet performed"
        );
    }
}

#[test]
fn hundreds_of_unrelated_or_unresolved_catalogues_do_not_assign_a_dat() {
    let mut f = fixture("BBC Micro", "ssd");
    f.catalogues[0].platform = EvidenceValue::Assigned("Amiga".into());
    let mut rows = vec![f.catalogues[0].clone(); 400];
    rows[1].platform = EvidenceValue::Unknown;
    rows[2].platform = EvidenceValue::Ambiguous(vec!["BBC Micro".into(), "Amiga".into()]);
    let mut disabled = f.catalogues[0].clone();
    disabled.platform = EvidenceValue::Confirmed("BBC Micro".into());
    disabled.enabled = false;
    rows.push(disabled);
    let result = explain(
        &f.game,
        true,
        true,
        0,
        DatProvenanceFreshness::Unknown,
        Ok(&rows),
        None,
    );
    assert_eq!(result.reason, "No compatible DAT assigned");
    assert_eq!(result.action, Action::SelectDat);
}

#[test]
fn an_unusable_saved_check_or_inventory_error_is_not_not_checked_or_no_match() {
    let f = fixture("Amiga", "adf");
    assert_eq!(
        explain(
            &f.game,
            true,
            true,
            1,
            DatProvenanceFreshness::Unknown,
            Ok(&f.catalogues),
            None
        )
        .reason,
        "Audit has no usable exact result"
    );
    assert_eq!(
        explain(
            &f.game,
            true,
            true,
            0,
            DatProvenanceFreshness::Unknown,
            Err("unreadable inventory"),
            None
        )
        .reason,
        "Catalogue status unavailable"
    );
}

#[test]
fn guidance_rendering_names_the_reason_and_navigation_action() {
    let context = egui::Context::default();
    let f = fixture("PS1", "cue");
    let guidance = explain(
        &f.game,
        true,
        true,
        0,
        DatProvenanceFreshness::Unknown,
        Ok(&f.catalogues),
        None,
    );
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 500.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show(ui, &guidance);
            });
        },
    );
    fn gather(shape: &egui::Shape, texts: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => texts.push(t.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    gather(shape, texts);
                }
            }
            _ => (),
        }
    }
    let mut texts = Vec::new();
    for shape in output.shapes {
        gather(&shape.shape, &mut texts);
    }
    assert!(
        texts.iter().any(|t| t == "DAT: Audit not yet performed"),
        "{texts:?}"
    );
    assert!(texts.iter().any(|t| t == "Audit"));
    assert!(!texts.iter().any(|t| t == "Verified"));
}

#[test]
fn navigation_discards_pending_and_loaded_game_guidance() {
    let mut state = State {
        game: Some(1),
        result: Some(unavailable()),
        receiver: None,
    };
    state.invalidate();
    assert!(state.game.is_none() && state.result.is_none() && state.receiver.is_none());
}

#[test]
fn clicking_the_guidance_action_only_requests_the_existing_platform_route() {
    let context = egui::Context::default();
    let f = fixture("BBC Micro", "ssd");
    let guidance = explain(
        &f.game,
        true,
        true,
        0,
        DatProvenanceFreshness::Unknown,
        Ok(&f.catalogues),
        None,
    );
    let mut button = None;
    let mut clicked = false;
    let mut frame = |events| {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 500.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // The same show function the Game Details dispatch uses.
                    clicked = show(ui, &guidance);
                });
            },
        )
    };
    let output = frame(vec![]);
    fn find(shape: &egui::Shape) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == "Audit" => {
                Some(t.pos + t.galley.size() / 2.0)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(find),
            _ => None,
        }
    }
    for shape in output.shapes {
        button = button.or_else(|| find(&shape.shape));
    }
    let position = button.expect("Audit button must be rendered");
    let _ = frame(vec![
        egui::Event::PointerMoved(position),
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        },
    ]);
    let _ = frame(vec![egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    }]);
    assert!(clicked);
    assert_eq!(
        guidance.action.route(&f.game),
        super::super::routes::Route::PlatformCheck("BBC Micro".into())
    );
}

#[test]
fn unknown_freshness_and_a_later_scan_never_renew_a_saved_exact_audit() {
    let mut f = fixture("PS1", "cue");
    f.game.dat_exact = Some(None);
    // Even an unchanged/latest scan comparison does not prove that the file
    // still matches the audit snapshot. Current must come from its owner.
    let result = explain(
        &f.game,
        true,
        true,
        1,
        DatProvenanceFreshness::Unknown,
        Ok(&f.catalogues),
        None,
    );
    assert_eq!(result.reason, "Saved exact match; freshness needs checking");
    assert_eq!(result.action, Action::Recheck);
}

#[test]
fn aggregate_folders_and_non_game_catalogues_need_selection_not_repeat_validation() {
    let mut f = fixture("BBC Micro", "ssd");
    f.catalogues[0].availability = CatalogueAvailability::AggregateFolder {
        note: "Several catalogues".into(),
    };
    let result = explain(
        &f.game,
        true,
        true,
        0,
        DatProvenanceFreshness::Unknown,
        Ok(&f.catalogues),
        None,
    );
    assert_eq!(result.action, Action::SelectDat);
    f.catalogues[0].availability = CatalogueAvailability::Ready;
    f.catalogues[0].capabilities.verify = false;
    assert_eq!(
        explain(
            &f.game,
            true,
            true,
            0,
            DatProvenanceFreshness::Unknown,
            Ok(&f.catalogues),
            None
        )
        .action,
        Action::SelectDat
    );
}

#[test]
fn a_stale_saved_exact_audit_is_not_cleared_by_an_unrelated_unknown_audit() {
    use super::super::dat_exact_tests;
    let fixture = dat_exact_tests::fixture(&["Exact".into()]);
    dat_exact_tests::audit_and_save(&fixture);
    let library = super::super::backend::load_library(&fixture.db).unwrap();
    let game = library
        .games
        .iter()
        .find(|g| g.dat_exact.is_some())
        .unwrap();
    let mut database = Database::open_or_create(&fixture.db).unwrap();
    let mut original = database
        .library_dat_identities_for_item(game.archive.id)
        .unwrap()
        .remove(0);
    original.source.source_revision = Some("old".into());
    database
        .persist_library_dat_identity(game.archive.id, &original)
        .unwrap();
    let mut unrelated = original.clone();
    unrelated.source.source_id = "unrelated".into();
    database
        .persist_library_dat_identity(game.archive.id, &unrelated)
        .unwrap();
    database
        .mark_library_dat_identity_stale_for_source_revision(
            &original.source.source_id,
            Some("changed"),
        )
        .unwrap();
    let (count, freshness) = saved_evidence(game, &database).unwrap();
    assert_eq!(count, 2);
    assert_eq!(freshness, DatProvenanceFreshness::Stale);
}

#[test]
fn changed_cue_companions_are_stale_even_when_the_sheet_is_unchanged() {
    use super::super::dat_exact_tests;
    use archivefs_core::identity_source::hashing::FileFingerprint;
    let fixture = dat_exact_tests::fixture(&["Exact".into()]);
    dat_exact_tests::audit_and_save(&fixture);
    let library = super::super::backend::load_library(&fixture.db).unwrap();
    let game = library
        .games
        .iter()
        .find(|g| g.dat_exact.is_some())
        .unwrap();
    let mut database = Database::open_or_create(&fixture.db).unwrap();
    let mut saved = database
        .library_dat_identities_for_item(game.archive.id)
        .unwrap()
        .remove(0);
    let companion = fixture.db.parent().unwrap().join("track.bin");
    std::fs::write(&companion, b"original track").unwrap();
    saved.disc_members = vec![
        FileFingerprint::observe(&game.archive.absolute_path).unwrap(),
        FileFingerprint::observe(&companion).unwrap(),
    ];
    database
        .persist_library_dat_identity(game.archive.id, &saved)
        .unwrap();
    assert_eq!(
        saved_evidence(game, &database).unwrap().1,
        DatProvenanceFreshness::Unknown
    );
    std::fs::write(&companion, b"changed track with another length").unwrap();
    assert_eq!(
        saved_evidence(game, &database).unwrap().1,
        DatProvenanceFreshness::Stale
    );
}
