//! Selected-game identity/evidence state-machine tests, plus the legacy
//! registry-backed No-Intro gatherer's focused wiring coverage.

use super::*;

// -- GUI Batch A closeout: registry-backed No-Intro wiring -----------
//
// These tests exercise the registry resolver end-to-end against real files -
// a temp DAT source, a real registry config on disk, and a real ROM file -
// never against the developer's own home directory. The live staged worker
// path is exercised by the state-machine regressions below.
mod selected_evidence_registry_wiring {
    use super::*;
    use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
    use std::io::Write;

    const GB_NO_INTRO_XML: &str = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Nintendo - Game Boy</name>
        <version>20250101-120000</version>
        <author>No-Intro</author>
    </header>
    <game name="Alleyway (World)">
        <rom name="Alleyway (World).gb" size="336" crc="00000000" sha1="__SHA1__"/>
    </game>
</datafile>"#;

    const GB_NO_INTRO_XML_OTHER: &str = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Nintendo - Game Boy (Rebuild)</name>
        <version>20250601-000000</version>
        <author>No-Intro</author>
    </header>
    <game name="Tetris (World)">
        <rom name="Tetris (World).gb" size="1" crc="00000000" sha1="0000000000000000000000000000000000000a"/>
    </game>
</datafile>"#;

    /// A self-cleaning fixture directory - matches
    /// `selected_evidence_page::tests::FixtureDir`'s own convention.
    struct FixtureDir(PathBuf);

