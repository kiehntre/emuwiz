//! Focused tests for TOSEC release-pack inventory, classification, selection,
//! persistence and registration. Every fixture is a small synthetic pack in a
//! temporary directory; nothing network-facing exists in this module.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::*;
use crate::dat::sources::DatSourceOwnership;

const AMIGA_GAMES_FLOPPY_DAT: &str = r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>Amiga - Games - Floppy</name>
    <description>Amiga - Games - Floppy (TOSEC-v2021-01-09)</description>
    <version>2021-01-09</version>
    <author>TOSEC</author>
    <homepage>https://www.tosecdev.org/</homepage>
  </header>
  <game name="Test Game (Europe)">
    <rom name="Test Game (Europe).adf" size="4" crc="00000001" md5="00000000000000000000000000000001" sha1="0000000000000000000000000000000000000001"/>
  </game>
  <game name="Another Game (Europe)">
    <rom name="Another Game (Europe).adf" size="4" crc="00000002" md5="00000000000000000000000000000002" sha1="0000000000000000000000000000000000000002"/>
  </game>
</datafile>
"#;

fn dat_xml(name: &str, description: &str, version: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>{name}</name>
    <description>{description}</description>
    <version>{version}</version>
    <author>TOSEC</author>
    <homepage>https://www.tosecdev.org/</homepage>
  </header>
  <game name="Test Entry (Europe)">
    <rom name="Test Entry (Europe).bin" size="4" crc="0000000a" md5="0000000000000000000000000000000a" sha1="000000000000000000000000000000000000000a"/>
  </game>
</datafile>
"#
    )
}

struct PackFixture {
    _root: tempfile::TempDir,
    pack_root: PathBuf,
}

impl PackFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let pack_root = root.path().join("TOSEC_pack_2021");
        std::fs::create_dir_all(&pack_root).unwrap();
        Self {
            _root: root,
            pack_root,
        }
    }

    fn write_dat(&self, relative: &[&str], contents: &str) -> PathBuf {
        let mut path = self.pack_root.clone();
        for segment in relative {
            path.push(segment);
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A representative synthetic official-style pack.
    fn standard() -> Self {
        let fixture = Self::new();
        fixture.write_dat(
            &["Amiga", "Amiga - Games - Floppy (TOSEC-v2021-01-09).dat"],
            AMIGA_GAMES_FLOPPY_DAT,
        );
        fixture.write_dat(
            &[
                "ZX Spectrum",
                "ZX Spectrum - Games - Tape (TOSEC-v2021-01-09).dat",
            ],
            &dat_xml(
                "ZX Spectrum - Games - Tape",
                "ZX Spectrum - Games - Tape (TOSEC-v2021-01-09)",
                "2021-01-09",
            ),
        );
        fixture.write_dat(
            &[
                "ZX Spectrum",
                "ZX Spectrum - Firmware (TOSEC-v2021-01-09).dat",
            ],
            &dat_xml(
                "ZX Spectrum - Firmware",
                "ZX Spectrum - Firmware (TOSEC-v2021-01-09)",
                "2021-01-09",
            ),
        );
        fixture.write_dat(
            &["ZX Spectrum", "ZX Spectrum - Demos (TOSEC-v2021-01-09).dat"],
            &dat_xml(
                "ZX Spectrum - Demos",
                "ZX Spectrum - Demos (TOSEC-v2021-01-09)",
                "2021-01-09",
            ),
        );
        // An unclassifiable catalogue: stays Everything Else.
        fixture.write_dat(
            &[
                "Mystery System",
                "Mystery System - Weird Stuff (TOSEC-v2021-01-09).dat",
            ],
            &dat_xml(
                "Mystery System - Weird Stuff",
                "Mystery System - Weird Stuff (TOSEC-v2021-01-09)",
                "2021-01-09",
            ),
        );
        // Non-DAT junk that must never enter the inventory.
        std::fs::write(fixture.pack_root.join("readme.txt"), "not a dat").unwrap();
        std::fs::write(
            fixture.pack_root.join("cover.png"),
            [0_u8, 1, 2, 3].as_slice(),
        )
        .unwrap();
        fixture
    }
}

fn find_dat<'a>(inventory: &'a TosecPackInventory, leaf: &str) -> &'a TosecPackDat {
    inventory
        .dats
        .iter()
        .find(|dat| dat.relative_path.to_string_lossy().contains(leaf))
        .unwrap_or_else(|| panic!("{leaf} missing from inventory"))
}

