//! GUI Maintenance Batch 2: relocated from main.rs's inline test module.
//! Exercises `crate::gather_selected_evidence_with_registry`/
//! `gather_selected_evidence_with_registry_at` - the GUI Batch A main.rs-side
//! wiring for the Selected-ROM evidence panel's registry-backed No-Intro
//! lookup. Copied byte-for-byte from its original nested-module location;
//! only its file location changed.

use super::*;

// -- GUI Batch A closeout: registry-backed No-Intro wiring -----------
//
// `gather_selected_evidence_with_registry_at` is exactly what
// `start_selected_evidence_load` calls in the real, running GUI (via
// `gather_selected_evidence_with_registry`, which only adds the real
// default config path). These tests exercise that same function
// end-to-end against real files - a temp DAT source, a real registry
// config on disk, and a real ROM file - never against the developer's
// own home directory.
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
