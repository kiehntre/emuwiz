use super::*;
use crate::gui_v2::{dat_exact_tests, identity_review::review_for};

#[test]
fn review_identity_does_not_revive_an_exact_match_from_an_unavailable_catalogue() {
    let fixture = dat_exact_tests::fixture(&["Synthetic Game (USA)".into()]);
    dat_exact_tests::audit_and_save(&fixture);
    let library = dat_exact_tests::load_library(&fixture.db).unwrap();
    assert!(library.game(1).unwrap().dat_exact.is_some());
    let mut game = library.game(1).unwrap().clone();
    game.dat_exact = None;
    let before = std::fs::read(&fixture.db).unwrap();
    let knowledge = knowledge_with_catalogues(&fixture.db, 1, &Default::default()).unwrap();
    assert!(
        !review_for(&game, &Default::default(), knowledge.as_ref()).is_verified(),
        "opening View evidence must not revive an exact match after its catalogue is unavailable"
    );
    assert_eq!(std::fs::read(&fixture.db).unwrap(), before);
}
use archivefs_core::dat::{
    catalogue_selection::CatalogueInventoryInputs,
    configured_catalogues::{ConfiguredCatalogue, configured_catalogues},
    managed_sources::ManagedDatSources,
    sources::{
        DatSourceRegistry,
        audit_cache::AuditCacheConfig,
        audit_run::{DatAuditRequest, run_dat_audit_with_cache},
        load_dat_sources_config_from,
    },
    updates::{
        ManagedDatSnapshot, ManagedDatSourceDescriptor, ManagedDatState, ManagedDatUpdatePolicy,
        RedumpGameSystem, save_managed_dat_state,
    },
};
use std::collections::HashMap;

type Catalogues = HashMap<String, ConfiguredCatalogue>;

fn catalogues(fixture: &dat_exact_tests::Fixture, managed: bool, enabled: bool) -> Catalogues {
    let root = fixture.db.parent().unwrap();
    let mut local = DatSourceRegistry::new();
    let mut store = ManagedDatSources::new();
    if managed {
        store
            .add_redump_games(
                RedumpGameSystem::PlayStation,
                if enabled {
                    ManagedDatUpdatePolicy::Manual
                } else {
                    ManagedDatUpdatePolicy::Disabled
                },
            )
            .unwrap();
    } else {
        let mut config = load_dat_sources_config_from(root.join("dat_sources.toml")).unwrap();
        config.sources.as_mut().unwrap()[0].enabled = Some(enabled);
        local = DatSourceRegistry::from_config(&config).0;
    }
    configured_catalogues(CatalogueInventoryInputs {
        local_registry: &local,
        managed_sources: &store,
        managed_root: &root.join("managed-dats"),
        limits: Default::default(),
    })
}