#[test]
fn a_small_synthetic_release_directory_is_inventoried() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    assert!(inventory.scan_complete);
    assert_eq!(inventory.dats.len(), 5);
    assert!(!inventory.pack_id.is_empty());
}

#[test]
fn non_dat_junk_is_ignored() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    assert!(
        !inventory
            .dats
            .iter()
            .any(|dat| dat.relative_path.to_string_lossy().contains("readme"))
    );
    assert!(
        !inventory
            .dats
            .iter()
            .any(|dat| dat.relative_path.to_string_lossy().contains("cover.png"))
    );
}

#[test]
fn a_symlink_inside_the_pack_is_never_followed() {
    let fixture = PackFixture::new();
    fixture.write_dat(
        &["Amiga", "Amiga - Games - Floppy (TOSEC-v2021-01-09).dat"],
        AMIGA_GAMES_FLOPPY_DAT,
    );
    // A link that points OUTSIDE the pack, disguised as a DAT and as a folder.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd.dat", fixture.pack_root.join("evil.dat")).unwrap();
        std::os::unix::fs::symlink("/etc", fixture.pack_root.join("escape-dir")).unwrap();
    }
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    // Only the real regular-file DAT is inventoried.
    assert_eq!(inventory.dats.len(), 1);
    assert!(
        inventory
            .skipped
            .iter()
            .any(|skipped| skipped.reason.contains("symbolic link"))
    );
}

#[test]
fn original_relative_paths_are_preserved_verbatim() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let amiga = find_dat(&inventory, "Amiga - Games - Floppy");
    assert_eq!(
        amiga.relative_path,
        PathBuf::from("Amiga").join("Amiga - Games - Floppy (TOSEC-v2021-01-09).dat")
    );
}

#[test]
fn raw_tosec_names_and_categories_are_preserved() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    for dat in &inventory.dats {
        assert!(!dat.raw_catalogue_name.is_empty());
        assert!(!dat.raw_category_label.is_empty());
    }
    let mystery = find_dat(&inventory, "Mystery System");
    assert_eq!(
        mystery.raw_catalogue_name,
        "Mystery System - Weird Stuff (TOSEC-v2021-01-09)"
    );
    assert_eq!(mystery.raw_category_label, "Weird Stuff");
}

#[test]
fn systems_come_from_the_catalogue_name_not_guesses() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    assert_eq!(find_dat(&inventory, "Games - Floppy").system, "Amiga");
    assert_eq!(
        find_dat(&inventory, "ZX Spectrum - Games - Tape").system,
        "ZX Spectrum"
    );
    assert_eq!(
        find_dat(&inventory, "Mystery System").system,
        "Mystery System"
    );
}

#[test]
fn games_tape_and_floppy_are_classified() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let tape = find_dat(&inventory, "ZX Spectrum - Games - Tape");
    assert_eq!(tape.category, TosecFriendlyCategory::Games);
    assert_eq!(tape.media, TosecMediaType::Tape);
    assert!(tape.classification_confident);

    let floppy = find_dat(&inventory, "Amiga - Games - Floppy");
    assert_eq!(floppy.category, TosecFriendlyCategory::Games);
    assert_eq!(floppy.media, TosecMediaType::FloppyDisk);
}

#[test]
fn firmware_and_demos_are_classified() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let firmware = find_dat(&inventory, "ZX Spectrum - Firmware");
    assert_eq!(
        firmware.category,
        TosecFriendlyCategory::FirmwareSystemSoftware
    );
    assert_eq!(firmware.media, TosecMediaType::Firmware);

    let demos = find_dat(&inventory, "ZX Spectrum - Demos");
    assert_eq!(demos.category, TosecFriendlyCategory::DemosScene);
}

#[test]
fn an_uncertain_category_stays_everything_else() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mystery = find_dat(&inventory, "Mystery System");
    assert_eq!(mystery.category, TosecFriendlyCategory::EverythingElse);
    assert!(!mystery.classification_confident);
}