    impl FixtureDir {
        fn new(label: &str) -> Self {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "archivefs-gui-selected-evidence-registry-{label}-{now}"
            ));
            std::fs::create_dir_all(&dir).expect("create fixture dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A minimal, deterministic synthetic Game Boy ROM - same
    /// convention `selected_evidence_page::tests::gb_rom_bytes` uses.
    fn gb_rom_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x150];
        let logo: [u8; 48] = [
            0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C,
            0x00, 0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6,
            0xDD, 0xDD, 0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC,
            0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
        ];
        bytes[0x104..0x134].copy_from_slice(&logo);
        bytes[0x134..0x143].copy_from_slice(b"TESTGAME\0\0\0\0\0\0\0");
        let checksum = archivefs_core::gb_header_evidence::compute_header_checksum(&bytes)
            .expect("checksum computable");
        bytes[0x14D] = checksum;
        bytes
    }

    fn write_rom(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).expect("create rom fixture");
        file.write_all(bytes).expect("write rom fixture");
        path
    }

    fn write_dat_matching(dir: &Path, name: &str, rom_bytes: &[u8]) -> PathBuf {
        let sha1 = archivefs_core::identity_source::hashing::hash_file(
            &write_rom(dir, "sha1-source.gb", rom_bytes),
            &archivefs_core::safe_read::TrustedRoots::from_paths([dir]),
            None,
        )
        .expect("hash the fixture rom")
        .sha1;
        let xml = GB_NO_INTRO_XML.replace("__SHA1__", &sha1);
        let path = dir.join(name);
        std::fs::write(&path, xml).expect("write dat fixture");
        path
    }

    fn write_dat_sources_config(dir: &Path, sources: &DatSourceRegistry) -> PathBuf {
        let config_path = dir.join("dat_sources.toml");
        archivefs_core::dat::sources::save_dat_sources_config_to(
            &config_path,
            &sources.to_config(),
        )
        .expect("write dat sources config");
        config_path
    }

    fn file_source(id: &str, path: PathBuf, platform: Option<&str>) -> DatSourceEntry {
        let mut entry =
            DatSourceEntry::new(id.to_string(), id.to_string(), path, DatSourceKind::File);
        entry.platform = platform.map(str::to_string);
        entry
    }

    #[test]
    fn configured_no_intro_source_reaches_the_real_selected_evidence_lookup() {
        let dir = FixtureDir::new("configured");
        let rom_bytes = gb_rom_bytes();
        let rom_path = write_rom(dir.path(), "Alleyway (Test).gb", &rom_bytes);
        let dat_path = write_dat_matching(dir.path(), "gb.dat", &rom_bytes);

        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_source("gb-no-intro", dat_path, Some("Game Boy")))
            .unwrap();
        let config_path = write_dat_sources_config(dir.path(), &registry);

        let cache = Mutex::new(selected_evidence_no_intro::NoIntroSourceCache::new());
        let report = gather_selected_evidence_with_registry_at(
            &rom_path,
            &cache,
            Some(config_path.as_path()),
        )
        .expect("gather succeeds");

        match report.no_intro {
            selected_evidence_page::NoIntroLookupResult::Matched { system_name, .. } => {
                assert_eq!(system_name, "Nintendo - Game Boy");
            }
            other => panic!("expected a real registry-backed match, got {other:?}"),
        }
    }

    #[test]
    fn no_configured_source_is_not_imported() {
        let dir = FixtureDir::new("none");
        let rom_bytes = gb_rom_bytes();
        let rom_path = write_rom(dir.path(), "game.gb", &rom_bytes);
        let config_path = write_dat_sources_config(dir.path(), &DatSourceRegistry::new());

        let cache = Mutex::new(selected_evidence_no_intro::NoIntroSourceCache::new());
        let report = gather_selected_evidence_with_registry_at(
            &rom_path,
            &cache,
            Some(config_path.as_path()),
        )
        .expect("gather succeeds");

        assert!(matches!(
            report.no_intro,
            selected_evidence_page::NoIntroLookupResult::NotImported
        ));
    }

    #[test]
    fn disabled_source_is_ignored_by_the_real_gather_path() {
        let dir = FixtureDir::new("disabled");
        let rom_bytes = gb_rom_bytes();
        let rom_path = write_rom(dir.path(), "game.gb", &rom_bytes);
        let dat_path = write_dat_matching(dir.path(), "gb.dat", &rom_bytes);

        let mut registry = DatSourceRegistry::new();
        let mut entry = file_source("gb-no-intro", dat_path, Some("Game Boy"));
        entry.enabled = false;
        registry.add(entry).unwrap();
        let config_path = write_dat_sources_config(dir.path(), &registry);

        let cache = Mutex::new(selected_evidence_no_intro::NoIntroSourceCache::new());
        let report = gather_selected_evidence_with_registry_at(
            &rom_path,
            &cache,
            Some(config_path.as_path()),
        )
        .expect("gather succeeds");

        assert!(matches!(
            report.no_intro,
            selected_evidence_page::NoIntroLookupResult::NotImported
        ));
    }

    #[test]
    fn wrong_platform_source_is_ignored_by_the_real_gather_path() {
        let dir = FixtureDir::new("wrong-platform");
        let rom_bytes = gb_rom_bytes();
        let rom_path = write_rom(dir.path(), "game.gb", &rom_bytes);
        let dat_path = write_dat_matching(dir.path(), "gb.dat", &rom_bytes);

        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_source("gb-no-intro", dat_path, Some("NES")))
            .unwrap();
        let config_path = write_dat_sources_config(dir.path(), &registry);

        let cache = Mutex::new(selected_evidence_no_intro::NoIntroSourceCache::new());
        let report = gather_selected_evidence_with_registry_at(
            &rom_path,
            &cache,
            Some(config_path.as_path()),
        )
        .expect("gather succeeds");

        assert!(matches!(
            report.no_intro,
            selected_evidence_page::NoIntroLookupResult::NotImported
        ));
    }

    #[test]
    fn multiple_compatible_sources_fail_closed_to_ambiguous_never_a_first_pick() {
        let dir = FixtureDir::new("ambiguous");
        let rom_bytes = gb_rom_bytes();
        let rom_path = write_rom(dir.path(), "game.gb", &rom_bytes);
        let dat_path_a = write_dat_matching(dir.path(), "gb-a.dat", &rom_bytes);
        let dat_path_b = dir.path().join("gb-b.dat");
        std::fs::write(&dat_path_b, GB_NO_INTRO_XML_OTHER).unwrap();

        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_source("gb-a", dat_path_a, Some("Game Boy")))
            .unwrap();
        registry
            .add(file_source("gb-b", dat_path_b, Some("Game Boy")))
            .unwrap();
        let config_path = write_dat_sources_config(dir.path(), &registry);

        let cache = Mutex::new(selected_evidence_no_intro::NoIntroSourceCache::new());
        let report = gather_selected_evidence_with_registry_at(
            &rom_path,
            &cache,
            Some(config_path.as_path()),
        )
        .expect("gather succeeds");

        match report.no_intro {
            selected_evidence_page::NoIntroLookupResult::Ambiguous { note } => {
                assert!(note.contains("gb-a"));
                assert!(note.contains("gb-b"));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
        // Ambiguity must never leak a fabricated match into the merged
        // evidence set - `base_observations` must carry no LocalNoIntro
        // observation when the source could not be honestly resolved.
        assert!(report.base_observations.iter().all(|observation| {
                observation.provenance.channel
                    != archivefs_core::platform_evidence_fusion::evidence_lineage::EvidenceChannel::LocalNoIntro
            }));
    }

    #[test]
    fn a_stale_disabled_source_does_not_keep_serving_cached_evidence() {
        let dir = FixtureDir::new("stale");
        let rom_bytes = gb_rom_bytes();
        let rom_path = write_rom(dir.path(), "game.gb", &rom_bytes);
        let dat_path = write_dat_matching(dir.path(), "gb.dat", &rom_bytes);

        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_source("gb-no-intro", dat_path, Some("Game Boy")))
            .unwrap();
        let config_path = write_dat_sources_config(dir.path(), &registry);

        let cache = Mutex::new(selected_evidence_no_intro::NoIntroSourceCache::new());
        let first = gather_selected_evidence_with_registry_at(
            &rom_path,
            &cache,
            Some(config_path.as_path()),
        )
        .expect("gather succeeds");
        assert!(matches!(
            first.no_intro,
            selected_evidence_page::NoIntroLookupResult::Matched { .. }
        ));

        // Disable the source on disk and rewrite the same config path -
        // the same cache instance must notice on the very next gather,
        // not keep serving the earlier Matched result.
        registry.get_mut("gb-no-intro").unwrap().enabled = false;
        write_dat_sources_config(dir.path(), &registry);

        let second = gather_selected_evidence_with_registry_at(
            &rom_path,
            &cache,
            Some(config_path.as_path()),
        )
        .expect("gather succeeds");
        assert!(matches!(
            second.no_intro,
            selected_evidence_page::NoIntroLookupResult::NotImported
        ));
    }

    #[test]
    fn absent_config_path_behaves_like_an_empty_registry_not_a_panic() {
        let dir = FixtureDir::new("no-config");
        let rom_bytes = gb_rom_bytes();
        let rom_path = write_rom(dir.path(), "game.gb", &rom_bytes);

        let cache = Mutex::new(selected_evidence_no_intro::NoIntroSourceCache::new());
        let report = gather_selected_evidence_with_registry_at(&rom_path, &cache, None)
            .expect("gather succeeds even with no resolvable config path");

        assert!(matches!(
            report.no_intro,
            selected_evidence_page::NoIntroLookupResult::NotImported
        ));
    }
}