fn install_managed(fixture: &dat_exact_tests::Fixture, body: &str) {
    use sha2::{Digest, Sha256};
    let sha: String = Sha256::digest(body.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let descriptor = ManagedDatSourceDescriptor::redump_games(RedumpGameSystem::PlayStation)
        .unwrap()
        .with_update_policy(ManagedDatUpdatePolicy::Manual);
    let state = ManagedDatState::new(&descriptor, ManagedDatSnapshot::new(&sha).unwrap()).unwrap();
    let root = fixture.db.parent().unwrap().join("managed-dats");
    let object = root
        .join(state.source_id.storage_relative_path())
        .join("objects")
        .join(sha);
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(object, body).unwrap();
    save_managed_dat_state(&root, &state).unwrap();
}

fn audited(managed: bool) -> (dat_exact_tests::Fixture, Catalogues) {
    let fixture = dat_exact_tests::fixture(&["Synthetic Game (USA)".into()]);
    let dat = fixture.db.parent().unwrap().join("gbc.dat");
    let body = std::fs::read_to_string(&dat)
        .unwrap()
        .replace("Nintendo - Game Boy Color", "Sony - PlayStation")
        .replace("No-Intro", "Redump");
    std::fs::write(&dat, &body).unwrap();
    if managed {
        install_managed(&fixture, &body);
    }
    let sources = catalogues(&fixture, managed, true);
    let source = sources.values().next().unwrap();
    let request = DatAuditRequest {
        source_id: source.source_id.clone(),
        source_display_name: source.display_name.clone(),
        dat_path: source.path.clone(),
        dat_kind: source.kind,
        // The only audit population is the disposable fixture's tiny directory.
        scan_root: fixture.db.parent().unwrap().join("roms"),
        limits: Default::default(),
        policy: None,
        platform: None,
    };
    let outcome = run_dat_audit_with_cache(
        &request,
        &archivefs_core::safe_read::TrustedRoots::none(),
        &std::sync::atomic::AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    let mut database = archivefs_core::Database::open_or_create(&fixture.db).unwrap();
    database
        .persist_library_dat_identities_from_audit(&outcome)
        .unwrap();
    (fixture, sources)
}

fn verified(fixture: &dat_exact_tests::Fixture, sources: &Catalogues) -> bool {
    let library =
        crate::gui_v2::backend::load_library_with_catalogues(&fixture.db, sources).unwrap();
    let known = knowledge_with_catalogues(&fixture.db, 1, sources).unwrap();
    review_for(
        library.game(1).unwrap(),
        &library.identity_context,
        known.as_ref(),
    )
    .is_verified()
}

#[test]
fn native_review_uses_current_trusted_redump_proof_from_either_store() {
    for managed in [false, true] {
        let (fixture, sources) = audited(managed);
        assert!(verified(&fixture, &sources));
        let known = knowledge_with_catalogues(&fixture.db, 1, &sources)
            .unwrap()
            .unwrap();
        let source = sources.values().next().unwrap();
        assert!(known.technical.contains(&source.source_id));
        assert!(known.technical.contains(source.sha256.as_ref().unwrap()));
        assert!(known.technical.contains("Catalogue revision: 1\n"));
        assert_eq!(known.ecosystem, Some("Redump"));
        assert_eq!(known.title.as_deref(), Some("Synthetic Game (USA)"));
        for id in [2, 3, 4] {
            let library =
                crate::gui_v2::backend::load_library_with_catalogues(&fixture.db, &sources)
                    .unwrap();
            let known = knowledge_with_catalogues(&fixture.db, id, &sources).unwrap();
            assert!(
                !review_for(
                    library.game(id).unwrap(),
                    &library.identity_context,
                    known.as_ref()
                )
                .is_verified()
            );
        }
    }
}

#[test]
fn native_review_withholds_disabled_removed_replaced_and_unproven_catalogues() {
    for managed in [false, true] {
        for change in [
            "disabled",
            "removed",
            "same-version",
            "new-version",
            "missing-digest",
        ] {
            let (fixture, mut sources) = audited(managed);
            let before = std::fs::read(&fixture.db).unwrap();
            assert!(verified(&fixture, &sources));
            match change {
                "disabled" => sources = catalogues(&fixture, managed, false),
                "removed" => sources.clear(),
                "missing-digest" => sources.values_mut().next().unwrap().sha256 = None,
                _ => {
                    let dat = fixture.db.parent().unwrap().join("gbc.dat");
                    let mut body = std::fs::read_to_string(&dat).unwrap();
                    body.push_str("\n<!-- replacement -->");
                    if change == "new-version" {
                        body = body.replace("<version>1</version>", "<version>2</version>");
                    }
                    std::fs::write(&dat, &body).unwrap();
                    if managed {
                        install_managed(&fixture, &body);
                    }
                    sources = catalogues(&fixture, managed, true);
                }
            }
            assert!(
                knowledge_with_catalogues(&fixture.db, 1, &sources)
                    .unwrap()
                    .is_none(),
                "{managed}/{change}"
            );
            assert!(!verified(&fixture, &sources), "{managed}/{change}");
            assert_eq!(
                std::fs::read(&fixture.db).unwrap(),
                before,
                "review never rewrites saved history"
            );
        }
    }
}

#[test]
fn native_review_withholds_changed_missing_and_legacy_file_evidence() {
    for managed in [false, true] {
        for change in ["changed", "missing", "legacy"] {
            let (fixture, sources) = audited(managed);
            let source = sources.values().next().unwrap();
            match change {
                "legacy" => {
                    let mut db = archivefs_core::Database::open_or_create(&fixture.db).unwrap();
                    let mut saved = db
                        .library_dat_identity_for_item(1, &source.source_id)
                        .unwrap()
                        .unwrap();
                    saved.audit_file = None;
                    saved.disc_members.clear();
                    db.persist_library_dat_identity(1, &saved).unwrap();
                }
                _ => {
                    let path = fixture
                        .db
                        .parent()
                        .unwrap()
                        .join("roms/Synthetic Game (USA).gbc");
                    if change == "missing" {
                        std::fs::remove_file(path).unwrap();
                    } else {
                        let mut bytes = std::fs::read(&path).unwrap();
                        bytes[0] ^= 1;
                        std::fs::write(path, bytes).unwrap();
                    }
                }
            }
            let before = std::fs::read(&fixture.db).unwrap();
            assert!(
                knowledge_with_catalogues(&fixture.db, 1, &sources)
                    .unwrap()
                    .is_none(),
                "{managed}/{change}"
            );
            assert!(!verified(&fixture, &sources), "{managed}/{change}");
            assert_eq!(std::fs::read(&fixture.db).unwrap(), before);
        }
    }
}

#[test]
fn completed_native_review_revokes_a_cached_exact_flag_after_catalogue_change() {
    for managed in [false, true] {
        let (fixture, sources) = audited(managed);
        let library =
            crate::gui_v2::backend::load_library_with_catalogues(&fixture.db, &sources).unwrap();
        assert!(library.game(1).unwrap().dat_exact.is_some());
        let context = egui::Context::default();
        let mut app = crate::gui_v2::tests::review_app(&context, vec![]);
        app.library = std::sync::Arc::new(library);
        // Keep the original library snapshot: the catalogue disappears before
        // opening View evidence, whose fresh lookup has now completed.
        app.review.knowledge =
            knowledge_with_catalogues(&fixture.db, 1, &Default::default()).unwrap();
        assert!(app.review.knowledge.is_none());
        assert!(!app.identity_review_for(1).unwrap().is_verified());
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| app.review_identity(ui, 1));
        });
        fn says_verified(shape: &egui::Shape) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text() == "Verified",
                egui::Shape::Vec(shapes) => shapes.iter().any(says_verified),
                _ => false,
            }
        }
        assert!(
            !output
                .shapes
                .iter()
                .any(|shape| says_verified(&shape.shape))
        );
        assert!(
            app.library.game(1).unwrap().dat_exact.is_some(),
            "view projection never mutates the library snapshot"
        );
    }
}