#[test]
fn the_raw_advanced_projection_remains_available() {
    // The raw name and raw category are retained for EVERY entry, so an
    // advanced view can always group by the original TOSEC taxonomy.
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let raw_groups: BTreeSet<String> = inventory
        .dats
        .iter()
        .map(|dat| dat.raw_category_label.clone())
        .collect();
    assert!(raw_groups.contains("Games"));
    assert!(raw_groups.contains("Firmware"));
    assert!(raw_groups.contains("Weird Stuff"));
}

fn persisted_from_inventory(inventory: &TosecPackInventory) -> PersistedTosecPack {
    PersistedTosecPack {
        pack_id: inventory.pack_id.clone(),
        root_path: inventory.pack_root.clone(),
        imported_unix_seconds: 1_700_000_000,
        // Conservative default: importing enables nothing.
        selections: BTreeSet::new(),
        dats: inventory.dats.clone(),
    }
}

fn local_source(id: &str, path: &str) -> DatSourceConfigEntry {
    DatSourceConfigEntry {
        id: id.to_string(),
        display_name: "My own DAT".to_string(),
        path: path.to_string(),
        kind: DatSourceKind::File,
        ownership: DatSourceOwnership::UserLocal,
        enabled: Some(true),
        priority: Some(100),
        platform: None,
        origin: None,
        added_unix_seconds: None,
        health_state: None,
        health_last_validated_unix_seconds: None,
        health_detail: None,
        health_entry_count: None,
        health_rom_count: None,
        health_file_count: None,
        health_formats: None,
        health_observed_size_bytes: None,
        health_observed_modified_unix_seconds: None,
        unknown_fields: toml::Table::new(),
    }
}

fn select_amiga_floppy(pack: &mut PersistedTosecPack) {
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
}

#[test]
fn a_user_can_enable_one_system_category_media_subset() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "ZX Spectrum".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::Tape,
    });
    let selected: Vec<&TosecPackDat> = pack.selected_dats().collect();
    assert_eq!(selected.len(), 1);
    assert!(
        selected[0]
            .relative_path
            .to_string_lossy()
            .contains("Games - Tape")
    );
}

#[test]
fn unselected_dats_are_never_registered() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert_eq!(outcome.registered.len(), 1);
    assert_eq!(outcome.failed.len(), 0);
    assert!(
        outcome.registered[0]
            .entry
            .display_name
            .contains("Amiga - Games - Floppy")
    );
}

#[test]
fn selected_release_pack_dat_gets_typed_ownership_that_survives_registry_reload() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    select_amiga_floppy(&mut pack);
    let registry = fixture._root.path().join("dat_sources.toml");

    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert_eq!(outcome.registered.len(), 1);
    assert!(matches!(
        outcome.registered[0].entry.ownership,
        DatSourceOwnership::ImportedTosecReleasePack {
            ref pack_id,
            ref relative_path,
        } if pack_id == &pack.pack_id && relative_path == &outcome.registered[0].provenance.relative_path
    ));

    let reloaded = load_dat_sources_config_from(&registry).unwrap();
    let entry = &reloaded.sources.unwrap()[0];
    assert_eq!(entry.ownership, outcome.registered[0].entry.ownership);
}

#[test]
fn old_dat_source_config_without_ownership_stays_user_local() {
    let fixture = PackFixture::new();
    let registry = fixture._root.path().join("dat_sources.toml");
    std::fs::write(
        &registry,
        r#"[[sources]]
id = "old-local"
display_name = "Old local DAT"
path = "/tmp/old.dat"
kind = "file"
origin = "TOSEC release pack whatever"
"#,
    )
    .unwrap();
    let config = load_dat_sources_config_from(&registry).unwrap();
    assert_eq!(
        config.sources.unwrap()[0].ownership,
        DatSourceOwnership::UserLocal
    );
}