fn bounded_rar_report(path: &Path) -> selected_evidence_page::SelectedEvidenceReport {
    std::fs::write(path, b"Rar!\x1a\x07\x01\0").expect("write bounded RAR marker fixture");
    selected_evidence_page::gather_selected_evidence_fast(path, Some("PlayStation 2"))
        .expect("bounded archive gather succeeds")
}

#[test]
fn live_selection_state_does_not_start_automatic_enrichment_for_an_archive() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("God of War II (Test).rar");
    let report = bounded_rar_report(&path);
    let mut app = app_for_operation_tests();
    app.archive_context.select_only(path);
    app.selected_evidence_generation = 7;
    app.selected_evidence = selected_evidence_page::SelectedEvidenceState::Ready {
        generation: 7,
        report: Box::new(report),
        hasheous: selected_evidence_page::HasheousState::Idle,
    };

    app.maybe_start_selected_evidence_enrichment(&egui::Context::default());

    assert!(matches!(
        app.selected_evidence_enrichment,
        SelectedEvidenceEnrichmentState::Idle
    ));
    let selected_evidence_page::SelectedEvidenceState::Ready { report, .. } =
        &app.selected_evidence
    else {
        panic!("the bounded archive report must remain ready");
    };
    assert_eq!(
        report.enrichment,
        selected_evidence_page::SelectedEvidenceEnrichmentStatus::SkippedArchive
    );
}