#[test]
fn current_review_preserves_independent_file_identity_without_dat_knowledge() {
    use archivefs_core::game_identity::*;
    let mut archive = crate::gui_v2::tests::archive(1, "pacman", Some("Arcade"));
    archive.identity_report = Some(GameIdentityReport {
        archive_path: archive.absolute_path.clone(),
        platform: IdentityPlatform::Arcade,
        format: IdentityImageFormat::LooseCartridgeRom,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::MameMachineName,
            status: IdentityStatus::Verified,
            value: Some("pacman".into()),
            confidence: IdentityConfidence::ExactBytes,
            provenance: IdentityProvenance {
                archive_path: archive.absolute_path.clone(),
                member_path: None,
                member_index: None,
                method: "synthetic file evidence".into(),
            },
            diagnostic: "synthetic".into(),
        }],
        warnings: vec![],
        bytes_read: 1,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    });
    let mut game = Game::from_archive(archive);
    game.mark_dat_exact(Some("Redump"));
    let review = review_with_current_knowledge(&game, &Default::default(), None);
    assert!(
        matches!(review.state, ReviewState::Verified(facts) if facts.by_file_evidence && facts.source.is_none())
    );
}

#[test]
fn native_review_waits_for_the_lookup_before_showing_a_cached_dat_verdict() {
    let context = egui::Context::default();
    let mut app = crate::gui_v2::tests::review_app(&context, vec![(1, "Synthetic", "PlayStation")]);
    std::sync::Arc::make_mut(&mut app.library).games[0].mark_dat_exact(Some("Redump"));
    app.review.knowledge_for = None;
    app.review.knowledge_job = Some(999); // No worker or real data access.
    let output = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| app.review_identity(ui, 1));
    });
    fn says_verified(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text() == "Verified",
            egui::Shape::Vec(shapes) => shapes.iter().any(says_verified),
            _ => false,
        }
    }
    assert!(
        !output
            .shapes
            .iter()
            .any(|shape| says_verified(&shape.shape))
    );
    assert_eq!(app.review.knowledge_job, Some(999));
}