#[test]
fn deselecting_a_group_removes_only_entries_owned_by_that_exact_pack() {
    let first = PackFixture::standard();
    let first_inventory = inventory_release_pack(&first.pack_root).unwrap();
    let mut first_pack = persisted_from_inventory(&first_inventory);
    select_amiga_floppy(&mut first_pack);

    let second = PackFixture::standard();
    let second_inventory = inventory_release_pack(&second.pack_root).unwrap();
    let mut second_pack = persisted_from_inventory(&second_inventory);
    select_amiga_floppy(&mut second_pack);

    let registry = first._root.path().join("dat_sources.toml");
    apply_selection_to_registry(&first_pack, &registry, 42).unwrap();
    apply_selection_to_registry(&second_pack, &registry, 43).unwrap();

    let mut config = load_dat_sources_config_from(&registry).unwrap();
    let local = local_source("manual-tosec", "/manual/tosec.dat");
    let mut managed = local_source("managed-mame", "/managed/mame.dat");
    managed.ownership = DatSourceOwnership::EmuWizManaged;
    config.sources.as_mut().unwrap().extend([local, managed]);
    save_dat_sources_config_to(&registry, &config).unwrap();

    first_pack.selections.clear();
    let outcome = apply_selection_to_registry(&first_pack, &registry, 44).unwrap();
    assert_eq!(outcome.removed.len(), 1);
    let after = load_dat_sources_config_from(&registry).unwrap();
    let entries = after.sources.unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().any(|entry| {
        matches!(
            &entry.ownership,
            DatSourceOwnership::ImportedTosecReleasePack { pack_id, .. }
                if pack_id == &second_pack.pack_id
        )
    }));
    assert!(entries.iter().any(|entry| entry.id == "manual-tosec"));
    assert!(entries.iter().any(|entry| entry.id == "managed-mame"));
}

#[test]
fn a_deselected_release_pack_group_can_be_reselected() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    select_amiga_floppy(&mut pack);
    let registry = fixture._root.path().join("dat_sources.toml");
    let first = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    let first_id = first.registered[0].entry.id.clone();

    pack.selections.clear();
    let removed = apply_selection_to_registry(&pack, &registry, 43).unwrap();
    assert_eq!(removed.removed.len(), 1);

    select_amiga_floppy(&mut pack);
    let reselected = apply_selection_to_registry(&pack, &registry, 44).unwrap();
    assert_eq!(reselected.registered[0].entry.id, first_id);
    let entries = load_dat_sources_config_from(&registry)
        .unwrap()
        .sources
        .unwrap();
    assert_eq!(entries.len(), 1);
}

#[test]
fn selections_survive_persistence_and_reload() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "ZX Spectrum".to_string(),
        category: TosecFriendlyCategory::FirmwareSystemSoftware,
        media: TosecMediaType::Firmware,
    });
    let path = fixture._root.path().join("tosec_release_packs.json");
    save_tosec_packs(&path, &[pack.clone()]).unwrap();
    let reloaded = load_tosec_packs(&path).unwrap();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0], pack);
    assert_eq!(reloaded[0].selections.len(), 1);
}

#[test]
fn exact_provenance_is_retained_for_registered_dats() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    let registered = &outcome.registered[0];
    assert_eq!(registered.provenance.pack_id, pack.pack_id);
    assert_eq!(
        registered.provenance.tosec_header_name,
        "Amiga - Games - Floppy"
    );
    assert_eq!(
        registered.provenance.tosec_version.as_deref(),
        Some("2021-01-09")
    );
    assert!(registered.provenance.content_sha256.is_some());
    // The registered origin line carries the same provenance.
    let origin = registered.entry.origin.clone().unwrap();
    assert!(origin.contains("TOSEC release pack"));
    assert!(origin.contains(registered.provenance.content_sha256.as_deref().unwrap()));
}

#[test]
fn a_selected_dat_feeds_the_existing_tosec_parser_and_evidence_path() {
    // Registration validates through import_tosec_dat: the bounded parser,
    // the internal TOSEC ecosystem gate and the DatIndex. Prove the selected
    // DAT passes that exact existing path with real parsed content.
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let amiga = find_dat(&inventory, "Amiga - Games - Floppy");
    let imported = crate::identity_source::tosec::import_tosec_dat(
        &fixture.pack_root.join(&amiga.relative_path),
    )
    .unwrap();
    assert_eq!(imported.system_name, "Amiga - Games - Floppy");
    assert!(!imported.index.by_crc32.is_empty());
    assert!(imported.index.lookup_crc32("00000001").len() == 1);
}