#[test]
fn changing_selection_cancels_and_detaches_the_old_evidence_generation() {
    let mut app = app_for_operation_tests();
    let old_path = PathBuf::from("/library/old.zip");
    let new_path = PathBuf::from("/library/new.zip");
    let (_sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.archive_context.select_only(new_path);
    app.selected_evidence_generation = 3;
    app.selected_evidence_cancel = Some(Arc::clone(&cancel));
    app.selected_evidence = selected_evidence_page::SelectedEvidenceState::Loading {
        generation: 3,
        path: old_path,
        receiver,
    };

    app.reconcile_selected_evidence_selection();

    assert!(cancel.load(Ordering::Relaxed));
    assert!(matches!(
        app.selected_evidence,
        selected_evidence_page::SelectedEvidenceState::Idle
    ));
    assert!(matches!(
        app.selected_evidence_enrichment,
        SelectedEvidenceEnrichmentState::Idle
    ));
}

#[test]
fn disconnected_identity_worker_becomes_a_visible_error_instead_of_loading_forever() {
    let mut app = app_for_operation_tests();
    let path = PathBuf::from("/library/game.zip");
    let (sender, receiver) = mpsc::channel();
    drop(sender);
    app.archive_context.select_only(path.clone());
    app.selected_evidence_generation = 11;
    app.selected_evidence = selected_evidence_page::SelectedEvidenceState::Loading {
        generation: 11,
        path: path.clone(),
        receiver,
    };

    app.poll_selected_evidence();

    match &app.selected_evidence {
        selected_evidence_page::SelectedEvidenceState::Error {
            path: error_path,
            message,
            ..
        } => {
            assert_eq!(error_path, &path);
            assert!(message.contains("stopped without returning a result"));
        }
        _ => panic!("a disconnected worker must settle to the panel's visible Error state"),
    }
}

#[test]
fn disconnected_enrichment_worker_is_visible_on_the_ready_selection_card() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("selected.rar");
    let mut report = bounded_rar_report(&path);
    report.enrichment = selected_evidence_page::SelectedEvidenceEnrichmentStatus::Pending;
    let (sender, receiver) = mpsc::channel();
    drop(sender);
    let mut app = app_for_operation_tests();
    app.archive_context.select_only(path.clone());
    app.selected_evidence_generation = 13;
    app.selected_evidence = selected_evidence_page::SelectedEvidenceState::Ready {
        generation: 13,
        report: Box::new(report),
        hasheous: selected_evidence_page::HasheousState::Idle,
    };
    app.selected_evidence_enrichment = SelectedEvidenceEnrichmentState::Loading {
        generation: 13,
        path,
        receiver,
    };

    app.poll_selected_evidence();

    let selected_evidence_page::SelectedEvidenceState::Ready { report, .. } =
        &app.selected_evidence
    else {
        panic!("base identity remains ready when optional enrichment fails");
    };
    match &report.enrichment {
        selected_evidence_page::SelectedEvidenceEnrichmentStatus::Failed(message) => {
            assert!(message.contains("stopped without returning a result"));
        }
        other => panic!("expected visible enrichment failure, got {other:?}"),
    }
}