#[test]
fn the_pack_is_never_modified_by_inventory_or_registration() {
    let fixture = PackFixture::standard();
    let before: BTreeSet<(PathBuf, Vec<u8>)> = {
        let mut set = BTreeSet::new();
        fn collect(dir: &Path, set: &mut BTreeSet<(PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, set);
                } else {
                    set.insert((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        collect(&fixture.pack_root, &mut set);
        set
    };

    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let registry = fixture._root.path().join("dat_sources.toml");
    apply_selection_to_registry(&pack, &registry, 42).unwrap();

    let after: BTreeSet<(PathBuf, Vec<u8>)> = {
        let mut set = BTreeSet::new();
        fn collect(dir: &Path, set: &mut BTreeSet<(PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, set);
                } else {
                    set.insert((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        collect(&fixture.pack_root, &mut set);
        set
    };
    assert_eq!(before, after, "the pack must be byte-identical after use");
}

#[test]
fn a_missing_pack_after_restart_is_reported_not_deleted() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    // Persist, then simulate the pack disappearing.
    let packs_path = fixture._root.path().join("tosec_release_packs.json");
    save_tosec_packs(&packs_path, &[pack.clone()]).unwrap();
    std::fs::remove_dir_all(&fixture.pack_root).unwrap();

    let reloaded = load_tosec_packs(&packs_path).unwrap();
    assert_eq!(
        reloaded.len(),
        1,
        "the configuration is never silently deleted"
    );
    assert_eq!(reloaded[0].availability(), PackAvailability::Missing);

    // A registration attempt against the missing pack fails honestly and
    // registers nothing.
    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&reloaded[0], &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(outcome.failed[0].1.contains("no longer available"));
}

#[test]
fn importing_a_pack_enables_nothing_by_default() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let pack = persisted_from_inventory(&inventory);
    assert!(pack.selections.is_empty());
    assert_eq!(pack.selected_dats().count(), 0);
    // And a registration run with no selections touches no registry entries.
    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
}

#[test]
fn existing_local_dat_sources_are_preserved_when_a_tosec_pack_registers() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    // Pre-existing user-local source in the same registry.
    let registry = fixture._root.path().join("dat_sources.toml");
    let mut sources = load_dat_sources_config_from(&registry).unwrap_or_default();
    sources
        .sources
        .get_or_insert_with(Vec::new)
        .push(local_source("user-dat", "/somewhere/user.dat"));
    save_dat_sources_config_to(&registry, &sources).unwrap();

    apply_selection_to_registry(&pack, &registry, 42).unwrap();
    let after = load_dat_sources_config_from(&registry).unwrap();
    let list = after.sources.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|entry| entry.id == "user-dat"));
    assert!(list.iter().any(|entry| entry.id.starts_with("tosec-")));
}

#[test]
fn an_unrelated_local_source_with_a_tosec_id_collision_is_not_replaced() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    let selection = selected.selection_key();
    let generated_id = registration_id(&pack, selected);
    pack.selections.insert(selection);

    let mut sources = DatSourcesConfig::default();
    sources
        .sources
        .get_or_insert_with(Vec::new)
        .push(local_source(&generated_id, "/unrelated/user.dat"));
    let outcome = register_selected_tosec_dats(&pack, &mut sources, 42);
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.conflicts.len(), 1);
    assert!(outcome.conflicts[0].1.contains("source ID"));
    assert_eq!(sources.sources.unwrap()[0].path, "/unrelated/user.dat");
}

#[test]
fn a_tosec_looking_filename_with_non_tosec_contents_is_rejected_at_registration() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    std::fs::write(
        fixture.pack_root.join(&selected.relative_path),
        "<datafile><header><name>Not TOSEC</name></header></datafile>",
    )
    .unwrap();

    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(
        outcome.failed[0]
            .1
            .contains("does not identify itself as TOSEC")
    );
}

#[test]
fn deferred_tosec_iso_catalogues_remain_in_inventory_but_are_not_registered() {
    let fixture = PackFixture::new();
    fixture.write_dat(
        &["TOSEC-ISO - PC.dat"],
        &dat_xml("TOSEC-ISO - PC", "TOSEC-ISO - PC", "2021-01-09"),
    );
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    assert_eq!(inventory.dats.len(), 1, "inventory is read-only projection");
    let mut pack = persisted_from_inventory(&inventory);
    let selection = pack.dats[0].selection_key();
    pack.selections.insert(selection);

    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.deferred.len(), 1);
    assert!(
        outcome.deferred[0]
            .1
            .contains("deferred TOSEC ISO catalogue")
    );
}

#[test]
fn registration_revalidates_changed_selected_bytes_and_replaces_its_own_entry() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    let inventory_digest = selected.content_sha256.clone().unwrap();
    std::fs::write(
        fixture.pack_root.join(&selected.relative_path),
        dat_xml(
            "Amiga - Games - Floppy",
            "Amiga - Games - Floppy (TOSEC-v2022-02-02)",
            "2022-02-02",
        ),
    )
    .unwrap();

    let registry = fixture._root.path().join("dat_sources.toml");
    let first = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert_eq!(first.registered.len(), 1, "failed: {:?}", first.failed);
    assert_ne!(
        first.registered[0].provenance.content_sha256.as_deref(),
        Some(inventory_digest.as_str()),
        "registration provenance must describe the bytes it actually parsed"
    );
    let first_id = first.registered[0].entry.id.clone();

    let second = apply_selection_to_registry(&pack, &registry, 43).unwrap();
    assert_eq!(second.registered.len(), 0);
    assert_eq!(second.already_registered.len(), 1);
    assert_eq!(
        load_dat_sources_config_from(&registry)
            .unwrap()
            .sources
            .unwrap()[0]
            .id,
        first_id
    );
    let saved = load_dat_sources_config_from(&registry).unwrap();
    assert_eq!(
        saved.sources.unwrap().len(),
        1,
        "a changed pack DAT must replace this pack/path's entry, not leave stale provenance"
    );
}

#[test]
fn an_exact_user_local_pack_path_is_already_satisfied_without_duplication() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    select_amiga_floppy(&mut pack);
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    let selected_path = fixture.pack_root.join(&selected.relative_path);
    let mut sources = DatSourcesConfig {
        sources: Some(vec![local_source(
            "manual-amiga",
            selected_path.to_str().unwrap(),
        )]),
        ..Default::default()
    };

    let outcome = register_selected_tosec_dats(&pack, &mut sources, 42);
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.already_registered.len(), 1);
    assert!(outcome.conflicts.is_empty());
    assert_eq!(sources.sources.unwrap().len(), 1);
}

#[test]
fn a_mixed_selection_reports_registered_already_deferred_and_conflict_separately() {
    let fixture = PackFixture::standard();
    fixture.write_dat(
        &["TOSEC-ISO - PC.dat"],
        &dat_xml("TOSEC-ISO - PC", "TOSEC-ISO - PC", "2021-01-09"),
    );
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    select_amiga_floppy(&mut pack);
    let iso = find_dat(&inventory, "TOSEC-ISO - PC");
    pack.selections.insert(iso.selection_key());
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    let generated_id = registration_id(&pack, selected);
    pack.selections.insert(TosecSelectionKey {
        system: "ZX Spectrum".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::Tape,
    });
    let mut sources = DatSourcesConfig {
        sources: Some(vec![local_source(&generated_id, "/unrelated/user.dat")]),
        ..Default::default()
    };

    let outcome = register_selected_tosec_dats(&pack, &mut sources, 42);
    assert_eq!(outcome.registered.len(), 1);
    assert_eq!(outcome.already_registered.len(), 0);
    assert_eq!(outcome.deferred.len(), 1);
    assert_eq!(outcome.conflicts.len(), 1);
    assert!(outcome.failed.is_empty());
}

#[test]
fn malicious_persisted_relative_path_cannot_register_outside_the_pack() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.dats[0].relative_path = PathBuf::from("..").join("outside.dat");
    let selection = pack.dats[0].selection_key();
    pack.selections.insert(selection);

    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(outcome.failed[0].1.contains("safe relative path"));
}

#[test]
fn malformed_persisted_pack_config_is_reported_not_silently_forgotten() {
    let fixture = PackFixture::new();
    let config = fixture._root.path().join("tosec_release_packs.json");
    std::fs::write(&config, "{ definitely not json").unwrap();
    assert!(load_tosec_packs(&config).is_err());
}
